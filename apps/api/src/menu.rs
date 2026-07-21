use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ApiError, AppState,
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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateMenuItem {
    name: String,
    category: Option<String>,
    selling_price: String,
    currency: String,
}

pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<MenuItem>>, ApiError> {
    let restaurant_id = membership(&state, &headers).await?.restaurant_id;
    let items = sqlx::query_as::<_, MenuItem>(
        "SELECT id,name,category,selling_price::text AS selling_price,currency,active
         FROM menu_items WHERE restaurant_id=$1 AND active
         ORDER BY category NULLS LAST,name,id",
    )
    .bind(restaurant_id)
    .fetch_all(&state.pool)
    .await
    .map_err(crate::database_error)?;
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
         RETURNING id,name,category,selling_price::text AS selling_price,currency,active",
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
                "That menu item is already in your Daybook.",
            )
        } else {
            crate::database_error(error)
        }
    })?;
    Ok((StatusCode::CREATED, Json(item)))
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
