use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{ApiError, AppState, authenticated_subject};

#[derive(sqlx::FromRow)]
struct SettingsContext {
    id: Uuid,
    name: String,
    city: String,
    service_style: String,
    timezone: String,
    role: String,
    user_id: Uuid,
}

#[derive(sqlx::FromRow)]
struct Actor {
    restaurant_id: Uuid,
    user_id: Uuid,
    role: String,
}

#[derive(sqlx::FromRow)]
struct TargetMember {
    user_id: Uuid,
    role: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RestaurantSettings {
    id: Uuid,
    name: String,
    city: String,
    service_style: String,
    timezone: String,
    role: String,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct TeamMember {
    id: Uuid,
    email: Option<String>,
    display_name: Option<String>,
    role: String,
    is_current_user: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsResponse {
    restaurant: RestaurantSettings,
    team: Option<Vec<TeamMember>>,
    invitations: Option<Vec<TeamInvitation>>,
    invitations_enabled: bool,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct TeamInvitation {
    id: Uuid,
    email: String,
    role: String,
    expires_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateInvitation {
    email: String,
    role: String,
}

#[derive(sqlx::FromRow)]
struct InvitationTarget {
    workos_invitation_id: String,
    email: String,
}

#[derive(sqlx::FromRow)]
struct ActiveInvitation {
    id: Uuid,
    email: String,
    workos_invitation_id: Option<String>,
    invited_by_subject: String,
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UpdateRestaurant {
    name: String,
    city: String,
    service_style: String,
    timezone: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateRole {
    role: String,
}

pub(crate) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SettingsResponse>, ApiError> {
    let subject = authenticated_subject(&state, &headers).await?;
    Ok(Json(load_settings(&state, &subject).await?))
}

pub(crate) async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<UpdateRestaurant>,
) -> Result<Json<SettingsResponse>, ApiError> {
    let subject = authenticated_subject(&state, &headers).await?;
    let input = input.validated()?;
    let mut transaction = state.pool.begin().await.map_err(database_error)?;
    let actor = lock_actor(&mut transaction, &subject).await?;
    require_owner(&actor)?;

    sqlx::query(
        "UPDATE restaurants
         SET name=$1,city=$2,service_style=$3,timezone=$4,updated_at=NOW()
         WHERE id=$5",
    )
    .bind(&input.name)
    .bind(&input.city)
    .bind(&input.service_style)
    .bind(&input.timezone)
    .bind(actor.restaurant_id)
    .execute(&mut *transaction)
    .await
    .map_err(database_error)?;
    transaction.commit().await.map_err(database_error)?;

    tracing::info!(
        restaurant_id = %actor.restaurant_id,
        actor_user_id = %actor.user_id,
        "restaurant settings updated"
    );
    Ok(Json(load_settings(&state, &subject).await?))
}

pub(crate) async fn update_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Json(input): Json<UpdateRole>,
) -> Result<Json<SettingsResponse>, ApiError> {
    let subject = authenticated_subject(&state, &headers).await?;
    let role = input.validated()?;
    let mut transaction = state.pool.begin().await.map_err(database_error)?;
    let actor = lock_actor(&mut transaction, &subject).await?;
    require_owner(&actor)?;
    let target = lock_target(&mut transaction, actor.restaurant_id, user_id).await?;
    let owner_count = owner_count(&mut transaction, actor.restaurant_id).await?;
    validate_role_change(&actor, &target, &role, owner_count)?;

    if target.role != role {
        sqlx::query(
            "UPDATE restaurant_memberships SET role=$1,updated_at=NOW()
             WHERE restaurant_id=$2 AND user_id=$3",
        )
        .bind(&role)
        .bind(actor.restaurant_id)
        .bind(target.user_id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    }
    transaction.commit().await.map_err(database_error)?;

    tracing::info!(
        restaurant_id = %actor.restaurant_id,
        actor_user_id = %actor.user_id,
        target_user_id = %target.user_id,
        previous_role = %target.role,
        new_role = %role,
        "team member role updated"
    );
    Ok(Json(load_settings(&state, &subject).await?))
}

pub(crate) async fn remove_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> Result<Json<SettingsResponse>, ApiError> {
    let subject = authenticated_subject(&state, &headers).await?;
    let mut transaction = state.pool.begin().await.map_err(database_error)?;
    let actor = lock_actor(&mut transaction, &subject).await?;
    require_owner(&actor)?;
    let target = lock_target(&mut transaction, actor.restaurant_id, user_id).await?;
    let owner_count = owner_count(&mut transaction, actor.restaurant_id).await?;
    validate_removal(&actor, &target, owner_count)?;

    sqlx::query("DELETE FROM restaurant_memberships WHERE restaurant_id=$1 AND user_id=$2")
        .bind(actor.restaurant_id)
        .bind(target.user_id)
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;
    transaction.commit().await.map_err(database_error)?;

    tracing::info!(
        restaurant_id = %actor.restaurant_id,
        actor_user_id = %actor.user_id,
        target_user_id = %target.user_id,
        removed_role = %target.role,
        "team member access removed"
    );
    Ok(Json(load_settings(&state, &subject).await?))
}

pub(crate) async fn invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CreateInvitation>,
) -> Result<Json<SettingsResponse>, ApiError> {
    let subject = authenticated_subject(&state, &headers).await?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let actor = lock_actor(&mut tx, &subject).await?;
    require_owner(&actor)?;
    if !state.workos.enabled() {
        return Err(invitations_unavailable());
    }
    let (email, role) = input.validated()?;
    tx.commit().await.map_err(database_error)?;
    reconcile_active_email(&state, &email).await?;

    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let actor = lock_actor(&mut tx, &subject).await?;
    require_owner(&actor)?;
    let member_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM users u JOIN restaurant_memberships m ON m.user_id=u.id WHERE LOWER(u.email)=LOWER($1))",
    ).bind(&email).fetch_one(&mut *tx).await.map_err(database_error)?;
    if member_exists {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "That email already belongs to an active team member.",
        ));
    }
    let id = Uuid::now_v7();
    if let Err(error) = sqlx::query("INSERT INTO team_invitations(id,restaurant_id,email,role,state,invited_by,inviter_auth_subject) VALUES($1,$2,$3,$4,'creating',$5,$6)")
        .bind(id).bind(actor.restaurant_id).bind(&email).bind(&role).bind(actor.user_id).bind(&subject).execute(&mut *tx).await {
        if error.as_database_error().is_some_and(|e| e.constraint() == Some("team_invitations_one_active_email_idx")) {
            return Err(ApiError(StatusCode::CONFLICT, "An active invitation already exists for that email."));
        }
        return Err(database_error(error));
    }
    tx.commit().await.map_err(database_error)?;
    let provider = match state.workos.send(&email, &subject).await {
        Ok(value) => value,
        Err(_) => {
            reconcile_active_email(&state, &email).await?;
            let recovered = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM team_invitations WHERE id=$1 AND state='pending')",
            )
            .bind(id)
            .fetch_one(&state.pool)
            .await
            .map_err(database_error)?;
            if recovered {
                return Ok(Json(load_settings(&state, &subject).await?));
            }
            return Err(provider_error());
        }
    };
    if normalize_email(&provider.email).as_deref() != Some(email.as_str())
        || provider.state != "pending"
    {
        sqlx::query("UPDATE team_invitations SET state='failed',updated_at=NOW() WHERE id=$1")
            .bind(id)
            .execute(&state.pool)
            .await
            .map_err(database_error)?;
        return Err(provider_error());
    }
    sqlx::query("UPDATE team_invitations SET workos_invitation_id=$1,provider_expires_at=$2,state='pending',updated_at=NOW() WHERE id=$3 AND state='creating'")
        .bind(provider.id).bind(provider.expires_at).bind(id).execute(&state.pool).await.map_err(database_error)?;
    Ok(Json(load_settings(&state, &subject).await?))
}

async fn reconcile_active_email(state: &AppState, email: &str) -> Result<(), ApiError> {
    let local = sqlx::query_as::<_, ActiveInvitation>(
        "SELECT invitation.id,invitation.email,invitation.workos_invitation_id,
                invitation.inviter_auth_subject AS invited_by_subject,invitation.created_at
         FROM team_invitations invitation
         WHERE invitation.email=$1 AND invitation.state IN ('creating','pending')",
    )
    .bind(email)
    .fetch_optional(&state.pool)
    .await
    .map_err(database_error)?;
    let Some(local) = local else {
        return Ok(());
    };

    let provider = if let Some(provider_id) = &local.workos_invitation_id {
        Some(
            state
                .workos
                .invitation(provider_id)
                .await
                .map_err(|_| provider_error())?,
        )
    } else {
        let earliest = local.created_at - chrono::Duration::minutes(1);
        let mut matches = state
            .workos
            .invitations_for_email(email)
            .await
            .map_err(|_| provider_error())?
            .into_iter()
            .filter(|candidate| {
                normalize_email(&candidate.email).as_deref() == Some(local.email.as_str())
                    && candidate.inviter_user_id.as_deref()
                        == Some(local.invited_by_subject.as_str())
                    && candidate.created_at >= earliest
            });
        let candidate = matches.next();
        if matches.next().is_some() {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "More than one invitation email needs support review before trying again.",
            ));
        }
        if candidate.is_none() && local.created_at < Utc::now() - chrono::Duration::minutes(1) {
            sqlx::query(
                "UPDATE team_invitations SET state='failed',updated_at=NOW()
                 WHERE id=$1 AND state='creating'",
            )
            .bind(local.id)
            .execute(&state.pool)
            .await
            .map_err(database_error)?;
        }
        candidate
    };

    let Some(provider) = provider else {
        return Ok(());
    };
    if normalize_email(&provider.email).as_deref() != Some(local.email.as_str()) {
        return Err(provider_error());
    }
    match provider.state.as_str() {
        "pending" | "accepted" => {
            sqlx::query(
                "UPDATE team_invitations
                 SET workos_invitation_id=$1,provider_expires_at=$2,state='pending',updated_at=NOW()
                 WHERE id=$3 AND state IN ('creating','pending')",
            )
            .bind(provider.id)
            .bind(provider.expires_at)
            .bind(local.id)
            .execute(&state.pool)
            .await
            .map_err(database_error)?;
        }
        "expired" => {
            sqlx::query(
                "UPDATE team_invitations
                 SET workos_invitation_id=$1,provider_expires_at=$2,state='expired',updated_at=NOW()
                 WHERE id=$3 AND state IN ('creating','pending')",
            )
            .bind(provider.id)
            .bind(provider.expires_at)
            .bind(local.id)
            .execute(&state.pool)
            .await
            .map_err(database_error)?;
        }
        "revoked" => {
            sqlx::query(
                "UPDATE team_invitations
                 SET workos_invitation_id=$1,provider_expires_at=$2,state='revoked',revoked_at=NOW(),updated_at=NOW()
                 WHERE id=$3 AND state IN ('creating','pending')",
            )
            .bind(provider.id)
            .bind(provider.expires_at)
            .bind(local.id)
            .execute(&state.pool)
            .await
            .map_err(database_error)?;
        }
        _ => return Err(provider_error()),
    }
    Ok(())
}

pub(crate) async fn resend_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<SettingsResponse>, ApiError> {
    let subject = authenticated_subject(&state, &headers).await?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let actor = lock_actor(&mut tx, &subject).await?;
    require_owner(&actor)?;
    if !state.workos.enabled() {
        return Err(invitations_unavailable());
    }
    let target = invitation_target(&mut tx, actor.restaurant_id, id).await?;
    tx.commit().await.map_err(database_error)?;
    let provider = state
        .workos
        .resend(&target.workos_invitation_id)
        .await
        .map_err(|_| provider_error())?;
    if normalize_email(&provider.email).as_deref() != Some(target.email.as_str())
        || provider.state != "pending"
    {
        return Err(provider_error());
    }
    sqlx::query("UPDATE team_invitations SET provider_expires_at=$1,updated_at=NOW() WHERE id=$2 AND restaurant_id=$3 AND state='pending'")
        .bind(provider.expires_at).bind(id).bind(actor.restaurant_id).execute(&state.pool).await.map_err(database_error)?;
    Ok(Json(load_settings(&state, &subject).await?))
}

pub(crate) async fn revoke_invitation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<SettingsResponse>, ApiError> {
    let subject = authenticated_subject(&state, &headers).await?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let actor = lock_actor(&mut tx, &subject).await?;
    require_owner(&actor)?;
    if !state.workos.enabled() {
        return Err(invitations_unavailable());
    }
    let target = invitation_target(&mut tx, actor.restaurant_id, id).await?;
    tx.commit().await.map_err(database_error)?;
    let provider = state
        .workos
        .revoke(&target.workos_invitation_id)
        .await
        .map_err(|_| provider_error())?;
    if normalize_email(&provider.email).as_deref() != Some(target.email.as_str())
        || provider.state != "revoked"
    {
        return Err(provider_error());
    }
    sqlx::query("UPDATE team_invitations SET state='revoked',revoked_at=NOW(),updated_at=NOW() WHERE id=$1 AND restaurant_id=$2 AND state='pending'")
        .bind(id).bind(actor.restaurant_id).execute(&state.pool).await.map_err(database_error)?;
    Ok(Json(load_settings(&state, &subject).await?))
}

async fn invitation_target(
    tx: &mut Transaction<'_, Postgres>,
    restaurant_id: Uuid,
    id: Uuid,
) -> Result<InvitationTarget, ApiError> {
    sqlx::query_as("SELECT workos_invitation_id,email FROM team_invitations WHERE id=$1 AND restaurant_id=$2 AND state='pending' AND provider_expires_at>NOW() FOR UPDATE")
        .bind(id).bind(restaurant_id).fetch_optional(&mut **tx).await.map_err(database_error)?.ok_or(ApiError(StatusCode::NOT_FOUND, "Active invitation not found."))
}

async fn load_settings(state: &AppState, subject: &str) -> Result<SettingsResponse, ApiError> {
    let context = sqlx::query_as::<_, SettingsContext>(
        "SELECT r.id,r.name,r.city,r.service_style,r.timezone,m.role,u.id AS user_id
         FROM users u
         JOIN restaurant_memberships m ON m.user_id=u.id
         JOIN restaurants r ON r.id=m.restaurant_id
         WHERE u.auth_subject=$1",
    )
    .bind(subject)
    .fetch_optional(&state.pool)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::FORBIDDEN,
        "A restaurant membership is required.",
    ))?;

    let team = if context.role == "owner" {
        Some(
            sqlx::query_as::<_, TeamMember>(
                "SELECT u.id,u.email,u.display_name,m.role,(u.id=$2) AS is_current_user
                 FROM restaurant_memberships m
                 JOIN users u ON u.id=m.user_id
                 WHERE m.restaurant_id=$1
                 ORDER BY CASE m.role WHEN 'owner' THEN 0 WHEN 'manager' THEN 1 ELSE 2 END,
                          LOWER(COALESCE(NULLIF(u.display_name,''),NULLIF(u.email,''),u.id::text)),u.id",
            )
            .bind(context.id)
            .bind(context.user_id)
            .fetch_all(&state.pool)
            .await
            .map_err(database_error)?,
        )
    } else {
        None
    };
    let invitations = if context.role == "owner" {
        Some(sqlx::query_as::<_, TeamInvitation>("SELECT id,email,role,provider_expires_at AS expires_at FROM team_invitations WHERE restaurant_id=$1 AND state='pending' AND provider_expires_at>NOW() ORDER BY created_at,id")
            .bind(context.id).fetch_all(&state.pool).await.map_err(database_error)?)
    } else {
        None
    };

    Ok(SettingsResponse {
        restaurant: RestaurantSettings {
            id: context.id,
            name: context.name,
            city: context.city,
            service_style: context.service_style,
            timezone: context.timezone,
            role: context.role,
        },
        team,
        invitations,
        invitations_enabled: state.workos.enabled(),
    })
}

async fn lock_actor(
    transaction: &mut Transaction<'_, Postgres>,
    subject: &str,
) -> Result<Actor, ApiError> {
    let restaurant_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT r.id FROM users u
         JOIN restaurant_memberships m ON m.user_id=u.id
         JOIN restaurants r ON r.id=m.restaurant_id
         WHERE u.auth_subject=$1 FOR UPDATE OF r",
    )
    .bind(subject)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::FORBIDDEN,
        "A restaurant membership is required.",
    ))?;

    sqlx::query_as::<_, Actor>(
        "SELECT m.restaurant_id,u.id AS user_id,m.role FROM users u
         JOIN restaurant_memberships m ON m.user_id=u.id
         WHERE u.auth_subject=$1 AND m.restaurant_id=$2",
    )
    .bind(subject)
    .bind(restaurant_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::FORBIDDEN,
        "A restaurant membership is required.",
    ))
}

async fn lock_target(
    transaction: &mut Transaction<'_, Postgres>,
    restaurant_id: Uuid,
    user_id: Uuid,
) -> Result<TargetMember, ApiError> {
    sqlx::query_as::<_, TargetMember>(
        "SELECT user_id,role FROM restaurant_memberships
         WHERE restaurant_id=$1 AND user_id=$2 FOR UPDATE",
    )
    .bind(restaurant_id)
    .bind(user_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(StatusCode::NOT_FOUND, "Team member not found."))
}

async fn owner_count(
    transaction: &mut Transaction<'_, Postgres>,
    restaurant_id: Uuid,
) -> Result<i64, ApiError> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM restaurant_memberships WHERE restaurant_id=$1 AND role='owner'",
    )
    .bind(restaurant_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(database_error)
}

fn require_owner(actor: &Actor) -> Result<(), ApiError> {
    if actor.role == "owner" {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::FORBIDDEN,
            "Only restaurant owners can make this change.",
        ))
    }
}

fn validate_role_change(
    actor: &Actor,
    target: &TargetMember,
    new_role: &str,
    owner_count: i64,
) -> Result<(), ApiError> {
    require_owner(actor)?;
    if actor.user_id == target.user_id && role_rank(new_role) > role_rank(&actor.role) {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "You cannot raise your own access level.",
        ));
    }
    if target.role == "owner" && new_role != "owner" && owner_count <= 1 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Add another owner before changing the last owner's role.",
        ));
    }
    Ok(())
}

fn validate_removal(
    actor: &Actor,
    target: &TargetMember,
    owner_count: i64,
) -> Result<(), ApiError> {
    require_owner(actor)?;
    if actor.user_id == target.user_id {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "You cannot remove your own access. Ask another owner to remove it.",
        ));
    }
    if target.role == "owner" && owner_count <= 1 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "The last owner cannot be removed.",
        ));
    }
    Ok(())
}

fn role_rank(role: &str) -> u8 {
    match role {
        "staff" => 0,
        "manager" => 1,
        "owner" => 2,
        _ => 0,
    }
}

fn database_error(error: sqlx::Error) -> ApiError {
    tracing::error!(%error, "settings database operation failed");
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "We couldn't save your settings. Please try again.",
    )
}

fn invitations_unavailable() -> ApiError {
    ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Team invitations have not been enabled for this environment.",
    )
}
fn provider_error() -> ApiError {
    ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "The invitation email could not be sent. Please try again.",
    )
}

pub(crate) fn normalize_email(value: &str) -> Option<String> {
    let email = value.trim().to_lowercase();
    if email.is_empty()
        || email.len() > 254
        || email.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let (local, domain) = email.split_once('@')?;
    if local.is_empty()
        || domain.is_empty()
        || domain.starts_with('.')
        || domain.ends_with('.')
        || !domain.contains('.')
        || email.matches('@').count() != 1
        || domain
            .split('.')
            .any(|part| part.is_empty() || part.starts_with('-') || part.ends_with('-'))
    {
        return None;
    }
    Some(email)
}

impl UpdateRestaurant {
    fn validated(mut self) -> Result<Self, ApiError> {
        self.name = self.name.trim().to_owned();
        self.city = self.city.trim().to_owned();
        self.service_style = self.service_style.trim().to_owned();
        self.timezone = self.timezone.trim().to_owned();
        if self.name.is_empty() || self.name.chars().count() > 50 {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Restaurant name must be between 1 and 50 characters.",
            ));
        }
        if self.city.is_empty() || self.city.chars().count() > 100 {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "City must be between 1 and 100 characters.",
            ));
        }
        if !matches!(
            self.service_style.as_str(),
            "counter_service" | "full_service" | "fast_casual" | "cafe_bakery" | "bar"
        ) {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Choose a listed service style.",
            ));
        }
        let timezone = self.timezone.parse::<Tz>().map_err(|_| {
            ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Enter a valid IANA timezone, such as America/Chicago.",
            )
        })?;
        self.timezone = timezone.name().to_owned();
        Ok(self)
    }
}

impl UpdateRole {
    fn validated(self) -> Result<String, ApiError> {
        let role = self.role.trim();
        if !matches!(role, "owner" | "manager" | "staff") {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Choose owner, manager, or staff access.",
            ));
        }
        Ok(role.to_owned())
    }
}

impl CreateInvitation {
    fn validated(self) -> Result<(String, String), ApiError> {
        let email = normalize_email(&self.email).ok_or(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Enter a valid email address.",
        ))?;
        let role = self.role.trim().to_lowercase();
        if !matches!(role.as_str(), "manager" | "staff") {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Choose manager or staff access for an invitation.",
            ));
        }
        Ok((email, role))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(timezone: &str) -> UpdateRestaurant {
        UpdateRestaurant {
            name: "  Marigold  ".into(),
            city: " Dallas ".into(),
            service_style: "fast_casual".into(),
            timezone: timezone.into(),
        }
    }

    fn actor(id: Uuid, role: &str) -> Actor {
        Actor {
            restaurant_id: Uuid::nil(),
            user_id: id,
            role: role.into(),
        }
    }

    fn target(id: Uuid, role: &str) -> TargetMember {
        TargetMember {
            user_id: id,
            role: role.into(),
        }
    }

    #[test]
    fn validates_and_canonicalizes_restaurant_settings() {
        let value = settings(" America/Chicago ").validated().unwrap();
        assert_eq!(value.name, "Marigold");
        assert_eq!(value.city, "Dallas");
        assert_eq!(value.timezone, "America/Chicago");
        assert!(settings("America/Dallas").validated().is_err());
        assert!(settings("Not/A_Zone").validated().is_err());
    }

    #[test]
    fn validates_settings_lengths_and_service_style() {
        let mut value = settings("UTC");
        value.name = "".into();
        assert!(value.validated().is_err());
        let mut value = settings("UTC");
        value.city = "x".repeat(101);
        assert!(value.validated().is_err());
        let mut value = settings("UTC");
        value.service_style = "buffet".into();
        assert!(value.validated().is_err());
    }

    #[test]
    fn protects_last_owner_and_rejects_self_escalation() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        assert!(
            validate_role_change(
                &actor(first, "owner"),
                &target(first, "owner"),
                "manager",
                1
            )
            .is_err()
        );
        assert!(
            validate_role_change(
                &actor(first, "owner"),
                &target(first, "owner"),
                "manager",
                2
            )
            .is_ok()
        );
        assert!(
            validate_role_change(
                &actor(first, "manager"),
                &target(first, "manager"),
                "owner",
                1
            )
            .is_err()
        );
        assert!(
            validate_role_change(
                &actor(first, "owner"),
                &target(second, "manager"),
                "owner",
                1
            )
            .is_ok()
        );
    }

    #[test]
    fn removal_requires_an_owner_and_never_allows_self_removal() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        assert!(validate_removal(&actor(first, "owner"), &target(first, "owner"), 2).is_err());
        assert!(validate_removal(&actor(first, "manager"), &target(second, "staff"), 1).is_err());
        assert!(validate_removal(&actor(first, "owner"), &target(second, "owner"), 1).is_err());
        assert!(validate_removal(&actor(first, "owner"), &target(second, "staff"), 1).is_ok());
    }

    #[test]
    fn validates_invitation_email_and_role() {
        let valid = CreateInvitation {
            email: " Person@Example.COM ".into(),
            role: "staff".into(),
        }
        .validated()
        .unwrap();
        assert_eq!(valid, ("person@example.com".into(), "staff".into()));
        for email in [
            "a b@example.com",
            "a@localhost",
            "a@@example.com",
            "a@-bad.com",
        ] {
            assert!(normalize_email(email).is_none());
        }
        assert!(
            CreateInvitation {
                email: "a@example.com".into(),
                role: "owner".into()
            }
            .validated()
            .is_err()
        );
    }
}
