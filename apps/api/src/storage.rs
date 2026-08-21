use anyhow::{Context, Result};
use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Builder, Region},
    presigning::PresigningConfig,
    primitives::ByteStream,
};
use bytes::Bytes;
use std::{env, time::Duration};

#[cfg(test)]
use std::{collections::HashMap, sync::Arc};
#[cfg(test)]
use tokio::sync::Mutex;

#[cfg(test)]
type MemoryObjects = Arc<Mutex<HashMap<String, Vec<u8>>>>;

#[derive(Clone)]
pub(crate) struct ObjectStorage {
    client: Client,
    bucket: String,
    #[cfg(test)]
    memory: Option<MemoryObjects>,
}

impl ObjectStorage {
    pub(crate) async fn from_env() -> Result<Self> {
        let account_id = required_env("R2_ACCOUNT_ID")?;
        let access_key = required_env("R2_ACCESS_KEY_ID")?;
        let secret_key = required_env("R2_SECRET_ACCESS_KEY")?;
        let bucket = required_env("R2_BUCKET")?;
        let credentials = Credentials::new(access_key, secret_key, None, None, "r2-static");
        let config = Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("auto"))
            .credentials_provider(credentials)
            .endpoint_url(format!("https://{account_id}.r2.cloudflarestorage.com"))
            .force_path_style(true)
            .build();
        Ok(Self {
            client: Client::from_conf(config),
            bucket,
            #[cfg(test)]
            memory: None,
        })
    }

    // In-memory backend so release tests exercise upload and cleanup paths
    // without touching real object storage.
    #[cfg(test)]
    pub(crate) fn inert_for_tests() -> Self {
        let config = Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("auto"))
            .credentials_provider(Credentials::new(
                uuid::Uuid::now_v7().to_string(),
                uuid::Uuid::now_v7().to_string(),
                None,
                None,
                "release-tests",
            ))
            .endpoint_url("http://127.0.0.1:9")
            .force_path_style(true)
            .build();
        Self {
            client: Client::from_conf(config),
            bucket: "release-tests".into(),
            memory: Some(Arc::new(Mutex::new(HashMap::new()))),
        }
    }

    pub(crate) async fn put(&self, key: &str, content_type: &str, body: Bytes) -> Result<()> {
        #[cfg(test)]
        if let Some(memory) = &self.memory {
            let mut objects = memory.lock().await;
            objects.insert(key.to_owned(), body.to_vec());
            return Ok(());
        }
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(ByteStream::from(body))
            .send()
            .await
            .context("R2 put object failed")?;
        Ok(())
    }

    pub(crate) async fn delete(&self, key: &str) -> Result<()> {
        #[cfg(test)]
        if let Some(memory) = &self.memory {
            memory.lock().await.remove(key);
            return Ok(());
        }
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .context("R2 delete object failed")?;
        Ok(())
    }

    pub(crate) async fn get(&self, key: &str) -> Result<Bytes> {
        #[cfg(test)]
        if let Some(memory) = &self.memory {
            let objects = memory.lock().await;
            return objects
                .get(key)
                .cloned()
                .map(Bytes::from)
                .context("object not found in test storage");
        }
        let object = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .context("R2 get object failed")?;
        Ok(object
            .body
            .collect()
            .await
            .context("R2 object download failed")?
            .into_bytes())
    }

    pub(crate) async fn signed_get_url(&self, key: &str) -> Result<String> {
        #[cfg(test)]
        if let Some(memory) = &self.memory {
            let exists = memory.lock().await.contains_key(key);
            anyhow::ensure!(exists, "object not found in test storage");
            return Ok(format!("http://test-storage.local/{key}"));
        }
        let config = PresigningConfig::expires_in(Duration::from_secs(300))
            .context("invalid signed URL expiry")?;
        let request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(config)
            .await
            .context("R2 URL signing failed")?;
        Ok(request.uri().to_string())
    }
}

fn required_env(name: &str) -> Result<String> {
    let value = env::var(name).with_context(|| format!("{name} must be set"))?;
    anyhow::ensure!(!value.trim().is_empty(), "{name} cannot be empty");
    Ok(value)
}
