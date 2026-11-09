//! Content-addressed blob storage (local filesystem or S3-compatible) via `object_store`.

use std::sync::Arc;

use bytes::Bytes;
use futures::stream::BoxStream;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};
use sha2::{Digest, Sha256};
use sqlx::PgExecutor;

use crate::config::Config;

#[derive(Clone)]
pub struct BlobStore {
    store: Arc<dyn ObjectStore>,
}

impl BlobStore {
    pub async fn new(config: &Config) -> anyhow::Result<Self> {
        let store: Arc<dyn ObjectStore> = match &config.s3_bucket {
            Some(bucket) => {
                let mut b = object_store::aws::AmazonS3Builder::from_env().with_bucket_name(bucket);
                if let Some(ep) = &config.s3_endpoint {
                    b = b.with_endpoint(ep).with_allow_http(ep.starts_with("http://")).with_virtual_hosted_style_request(false);
                }
                Arc::new(b.build()?)
            }
            None => {
                std::fs::create_dir_all(config.blob_dir())?;
                Arc::new(object_store::local::LocalFileSystem::new_with_prefix(config.blob_dir())?)
            }
        };
        Ok(Self { store })
    }

    fn path(key: &str) -> Path {
        Path::from(format!("{}/{}/{}", &key[0..2], &key[2..4], key))
    }

    pub fn key_for(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    /// Stores `data` (deduplicated by sha256) and records it in the `blobs` table. Returns the key.
    pub async fn put<'e>(&self, db: impl PgExecutor<'e>, data: Bytes) -> anyhow::Result<String> {
        let key = Self::key_for(&data);
        let size = data.len() as i64;
        let path = Self::path(&key);
        if self.store.head(&path).await.is_err() {
            self.store.put(&path, PutPayload::from_bytes(data)).await?;
        }
        sqlx::query("INSERT INTO blobs (key, size) VALUES ($1, $2) ON CONFLICT (key) DO NOTHING").bind(&key).bind(size).execute(db).await?;
        Ok(key)
    }

    pub async fn get(&self, key: &str) -> anyhow::Result<Bytes> {
        Ok(self.store.get(&Self::path(key)).await?.bytes().await?)
    }

    pub async fn stream(&self, key: &str) -> anyhow::Result<BoxStream<'static, object_store::Result<Bytes>>> {
        Ok(self.store.get(&Self::path(key)).await?.into_stream())
    }
}
