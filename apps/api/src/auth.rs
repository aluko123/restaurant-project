use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::http::{HeaderMap, header::AUTHORIZATION};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use tokio::{
    sync::{Mutex, RwLock},
    time::Instant,
};

const REFRESH_COOLDOWN: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct JwtVerifier {
    inner: Arc<Inner>,
}

struct Inner {
    issuer: String,
    jwks_url: reqwest::Url,
    client: reqwest::Client,
    keys: RwLock<HashMap<String, DecodingKey>>,
    refresh: Mutex<Option<Instant>>,
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    sid: String,
    exp: usize,
    #[serde(default)]
    nbf: Option<usize>,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    #[serde(default)]
    alg: Option<String>,
    n: String,
    e: String,
}

impl JwtVerifier {
    pub fn new(issuer: String, jwks_url: String) -> anyhow::Result<Self> {
        anyhow::ensure!(!issuer.trim().is_empty(), "WORKOS_ISSUER cannot be empty");
        let jwks_url = reqwest::Url::parse(&jwks_url)?;
        anyhow::ensure!(
            jwks_url.scheme() == "https",
            "WORKOS_JWKS_URL must use HTTPS"
        );

        Ok(Self {
            inner: Arc::new(Inner {
                issuer,
                jwks_url,
                client: reqwest::Client::builder()
                    .https_only(true)
                    .timeout(Duration::from_secs(5))
                    .build()?,
                keys: RwLock::new(HashMap::new()),
                refresh: Mutex::new(None),
            }),
        })
    }

    pub async fn subject(&self, headers: &HeaderMap) -> Result<String, ()> {
        let token = bearer_token(headers).ok_or(())?;
        let header = decode_header(token).map_err(|_| ())?;
        if header.alg != Algorithm::RS256 {
            return Err(());
        }
        let kid = header
            .kid
            .filter(|value| !value.trim().is_empty())
            .ok_or(())?;
        let cached_key = {
            let keys = self.inner.keys.read().await;
            keys.get(&kid).cloned()
        };
        let key = match cached_key {
            Some(key) => key,
            None => {
                self.refresh_keys(&kid).await;
                self.inner.keys.read().await.get(&kid).cloned().ok_or(())?
            }
        };
        validate_token(token, &key, &self.inner.issuer)
    }

    async fn refresh_keys(&self, wanted_kid: &str) {
        let mut last_attempt = self.inner.refresh.lock().await;
        if self.inner.keys.read().await.contains_key(wanted_kid) {
            return;
        }
        if last_attempt.is_some_and(|last| last.elapsed() < REFRESH_COOLDOWN) {
            return;
        }
        *last_attempt = Some(Instant::now());

        let fetched = async {
            let response = self
                .inner
                .client
                .get(self.inner.jwks_url.clone())
                .send()
                .await?
                .error_for_status()?;
            response.json::<Jwks>().await
        }
        .await;
        let Ok(jwks) = fetched else {
            return;
        };
        let keys: HashMap<_, _> = jwks
            .keys
            .into_iter()
            .filter_map(|jwk| {
                if jwk.kty != "RSA" || jwk.alg.as_deref().is_some_and(|alg| alg != "RS256") {
                    return None;
                }
                DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
                    .ok()
                    .map(|key| (jwk.kid, key))
            })
            .collect();
        if !keys.is_empty() {
            *self.inner.keys.write().await = keys;
        }
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

fn validate_token(token: &str, key: &DecodingKey, issuer: &str) -> Result<String, ()> {
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[issuer]);
    validation.validate_aud = false;
    validation.leeway = 60;
    validation.validate_nbf = true;
    validation.required_spec_claims = ["exp", "iss", "sub", "sid"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let claims = decode::<Claims>(token, key, &validation)
        .map_err(|error| {
            tracing::debug!(%error, "WorkOS JWT validation failed");
        })?
        .claims;
    let _ = (claims.exp, claims.nbf);
    if claims.sub.trim().is_empty() || claims.sid.trim().is_empty() {
        return Err(());
    }
    Ok(claims.sub)
}

#[cfg(test)]
mod tests{
    use super::*;

    #[test]
    fn rejects_non_rs256_before_key_lookup() {
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(Algorithm::HS256),
            &serde_json::json!({"sub":"user","sid":"session","exp":4102444800_u64}),
            &jsonwebtoken::EncodingKey::from_secret(b"secret"),
        )
        .unwrap();
        assert_ne!(decode_header(&token).unwrap().alg, Algorithm::RS256);
    }

    #[test]
    fn bearer_header_is_exact_and_nonempty() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, "Bearer token".parse().unwrap());
        assert_eq!(bearer_token(&headers), Some("token"));
        headers.insert(AUTHORIZATION, "bearer token".parse().unwrap());
        assert_eq!(bearer_token(&headers), None);
    }

    #[test]
    fn verifier_requires_complete_https_configuration() {
        assert!(JwtVerifier::new("issuer".into(), "http://example.com/jwks".into()).is_err());
        assert!(JwtVerifier::new("".into(), "https://example.com/jwks".into()).is_err());
    }
}
