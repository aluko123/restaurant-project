use std::{
    borrow::Cow,
    str::FromStr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderValue, Method, Request, StatusCode, header},
};
use bigdecimal::BigDecimal;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, encode};
use rand::thread_rng;
use rsa::{
    RsaPrivateKey,
    pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding},
};
use serde_json::{Value, json};
use sqlx::{
    PgPool,
    migrate::{MigrateError, Migrator},
    postgres::{PgConnectOptions, PgPoolOptions},
};
use tower::ServiceExt;
use uuid::Uuid;

use crate::{
    AppState,
    auth::JwtVerifier,
    router,
    storage::ObjectStorage,
    workos::{MockWorkos, WorkosClient},
};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");
const TEST_ISSUER: &str = "https://api.workos.com/user_management/client_release_tests";
const TEST_KID: &str = "release-test-key";

struct TestDatabase {
    name: String,
    admin: PgPool,
    pool: PgPool,
}

impl TestDatabase {
    async fn create(label: &str) -> Self {
        let url = std::env::var("TEST_DATABASE_URL").expect(
            "TEST_DATABASE_URL must point to a PostgreSQL database whose user can create databases",
        );
        let options = PgConnectOptions::from_str(&url).expect("TEST_DATABASE_URL must be valid");
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options.clone())
            .await
            .expect("release tests could not connect to TEST_DATABASE_URL");
        let name = format!("restaurant_release_{}_{}", label, Uuid::now_v7().simple());
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(&admin)
            .await
            .expect("release tests could not create a disposable database");
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect_with(options.database(&name))
            .await
            .expect("release tests could not connect to the disposable database");
        Self { name, admin, pool }
    }

    async fn drop(self) {
        self.pool.close().await;
        sqlx::query(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity
             WHERE datname=$1 AND pid<>pg_backend_pid()",
        )
        .bind(&self.name)
        .execute(&self.admin)
        .await
        .expect("could not terminate disposable database sessions");
        sqlx::query(&format!("DROP DATABASE {}", self.name))
            .execute(&self.admin)
            .await
            .expect("could not drop the disposable database");
        self.admin.close().await;
    }
}

#[derive(Clone)]
struct TestAuth {
    encoding_key: Arc<EncodingKey>,
}

impl TestAuth {
    fn create() -> (Self, JwtVerifier) {
        let private_key =
            RsaPrivateKey::new(&mut thread_rng(), 2048).expect("test RSA key generation failed");
        let private_pem = private_key
            .to_pkcs8_pem(LineEnding::LF)
            .expect("test private key encoding failed");
        let public_pem = private_key
            .to_public_key()
            .to_public_key_pem(LineEnding::LF)
            .expect("test public key encoding failed");
        let encoding_key = EncodingKey::from_rsa_pem(private_pem.as_bytes())
            .expect("test encoding key was invalid");
        let decoding_key = DecodingKey::from_rsa_pem(public_pem.as_bytes())
            .expect("test decoding key was invalid");
        (
            Self {
                encoding_key: Arc::new(encoding_key),
            },
            JwtVerifier::with_test_key(TEST_ISSUER, TEST_KID, decoding_key),
        )
    }

    fn token(&self, subject: &str) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.into());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is before the Unix epoch")
            .as_secs();
        encode(
            &header,
            &json!({
                "iss": TEST_ISSUER,
                "sub": subject,
                "sid": format!("session-{subject}"),
                "exp": now + 3600,
            }),
            &self.encoding_key,
        )
        .expect("test JWT signing failed")
    }
}

#[derive(Clone, Copy)]
struct FixtureIds {
    restaurant_a: Uuid,
    restaurant_b: Uuid,
    owner_a: Uuid,
    inventory_a: Uuid,
    inventory_b: Uuid,
}

struct ApiFixture {
    database: TestDatabase,
    app: Router,
    auth: TestAuth,
    ids: FixtureIds,
    workos: MockWorkos,
}

impl ApiFixture {
    async fn create(label: &str) -> Self {
        let database = TestDatabase::create(label).await;
        MIGRATOR
            .run(&database.pool)
            .await
            .expect("fresh migrations failed");
        let ids = seed_tenants(&database.pool).await;
        let (auth, verifier) = TestAuth::create();
        let workos = MockWorkos::default();
        let app = router(
            AppState {
                pool: database.pool.clone(),
                verifier,
                storage: ObjectStorage::inert_for_tests(),
                workos: WorkosClient::mock(workos.clone()),
            },
            HeaderValue::from_static("http://localhost:5173"),
        );
        Self {
            database,
            app,
            auth,
            ids,
            workos,
        }
    }

    fn token(&self, subject: &str) -> String {
        self.auth.token(subject)
    }

    async fn drop(self) {
        drop(self.app);
        self.database.drop().await;
    }
}

struct ApiResponse {
    status: StatusCode,
    body: Value,
}

async fn request(
    app: Router,
    token: Option<&str>,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> ApiResponse {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = match body {
        Some(body) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&body).unwrap())
        }
        None => Body::empty(),
    };
    let response = app
        .oneshot(builder.body(body).unwrap())
        .await
        .expect("router request failed");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .expect("response body could not be read");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("response was not JSON")
    };
    ApiResponse { status, body }
}

fn decimal(value: &str) -> BigDecimal {
    BigDecimal::from_str(value).unwrap()
}

fn assert_decimal_json(value: &Value, expected: &str) {
    let actual = value
        .as_str()
        .and_then(|value| BigDecimal::from_str(value).ok())
        .expect("API decimal must be a decimal string");
    assert_eq!(actual, decimal(expected));
}

async fn seed_tenants(pool: &PgPool) -> FixtureIds {
    let restaurant_a = Uuid::now_v7();
    let restaurant_b = Uuid::now_v7();
    for (id, name, city) in [
        (restaurant_a, "Tenant A Kitchen", "Dallas"),
        (restaurant_b, "Tenant B Kitchen", "Austin"),
    ] {
        sqlx::query(
            "INSERT INTO restaurants(id,name,city,service_style) VALUES($1,$2,$3,'fast_casual')",
        )
        .bind(id)
        .bind(name)
        .bind(city)
        .execute(pool)
        .await
        .unwrap();
    }

    let owner_a = Uuid::now_v7();
    let manager_a = Uuid::now_v7();
    let staff_a = Uuid::now_v7();
    let owner_b = Uuid::now_v7();
    for (id, subject, restaurant, role) in [
        (owner_a, "owner-a", restaurant_a, "owner"),
        (manager_a, "manager-a", restaurant_a, "manager"),
        (staff_a, "staff-a", restaurant_a, "staff"),
        (owner_b, "owner-b", restaurant_b, "owner"),
    ] {
        sqlx::query("INSERT INTO users(id,auth_subject) VALUES($1,$2)")
            .bind(id)
            .bind(subject)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO restaurant_memberships(restaurant_id,user_id,role) VALUES($1,$2,$3)",
        )
        .bind(restaurant)
        .bind(id)
        .bind(role)
        .execute(pool)
        .await
        .unwrap();
    }

    let inventory_a = Uuid::now_v7();
    let inventory_b = Uuid::now_v7();
    for (id, restaurant, name) in [
        (inventory_a, restaurant_a, "Tenant A Tomatoes"),
        (inventory_b, restaurant_b, "Tenant B Avocados"),
    ] {
        sqlx::query(
            "INSERT INTO inventory_items(id,restaurant_id,name,count_unit,par_level)
             VALUES($1,$2,$3,'lb',$4)",
        )
        .bind(id)
        .bind(restaurant)
        .bind(name)
        .bind(decimal("0.000001"))
        .execute(pool)
        .await
        .unwrap();
    }

    FixtureIds {
        restaurant_a,
        restaurant_b,
        owner_a,
        inventory_a,
        inventory_b,
    }
}

#[tokio::test]
#[ignore = "run by scripts/release-gate.sh with disposable PostgreSQL databases"]
async fn migrations_support_fresh_upgrade_and_checksum_safety() {
    let fresh = TestDatabase::create("migrations_fresh").await;
    MIGRATOR
        .run(&fresh.pool)
        .await
        .expect("fresh migration run failed");
    MIGRATOR
        .run(&fresh.pool)
        .await
        .expect("an unchanged migration rerun must be safe");
    let expected = MIGRATOR
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .count() as i64;
    let applied = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&fresh.pool)
        .await
        .unwrap();
    assert_eq!(applied, expected);

    let changed_version = MIGRATOR.iter().next().unwrap().version;
    sqlx::query("UPDATE _sqlx_migrations SET checksum=$1 WHERE version=$2")
        .bind(vec![0_u8; 48])
        .bind(changed_version)
        .execute(&fresh.pool)
        .await
        .unwrap();
    let error = MIGRATOR.run(&fresh.pool).await.unwrap_err();
    assert!(
        matches!(error, MigrateError::VersionMismatch(version) if version == changed_version),
        "unexpected checksum error: {error}"
    );
    fresh.drop().await;

    let upgrade = TestDatabase::create("migrations_upgrade").await;
    let migrations = MIGRATOR.iter().cloned().collect::<Vec<_>>();
    assert!(
        migrations.len() > 1,
        "upgrade coverage needs two migrations"
    );
    let previous_release = Migrator {
        migrations: Cow::Owned(migrations[..migrations.len() - 1].to_vec()),
        ignore_missing: false,
        locking: true,
        no_tx: false,
    };
    previous_release
        .run(&upgrade.pool)
        .await
        .expect("previous release migrations failed");

    let restaurant = Uuid::now_v7();
    let user = Uuid::now_v7();
    let item = Uuid::now_v7();
    let loss = Uuid::now_v7();
    sqlx::query("INSERT INTO users(id,auth_subject) VALUES($1,'upgrade-owner')")
        .bind(user)
        .execute(&upgrade.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO restaurants(id,name,city,service_style)
         VALUES($1,'Upgrade Cafe','Dallas','cafe_bakery')",
    )
    .bind(restaurant)
    .execute(&upgrade.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO restaurant_memberships(restaurant_id,user_id,role)
         VALUES($1,$2,'owner')",
    )
    .bind(restaurant)
    .bind(user)
    .execute(&upgrade.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO inventory_items(id,restaurant_id,name,count_unit)
         VALUES($1,$2,'Flour','kg')",
    )
    .bind(item)
    .bind(restaurant)
    .execute(&upgrade.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO loss_events(
             id,restaurant_id,inventory_item_id,created_by,event_type,
             inventory_item_name,count_unit,quantity,reason)
         VALUES($1,$2,$3,$4,'waste','Flour','kg',1.234567,'spoilage')",
    )
    .bind(loss)
    .bind(restaurant)
    .bind(item)
    .bind(user)
    .execute(&upgrade.pool)
    .await
    .expect("previous-release fixture could not be seeded");

    MIGRATOR
        .run(&upgrade.pool)
        .await
        .expect("upgrade from the previous release failed");
    let quantity =
        sqlx::query_scalar::<_, String>("SELECT quantity::text FROM loss_events WHERE id=$1")
            .bind(loss)
            .fetch_one(&upgrade.pool)
            .await
            .unwrap();
    assert_eq!(quantity, "1.234567", "upgrade changed an exact decimal");

    let invalid = sqlx::query(
        "INSERT INTO loss_events(
             id,restaurant_id,inventory_item_id,created_by,event_type,
             inventory_item_name,count_unit,quantity,reason)
         VALUES($1,$2,$3,$4,'waste','Flour','kg','NaN'::numeric,'spoilage')",
    )
    .bind(Uuid::now_v7())
    .bind(restaurant)
    .bind(item)
    .bind(user)
    .execute(&upgrade.pool)
    .await
    .unwrap_err();
    assert_eq!(
        invalid.as_database_error().and_then(|error| error.code()),
        Some(Cow::Borrowed("23514")),
        "latest constraint must reject numeric NaN"
    );
    upgrade.drop().await;
}

#[tokio::test]
#[ignore = "run by scripts/release-gate.sh with disposable PostgreSQL databases"]
async fn api_enforces_tenant_and_role_boundaries() {
    let fixture = ApiFixture::create("tenant_roles").await;
    let owner_a = fixture.token("owner-a");
    let manager_a = fixture.token("manager-a");
    let staff_a = fixture.token("staff-a");
    let owner_b = fixture.token("owner-b");

    for token in [&manager_a, &staff_a] {
        let forbidden = request(
            fixture.app.clone(),
            Some(token),
            Method::POST,
            "/v1/settings/invitations",
            Some(json!({"email":"new.person@example.com","role":"staff"})),
        )
        .await;
        assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    }
    let sent = request(
        fixture.app.clone(),
        Some(&owner_a),
        Method::POST,
        "/v1/settings/invitations",
        Some(json!({"email":" Invitee@Example.com ","role":"manager"})),
    )
    .await;
    assert_eq!(sent.status, StatusCode::OK);
    assert_eq!(sent.body["invitations"][0]["email"], "invitee@example.com");
    let invitation_id =
        Uuid::parse_str(sent.body["invitations"][0]["id"].as_str().unwrap()).unwrap();
    let duplicate = request(
        fixture.app.clone(),
        Some(&owner_b),
        Method::POST,
        "/v1/settings/invitations",
        Some(json!({"email":"invitee@example.com","role":"staff"})),
    )
    .await;
    assert_eq!(duplicate.status, StatusCode::CONFLICT);
    let manager_settings = request(
        fixture.app.clone(),
        Some(&manager_a),
        Method::GET,
        "/v1/settings",
        None,
    )
    .await;
    assert!(manager_settings.body["invitations"].is_null());

    let provider_id: String =
        sqlx::query_scalar("SELECT workos_invitation_id FROM team_invitations WHERE id=$1")
            .bind(invitation_id)
            .fetch_one(&fixture.database.pool)
            .await
            .unwrap();
    let cross_tenant_revoke = request(
        fixture.app.clone(),
        Some(&owner_b),
        Method::DELETE,
        &format!("/v1/settings/invitations/{invitation_id}"),
        None,
    )
    .await;
    assert_eq!(cross_tenant_revoke.status, StatusCode::NOT_FOUND);
    let resent = request(
        fixture.app.clone(),
        Some(&owner_a),
        Method::POST,
        &format!("/v1/settings/invitations/{invitation_id}/resend"),
        None,
    )
    .await;
    assert_eq!(resent.status, StatusCode::OK);
    fixture.workos.users.lock().await.insert(
        "wrong-subject".into(),
        crate::workos::User {
            id: "wrong-subject".into(),
            email: "invitee@example.com".into(),
            email_verified: true,
            first_name: None,
            last_name: None,
        },
    );
    let mismatched = request(
        fixture.app.clone(),
        Some(&fixture.token("wrong-subject")),
        Method::GET,
        "/v1/me",
        None,
    )
    .await;
    assert_eq!(mismatched.status, StatusCode::OK);
    assert!(mismatched.body["restaurant"].is_null());
    fixture.workos.users.lock().await.insert(
        "invitee-subject".into(),
        crate::workos::User {
            id: "invitee-subject".into(),
            email: "invitee@example.com".into(),
            email_verified: true,
            first_name: Some("Invited".into()),
            last_name: Some("Manager".into()),
        },
    );
    {
        let mut invitations = fixture.workos.invitations.lock().await;
        let provider = invitations.get_mut(&provider_id).unwrap();
        provider.state = "accepted".into();
        provider.accepted_user_id = Some("invitee-subject".into());
        provider.expires_at = chrono::Utc::now() - chrono::Duration::hours(1);
    }
    let accepted = request(
        fixture.app.clone(),
        Some(&fixture.token("invitee-subject")),
        Method::GET,
        "/v1/me",
        None,
    )
    .await;
    assert_eq!(accepted.status, StatusCode::OK);
    assert_eq!(accepted.body["restaurant"]["role"], "manager");
    assert_eq!(
        accepted.body["restaurant"]["id"],
        fixture.ids.restaurant_a.to_string()
    );

    let unauthenticated = request(
        fixture.app.clone(),
        None,
        Method::GET,
        "/v1/inventory-items",
        None,
    )
    .await;
    assert_eq!(unauthenticated.status, StatusCode::UNAUTHORIZED);

    for (token, expected_role) in [
        (&owner_a, "owner"),
        (&manager_a, "manager"),
        (&staff_a, "staff"),
    ] {
        let response = request(
            fixture.app.clone(),
            Some(token),
            Method::GET,
            "/v1/me",
            None,
        )
        .await;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body["restaurant"]["role"], expected_role);
        assert_eq!(
            response.body["restaurant"]["id"],
            fixture.ids.restaurant_a.to_string()
        );
    }

    let list_a = request(
        fixture.app.clone(),
        Some(&staff_a),
        Method::GET,
        "/v1/inventory-items",
        None,
    )
    .await;
    assert_eq!(list_a.status, StatusCode::OK);
    assert_eq!(list_a.body.as_array().unwrap().len(), 1);
    assert_eq!(list_a.body[0]["name"], "Tenant A Tomatoes");
    let list_b = request(
        fixture.app.clone(),
        Some(&owner_b),
        Method::GET,
        "/v1/inventory-items",
        None,
    )
    .await;
    assert_eq!(list_b.body.as_array().unwrap().len(), 1);
    assert_eq!(list_b.body[0]["name"], "Tenant B Avocados");

    for (token, name) in [(&owner_a, "Owner item"), (&manager_a, "Manager item")] {
        let response = request(
            fixture.app.clone(),
            Some(token),
            Method::POST,
            "/v1/inventory-items",
            Some(json!({
                "name": name,
                "category": "Produce",
                "countUnit": "each",
                "parLevel": "2.000001"
            })),
        )
        .await;
        assert_eq!(response.status, StatusCode::CREATED);
    }
    let staff_catalog_write = request(
        fixture.app.clone(),
        Some(&staff_a),
        Method::POST,
        "/v1/inventory-items",
        Some(json!({
            "name": "Staff item",
            "category": null,
            "countUnit": "each",
            "parLevel": null
        })),
    )
    .await;
    assert_eq!(staff_catalog_write.status, StatusCode::FORBIDDEN);

    let cross_tenant_update = request(
        fixture.app.clone(),
        Some(&manager_a),
        Method::PUT,
        &format!("/v1/inventory-items/{}", fixture.ids.inventory_b),
        Some(json!({
            "name": "Stolen item",
            "category": null,
            "countUnit": "lb",
            "parLevel": null
        })),
    )
    .await;
    assert_eq!(cross_tenant_update.status, StatusCode::NOT_FOUND);

    for token in [&owner_a, &manager_a] {
        let invoices = request(
            fixture.app.clone(),
            Some(token),
            Method::GET,
            "/v1/invoices",
            None,
        )
        .await;
        assert_eq!(invoices.status, StatusCode::OK);
    }
    assert_eq!(
        request(
            fixture.app.clone(),
            Some(&staff_a),
            Method::GET,
            "/v1/invoices",
            None,
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );

    assert_eq!(
        request(
            fixture.app.clone(),
            Some(&owner_a),
            Method::GET,
            "/v1/weekly-brief",
            None,
        )
        .await
        .status,
        StatusCode::OK
    );
    for token in [&manager_a, &staff_a] {
        assert_eq!(
            request(
                fixture.app.clone(),
                Some(token),
                Method::GET,
                "/v1/weekly-brief",
                None,
            )
            .await
            .status,
            StatusCode::FORBIDDEN
        );
    }

    assert_eq!(
        request(
            fixture.app.clone(),
            Some(&manager_a),
            Method::GET,
            "/v1/menu-items",
            None,
        )
        .await
        .status,
        StatusCode::OK
    );
    assert_eq!(
        request(
            fixture.app.clone(),
            Some(&staff_a),
            Method::GET,
            "/v1/menu-items",
            None,
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );

    let staff_loss = request(
        fixture.app.clone(),
        Some(&staff_a),
        Method::POST,
        "/v1/loss-events",
        Some(json!({
            "eventType": "waste",
            "inventoryItemId": fixture.ids.inventory_a,
            "quantity": "0.000001",
            "severity": null,
            "reason": "spoilage",
            "note": "Trim loss"
        })),
    )
    .await;
    assert_eq!(staff_loss.status, StatusCode::CREATED);
    assert_eq!(staff_loss.body["quantity"], "0.000001");
    let tenant_b_losses = request(
        fixture.app.clone(),
        Some(&owner_b),
        Method::GET,
        "/v1/loss-events",
        None,
    )
    .await;
    assert_eq!(tenant_b_losses.status, StatusCode::OK);
    assert!(tenant_b_losses.body.as_array().unwrap().is_empty());

    fixture.drop().await;
}

#[tokio::test]
#[ignore = "run by scripts/release-gate.sh with disposable PostgreSQL databases"]
async fn exact_decimals_and_concurrent_replays_remain_stable() {
    let fixture = ApiFixture::create("decimal_replay").await;
    let owner = fixture.token("owner-a");
    let staff = fixture.token("staff-a");

    let menu = request(
        fixture.app.clone(),
        Some(&owner),
        Method::POST,
        "/v1/menu-items",
        Some(json!({
            "name": "Exact taco",
            "category": "Tacos",
            "sellingPrice": "12.3400",
            "currency": "USD"
        })),
    )
    .await;
    assert_eq!(menu.status, StatusCode::CREATED);
    assert_decimal_json(&menu.body["sellingPrice"], "12.3400");
    let menu_id = Uuid::parse_str(menu.body["id"].as_str().unwrap()).unwrap();

    let day_uri = "/v1/sales-days/2026-07-22";
    let create_sales = json!({
        "expectedRevision": null,
        "lines": [{
            "menuItemId": menu_id,
            "quantity": "0.123456",
            "reportedNetSales": "10.0100"
        }]
    });
    let left = request(
        fixture.app.clone(),
        Some(&owner),
        Method::PUT,
        day_uri,
        Some(create_sales.clone()),
    );
    let right = request(
        fixture.app.clone(),
        Some(&owner),
        Method::PUT,
        day_uri,
        Some(create_sales.clone()),
    );
    let (left, right) = tokio::join!(left, right);
    for response in [&left, &right] {
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body["revision"], 1);
        assert_decimal_json(&response.body["lines"][0]["quantity"], "0.123456");
        assert_decimal_json(&response.body["lines"][0]["reportedNetSales"], "10.0100");
    }
    let stored_days = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sales_days WHERE restaurant_id=$1 AND business_date='2026-07-22'",
    )
    .bind(fixture.ids.restaurant_a)
    .fetch_one(&fixture.database.pool)
    .await
    .unwrap();
    assert_eq!(stored_days, 1, "concurrent create duplicated a sales day");

    let correction = json!({
        "expectedRevision": 1,
        "lines": [{
            "menuItemId": menu_id,
            "quantity": "0.123457",
            "reportedNetSales": "10.0101"
        }]
    });
    let corrected = request(
        fixture.app.clone(),
        Some(&owner),
        Method::PUT,
        day_uri,
        Some(correction.clone()),
    )
    .await;
    assert_eq!(corrected.status, StatusCode::OK);
    assert_eq!(corrected.body["revision"], 2);
    let retry = request(
        fixture.app.clone(),
        Some(&owner),
        Method::PUT,
        day_uri,
        Some(correction),
    )
    .await;
    assert_eq!(retry.status, StatusCode::OK);
    assert_eq!(retry.body["revision"], 2, "retry incremented the revision");

    let stale = request(
        fixture.app.clone(),
        Some(&owner),
        Method::PUT,
        day_uri,
        Some(json!({
            "expectedRevision": 1,
            "lines": [{
                "menuItemId": menu_id,
                "quantity": "1.000000",
                "reportedNetSales": "11.0000"
            }]
        })),
    )
    .await;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    let exact = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT line.quantity::text,line.reported_net_sales::text,day.revision
         FROM sales_days day JOIN sales_lines line ON line.sales_day_id=day.id
         WHERE day.restaurant_id=$1 AND day.business_date='2026-07-22'",
    )
    .bind(fixture.ids.restaurant_a)
    .fetch_one(&fixture.database.pool)
    .await
    .unwrap();
    assert_eq!(exact, ("0.123457".into(), "10.0101".into(), 2));

    let tenant_b_menu = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO menu_items(id,restaurant_id,name,selling_price,currency)
         VALUES($1,$2,'Other tenant item',9.9900,'USD')",
    )
    .bind(tenant_b_menu)
    .bind(fixture.ids.restaurant_b)
    .execute(&fixture.database.pool)
    .await
    .unwrap();
    let cross_tenant_sales = request(
        fixture.app.clone(),
        Some(&owner),
        Method::PUT,
        "/v1/sales-days/2026-07-23",
        Some(json!({
            "expectedRevision": null,
            "lines": [{
                "menuItemId": tenant_b_menu,
                "quantity": "1",
                "reportedNetSales": null
            }]
        })),
    )
    .await;
    assert_eq!(cross_tenant_sales.status, StatusCode::UNPROCESSABLE_ENTITY);
    let staff_sales = request(
        fixture.app.clone(),
        Some(&staff),
        Method::PUT,
        "/v1/sales-days/2026-07-23",
        Some(create_sales),
    )
    .await;
    assert_eq!(staff_sales.status, StatusCode::OK);

    let invoice = Uuid::now_v7();
    let source_line = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO invoices(
             id,restaurant_id,uploaded_by,supplier_name,invoice_date,original_filename,
             content_type,size_bytes,object_key,status)
         VALUES($1,$2,$3,'Exact Supplier','2026-07-20','exact.pdf',
                'application/pdf',8,$4,'ready')",
    )
    .bind(invoice)
    .bind(fixture.ids.restaurant_a)
    .bind(fixture.ids.owner_a)
    .bind(format!("release-tests/{invoice}.pdf"))
    .execute(&fixture.database.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO invoice_extractions(
             invoice_id,provider,model_id,raw_provider_json,supplier_name,invoice_number,
             invoice_date,currency,total)
         VALUES($1,'test','test-model','{}'::jsonb,'Exact Supplier','INV-1',
                '2026-07-20','USD',3.0750)",
    )
    .bind(invoice)
    .execute(&fixture.database.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO invoice_line_items(
             id,invoice_id,position,description,quantity,unit,unit_price,line_total,
             comparison_key,comparison_unit)
         VALUES($1,$2,0,'Tomatoes',2.500000,'case',1.2300,3.0750,
                'description:tomatoes','case')",
    )
    .bind(source_line)
    .bind(invoice)
    .execute(&fixture.database.pool)
    .await
    .unwrap();
    let receipt_body = json!({
        "resolutions": [{"lineId": source_line, "action": "ignore"}]
    });
    let receipt_uri = format!("/v1/invoices/{invoice}/purchase-receipt");
    let first = request(
        fixture.app.clone(),
        Some(&owner),
        Method::PUT,
        &receipt_uri,
        Some(receipt_body.clone()),
    );
    let second = request(
        fixture.app.clone(),
        Some(&owner),
        Method::PUT,
        &receipt_uri,
        Some(receipt_body),
    );
    let (first, second) = tokio::join!(first, second);
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(second.status, StatusCode::OK);
    let mut replay_flags = [
        first.body["alreadyRecorded"].as_bool().unwrap(),
        second.body["alreadyRecorded"].as_bool().unwrap(),
    ];
    replay_flags.sort_unstable();
    assert_eq!(replay_flags, [false, true]);
    let saved_receipt = sqlx::query_as::<_, (i64, String, String, String)>(
        "SELECT COUNT(*) OVER()::bigint,purchase_quantity::text,
                unit_price::text,line_total::text
         FROM purchase_receipt_lines WHERE invoice_id=$1",
    )
    .bind(invoice)
    .fetch_one(&fixture.database.pool)
    .await
    .unwrap();
    assert_eq!(
        saved_receipt,
        (1, "2.500000".into(), "1.2300".into(), "3.0750".into())
    );

    let start_owner = request(
        fixture.app.clone(),
        Some(&owner),
        Method::POST,
        "/v1/inventory-counts",
        None,
    );
    let start_staff = request(
        fixture.app.clone(),
        Some(&staff),
        Method::POST,
        "/v1/inventory-counts",
        None,
    );
    let (start_owner, start_staff) = tokio::join!(start_owner, start_staff);
    let mut count_statuses = [start_owner.status.as_u16(), start_staff.status.as_u16()];
    count_statuses.sort_unstable();
    assert_eq!(
        count_statuses,
        [StatusCode::CREATED.as_u16(), StatusCode::CONFLICT.as_u16()]
    );
    let drafts = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM inventory_count_sessions
         WHERE restaurant_id=$1 AND status='draft'",
    )
    .bind(fixture.ids.restaurant_a)
    .fetch_one(&fixture.database.pool)
    .await
    .unwrap();
    assert_eq!(drafts, 1);

    fixture.drop().await;
}
