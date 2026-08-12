use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ApiError, AppState, authenticated_subject, database_error,
    invoices::{membership, strict_decimal},
};

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MenuItem {
    id: Uuid,
    name: String,
    category: Option<String>,
    selling_price: String,
    currency: String,
    active: bool,
    ingredient_count: i64,
    setup_choice: Option<String>,
    cost_state: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateMenuItem {
    name: String,
    category: Option<String>,
    selling_price: String,
    currency: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IngredientSetupChoice {
    choice: String,
}

pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<MenuItem>>, ApiError> {
    let restaurant_id = manager_restaurant_id(&state, &headers).await?;
    let mut items = sqlx::query_as::<_, MenuItem>(
        "SELECT item.id,item.name,item.category,item.selling_price::text AS selling_price,
                item.currency,item.active,COUNT(ingredient.id)::bigint AS ingredient_count,
                preference.choice AS setup_choice,'recipe_not_configured'::text AS cost_state
         FROM menu_items item
         LEFT JOIN menu_item_ingredients ingredient ON ingredient.menu_item_id=item.id
           AND ingredient.restaurant_id=item.restaurant_id
         LEFT JOIN menu_ingredient_setup_preferences preference
           ON preference.menu_item_id=item.id AND preference.restaurant_id=item.restaurant_id
         WHERE item.restaurant_id=$1 AND item.active
         GROUP BY item.id,item.name,item.category,item.selling_price,item.currency,item.active,preference.choice
         ORDER BY item.category NULLS LAST,item.name,item.id",
    )
    .bind(restaurant_id)
    .fetch_all(&state.pool)
    .await
    .map_err(crate::database_error)?;
    let ids = items.iter().map(|item| item.id).collect::<Vec<_>>();
    let states = crate::costing::load_cost_states(&state.pool, restaurant_id, &ids).await?;
    for item in &mut items {
        item.cost_state = states
            .get(&item.id)
            .cloned()
            .unwrap_or_else(|| "recipe_not_configured".into());
    }
    Ok(Json(items))
}

pub(crate) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CreateMenuItem>,
) -> Result<(StatusCode, Json<MenuItem>), ApiError> {
    let restaurant_id = membership(&state, &headers).await?.restaurant_id;
    let input = input.validated()?;
    let item = sqlx::query_as::<_, MenuItem>(
        "INSERT INTO menu_items (id,restaurant_id,name,category,selling_price,currency)
         VALUES ($1,$2,$3,$4,$5,$6)
         RETURNING id,name,category,selling_price::text AS selling_price,currency,active,
                   0::bigint AS ingredient_count,NULL::text AS setup_choice,
                   'recipe_not_configured'::text AS cost_state",
    )
    .bind(Uuid::now_v7())
    .bind(restaurant_id)
    .bind(input.name)
    .bind(input.category)
    .bind(input.selling_price)
    .bind(input.currency)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| {
        if error
            .as_database_error()
            .and_then(|error| error.code())
            .is_some_and(|code| code == "23505")
        {
            ApiError(
                StatusCode::CONFLICT,
                "That menu item is already in Parline.",
            )
        } else {
            crate::database_error(error)
        }
    })?;
    Ok((StatusCode::CREATED, Json(item)))
}

pub(crate) async fn put_ingredient_setup_choice(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<Uuid>,
    Json(input): Json<IngredientSetupChoice>,
) -> Result<StatusCode, ApiError> {
    let member = membership(&state, &headers).await?;
    let restaurant_id = manager_restaurant_id(&state, &headers).await?;
    if !matches!(input.choice.as_str(), "important" | "later") {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Choose important or later.",
        ));
    }
    let affected = sqlx::query(
        "INSERT INTO menu_ingredient_setup_preferences
         (restaurant_id,menu_item_id,choice,created_by,updated_by)
         SELECT $1,id,$3,$4,$4 FROM menu_items WHERE id=$2 AND restaurant_id=$1 AND active
         ON CONFLICT (restaurant_id,menu_item_id) DO UPDATE SET
           choice=EXCLUDED.choice,updated_by=EXCLUDED.updated_by,updated_at=NOW()",
    )
    .bind(restaurant_id)
    .bind(id)
    .bind(input.choice)
    .bind(member.user_id)
    .execute(&state.pool)
    .await
    .map_err(database_error)?
    .rows_affected();
    if affected == 0 {
        return Err(ApiError(StatusCode::NOT_FOUND, "Menu item not found."));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn manager_restaurant_id(state: &AppState, headers: &HeaderMap) -> Result<Uuid, ApiError> {
    let subject = authenticated_subject(state, headers).await?;
    sqlx::query_scalar(
        "SELECT m.restaurant_id FROM users u
         JOIN restaurant_memberships m ON m.user_id=u.id
         WHERE u.auth_subject=$1 AND m.role IN ('owner','manager')",
    )
    .bind(subject)
    .fetch_optional(&state.pool)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::FORBIDDEN,
        "Owner or manager access is required for menu costing.",
    ))
}

impl CreateMenuItem {
    fn validated(mut self) -> Result<ValidatedMenuItem, ApiError> {
        self.name = self.name.trim().to_owned();
        self.category = self.category.and_then(|category| {
            let category = category.trim();
            (!category.is_empty()).then(|| category.to_owned())
        });
        self.currency = self.currency.trim().to_ascii_uppercase();
        if self.name.is_empty() || self.name.chars().count() > 50 {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Menu item name must be between 1 and 50 characters.",
            ));
        }
        if self
            .category
            .as_ref()
            .is_some_and(|category| category.chars().count() > 20)
        {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Menu item category must be no more than 20 characters.",
            ));
        }
        if self.currency.len() != 3 || !self.currency.bytes().all(|byte| byte.is_ascii_uppercase())
        {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Currency must be a three-letter code such as USD.",
            ));
        }
        let selling_price = strict_decimal(&self.selling_price, 4).map_err(|_| {
            ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Selling price must be a positive decimal value.",
            )
        })?;
        if selling_price <= 0 {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Selling price must be greater than zero.",
            ));
        }
        Ok(ValidatedMenuItem {
            name: self.name,
            category: self.category,
            selling_price,
            currency: self.currency,
        })
    }
}

struct ValidatedMenuItem {
    name: String,
    category: Option<String>,
    selling_price: BigDecimal,
    currency: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(price: &str) -> CreateMenuItem {
        CreateMenuItem {
            name: "  Chicken taco ".into(),
            category: Some("  Tacos ".into()),
            selling_price: price.into(),
            currency: " usd ".into(),
        }
    }

    #[test]
    fn normalizes_valid_menu_items() {
        let item = input("12.50").validated().unwrap();
        assert_eq!(item.name, "Chicken taco");
        assert_eq!(item.category.as_deref(), Some("Tacos"));
        assert_eq!(item.currency, "USD");
    }

    #[test]
    fn rejects_invalid_prices_and_currency() {
        assert!(input("0").validated().is_err());
        assert!(input("12.12345").validated().is_err());
        let mut invalid = input("12.50");
        invalid.currency = "dollars".into();
        assert!(invalid.validated().is_err());

        let mut long_name = input("12.50");
        long_name.name = "x".repeat(51);
        assert!(long_name.validated().is_err());

        let mut long_category = input("12.50");
        long_category.category = Some("x".repeat(21));
        assert!(long_category.validated().is_err());
    }
}
