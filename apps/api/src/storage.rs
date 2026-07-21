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

#[derive(Clone)]
pub(crate) struct ObjectStorage {
    client: Client,
    bucket: String,
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
        })
    }

    pub(crate) async fn put(&self, key: &str, content_type: &str, body: Bytes) -> Result<()> {
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
