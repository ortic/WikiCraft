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
                // credentials: WIKICRAFT_S3_* settings, falling back to the standard AWS_* environment
                let mut b = object_store::aws::AmazonS3Builder::from_env().with_bucket_name(bucket);
                if let Some(r) = &config.s3_region {
                    b = b.with_region(r);
                }
                if let Some(k) = &config.s3_access_key_id {
                    b = b.with_access_key_id(k);
                }
                if let Some(k) = &config.s3_secret_access_key {
                    b = b.with_secret_access_key(k);
                }
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

    /// Streams a local file to `key` (multipart upload; used for import uploads, not content-addressed).
    pub async fn put_file(&self, key: &str, path: &std::path::Path) -> anyhow::Result<u64> {
        use tokio::io::AsyncReadExt;
        let upload = self.store.put_multipart(&Path::from(key)).await?;
        let mut w = object_store::WriteMultipart::new_with_chunk_size(upload, 8 << 20);
        let mut f = tokio::fs::File::open(path).await?;
        let mut buf = vec![0u8; 1 << 20];
        let mut total = 0u64;
        loop {
            let n = f.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            w.wait_for_capacity(4).await?;
            w.write(&buf[..n]);
            total += n as u64;
        }
        w.finish().await?;
        Ok(total)
    }

    /// Streams `key` into a local file.
    pub async fn download_to(&self, key: &str, path: &std::path::Path) -> anyhow::Result<()> {
        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;
        let mut stream = self.store.get(&Path::from(key)).await?.into_stream();
        let mut f = tokio::fs::File::create(path).await?;
        while let Some(chunk) = stream.next().await {
            f.write_all(&chunk?).await?;
        }
        f.flush().await?;
        Ok(())
    }

    pub async fn delete_key(&self, key: &str) -> anyhow::Result<()> {
        self.store.delete(&Path::from(key)).await?;
        Ok(())
    }

    pub fn backend_name(&self) -> String {
        self.store.to_string()
    }

    /// Streams a non-content-addressed object (export archives, uploads).
    pub async fn stream_key(&self, key: &str) -> anyhow::Result<BoxStream<'static, object_store::Result<Bytes>>> {
        Ok(self.store.get(&Path::from(key)).await?.into_stream())
    }

    pub async fn stream(&self, key: &str) -> anyhow::Result<BoxStream<'static, object_store::Result<Bytes>>> {
        Ok(self.store.get(&Self::path(key)).await?.into_stream())
    }
}
