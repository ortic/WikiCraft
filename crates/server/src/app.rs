use std::sync::Arc;

use sqlx::PgPool;

use crate::blobs::BlobStore;
use crate::collab::Rooms;
use crate::config::Config;
use crate::search::SearchIndex;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<Inner>,
}

pub struct Inner {
    /// set on SIGTERM: readiness turns false so load balancers stop sending traffic
    pub shutting_down: std::sync::atomic::AtomicBool,
    pub metrics: crate::metrics::Metrics,
    /// identifies this replica (cluster relay origin, job worker id)
    pub node_id: uuid::Uuid,
    /// collaboration relay to other replicas (`WIKICRAFT_CLUSTER=postgres`)
    pub relay: Option<crate::cluster::Relay>,
    /// configured external sign-in providers
    pub providers: Vec<crate::sso::Provider>,
    pub db: PgPool,
    pub config: Config,
    pub blobs: BlobStore,
    pub search: SearchIndex,
    pub rooms: Rooms,
}

impl std::ops::Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.inner
    }
}

impl AppState {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&config.data_dir)?;
        std::fs::create_dir_all(config.upload_dir())?;
        let db =
            sqlx::postgres::PgPoolOptions::new().max_connections(20).acquire_timeout(std::time::Duration::from_secs(10)).connect(&config.database_url).await?;
        sqlx::migrate!("./migrations").run(&db).await?;
        let blobs = BlobStore::new(&config).await?;
        let search = SearchIndex::open(&config, db.clone())?;
        let providers = crate::sso::load(&config)?;
        let relay = match config.cluster.as_str() {
            "postgres" => Some(crate::cluster::Relay::new()),
            "none" | "" => None,
            other => anyhow::bail!("unknown cluster mode {other:?} (use none or postgres)"),
        };
        let state = AppState {
            inner: Arc::new(Inner {
                shutting_down: Default::default(),
                metrics: Default::default(),
                node_id: uuid::Uuid::new_v4(),
                relay,
                providers,
                db,
                config,
                blobs,
                search,
                rooms: Rooms::default(),
            }),
        };
        Ok(state)
    }
}

pub fn new_id() -> uuid::Uuid {
    uuid::Uuid::now_v7()
}
