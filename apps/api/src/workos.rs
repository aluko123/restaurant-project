use std::{env, sync::Arc, time::Duration};

use anyhow::{Context, ensure};
use chrono::{DateTime, Utc};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};

const API_BASE: &str = "https://api.workos.com/";

#[derive(Clone)]
pub(crate) struct WorkosClient {
    inner: Option<Arc<Inner>>,
}

struct Inner {
    client: Client,
    base: Url,
    api_key: String,
    #[cfg(test)]
    mock: Option<MockWorkos>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Invitation {
    pub id: String,
    pub email: String,
    pub state: String,
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub accepted_user_id: Option<String>,
    #[serde(default)]
    pub inviter_user_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct User {
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub email_verified: bool,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
}

#[derive(Debug)]
pub(crate) enum WorkosError {
    Unconfigured,
    Provider,
}

#[derive(Serialize)]
struct SendRequest<'a> {
    email: &'a str,
    expires_in_days: u8,
    inviter_user_id: &'a str,
}

#[derive(Deserialize)]
struct InvitationList {
    data: Vec<Invitation>,
}

impl WorkosClient {
    pub(crate) fn from_env() -> anyhow::Result<Self> {
        match env::var("WORKOS_API_KEY") {
            Err(env::VarError::NotPresent) => Ok(Self { inner: None }),
            Err(error) => Err(error).context("could not read WORKOS_API_KEY"),
            Ok(key) => {
                ensure!(!key.trim().is_empty(), "WORKOS_API_KEY cannot be blank");
                Self::new(key, API_BASE)
            }
        }
    }

    fn new(api_key: String, base: &str) -> anyhow::Result<Self> {
        let base = Url::parse(base).context("invalid WorkOS API URL")?;
        ensure!(base.scheme() == "https", "WorkOS API URL must use HTTPS");
        let client = Client::builder()
            .https_only(true)
            .timeout(Duration::from_secs(7))
            .build()?;
        Ok(Self {
            inner: Some(Arc::new(Inner {
                client,
                base,
                api_key,
                #[cfg(test)]
                mock: None,
            })),
        })
    }

    pub(crate) fn enabled(&self) -> bool {
        self.inner.is_some()
    }

    pub(crate) async fn send(&self, email: &str, inviter: &str) -> Result<Invitation, WorkosError> {
        let inner = self.inner.as_ref().ok_or(WorkosError::Unconfigured)?;
        #[cfg(test)]
        if let Some(mock) = &inner.mock {
            return mock.send(email, inviter).await;
        }
        let response = self
            .request(
                inner.client.post(
                    inner
                        .base
                        .join("user_management/invitations")
                        .map_err(|_| WorkosError::Provider)?,
                ),
            )
            .json(&SendRequest {
                email,
                expires_in_days: 7,
                inviter_user_id: inviter,
            })
            .send()
            .await
            .map_err(|error| {
                tracing::error!(%error, operation = "send invitation", "WorkOS request failed");
                WorkosError::Provider
            })?;
        decode_response(response, "send invitation").await
    }

    pub(crate) async fn user(&self, subject: &str) -> Result<User, WorkosError> {
        let inner = self.inner.as_ref().ok_or(WorkosError::Unconfigured)?;
        #[cfg(test)]
        if let Some(mock) = &inner.mock {
            return mock.user(subject).await;
        }
        let url = inner
            .base
            .join(&format!("user_management/users/{subject}"))
            .map_err(|_| WorkosError::Provider)?;
        self.request(inner.client.get(url))
            .send()
            .await
            .map_err(|_| WorkosError::Provider)?
            .error_for_status()
            .map_err(|_| WorkosError::Provider)?
            .json()
            .await
            .map_err(|_| WorkosError::Provider)
    }

    pub(crate) async fn invitation(&self, id: &str) -> Result<Invitation, WorkosError> {
        self.invitation_action(id, "").await
    }

    pub(crate) async fn invitations_for_email(
        &self,
        email: &str,
    ) -> Result<Vec<Invitation>, WorkosError> {
        let inner = self.inner.as_ref().ok_or(WorkosError::Unconfigured)?;
        #[cfg(test)]
        if let Some(mock) = &inner.mock {
            return Ok(mock
                .invitations
                .lock()
                .await
                .values()
                .filter(|invitation| invitation.email == email)
                .cloned()
                .collect());
        }
        let url = inner
            .base
            .join("user_management/invitations")
            .map_err(|_| WorkosError::Provider)?;
        self.request(inner.client.get(url))
            .query(&[("email", email), ("limit", "100")])
            .send()
            .await
            .map_err(|_| WorkosError::Provider)?
            .error_for_status()
            .map_err(|_| WorkosError::Provider)?
            .json::<InvitationList>()
            .await
            .map(|response| response.data)
            .map_err(|_| WorkosError::Provider)
    }

    pub(crate) async fn resend(&self, id: &str) -> Result<Invitation, WorkosError> {
        self.invitation_action(id, "/resend").await
    }

    pub(crate) async fn revoke(&self, id: &str) -> Result<Invitation, WorkosError> {
        self.invitation_action(id, "/revoke").await
    }

    async fn invitation_action(&self, id: &str, action: &str) -> Result<Invitation, WorkosError> {
        let inner = self.inner.as_ref().ok_or(WorkosError::Unconfigured)?;
        #[cfg(test)]
        if let Some(mock) = &inner.mock {
            return mock.invitation(id, action).await;
        }
        let url = inner
            .base
            .join(&format!("user_management/invitations/{id}{action}"))
            .map_err(|_| WorkosError::Provider)?;
        let request = if action.is_empty() {
            inner.client.get(url)
        } else {
            inner.client.post(url)
        };
        self.request(request)
            .send()
            .await
            .map_err(|_| WorkosError::Provider)?
            .error_for_status()
            .map_err(|_| WorkosError::Provider)?
            .json()
            .await
            .map_err(|_| WorkosError::Provider)
    }

    fn request(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.bearer_auth(&self.inner.as_ref().expect("configured client").api_key)
    }

    #[cfg(test)]
    pub(crate) fn mock(mock: MockWorkos) -> Self {
        let mut client = Self::new("test-secret".into(), API_BASE).unwrap();
        Arc::get_mut(client.inner.as_mut().unwrap()).unwrap().mock = Some(mock);
        client
    }
}

async fn decode_response<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    operation: &'static str,
) -> Result<T, WorkosError> {
    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unavailable")
        .to_owned();
    let body = response.bytes().await.map_err(|error| {
        tracing::error!(%error, %status, %request_id, operation, "WorkOS response could not be read");
        WorkosError::Provider
    })?;
    if !status.is_success() {
        let value = serde_json::from_slice::<serde_json::Value>(&body).unwrap_or_default();
        let code = value
            .get("error")
            .or_else(|| value.get("code"))
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let message = value
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("No provider message returned");
        tracing::error!(%status, %request_id, operation, code, message, "WorkOS rejected request");
        return Err(WorkosError::Provider);
    }
    serde_json::from_slice(&body).map_err(|error| {
        tracing::error!(%error, %status, %request_id, operation, "WorkOS response had an unexpected shape");
        WorkosError::Provider
    })
}

#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct MockWorkos {
    pub invitations: Arc<tokio::sync::Mutex<std::collections::HashMap<String, Invitation>>>,
    pub users: Arc<tokio::sync::Mutex<std::collections::HashMap<String, User>>>,
}

#[cfg(test)]
impl MockWorkos {
    async fn send(&self, email: &str, inviter: &str) -> Result<Invitation, WorkosError> {
        let value = Invitation {
            id: format!("inv_{}", uuid::Uuid::now_v7()),
            email: email.into(),
            state: "pending".into(),
            expires_at: Utc::now() + chrono::Duration::days(7),
            accepted_user_id: None,
            inviter_user_id: Some(inviter.into()),
            created_at: Utc::now(),
        };
        self.invitations
            .lock()
            .await
            .insert(value.id.clone(), value.clone());
        Ok(value)
    }
    async fn user(&self, id: &str) -> Result<User, WorkosError> {
        self.users
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or(WorkosError::Provider)
    }
    async fn invitation(&self, id: &str, action: &str) -> Result<Invitation, WorkosError> {
        let mut values = self.invitations.lock().await;
        let value = values.get_mut(id).ok_or(WorkosError::Provider)?;
        if action == "/revoke" {
            value.state = "revoked".into();
        }
        Ok(value.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_only_safe_invitation_fields() {
        let value: Invitation = serde_json::from_value(serde_json::json!({"id":"inv_1","email":"a@example.com","state":"accepted","expires_at":"2099-01-01T00:00:00Z","accepted_user_id":"user_1","created_at":"2026-01-01T00:00:00Z","accept_url":"secret"})).unwrap();
        assert_eq!(value.accepted_user_id.as_deref(), Some("user_1"));
        assert!(
            !serde_json::to_string(&value)
                .unwrap()
                .contains("accept_url")
        );
    }
    #[test]
    fn rejects_non_https_and_supports_unconfigured() {
        assert!(WorkosClient::new("x".into(), "http://api.example.com").is_err());
        assert!(!WorkosClient { inner: None }.enabled());
    }

    #[tokio::test]
    async fn unconfigured_client_never_attempts_a_mutation() {
        let client = WorkosClient { inner: None };
        assert!(matches!(
            client.send("person@example.com", "user_1").await,
            Err(WorkosError::Unconfigured)
        ));
    }
}
