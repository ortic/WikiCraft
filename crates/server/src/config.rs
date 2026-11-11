use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Args;

/// Server configuration (CLI flags or `WIKICRAFT_*` environment variables).
#[derive(Debug, Clone, Args)]
pub struct Config {
    /// PostgreSQL connection URL.
    #[arg(long, env = "WIKICRAFT_DATABASE_URL", default_value = "postgres://wikicraft:wikicraft@127.0.0.1:55432/wikicraft")]
    pub database_url: String,

    /// Address to listen on.
    #[arg(long, env = "WIKICRAFT_BIND", default_value = "127.0.0.1:8787")]
    pub bind: SocketAddr,

    /// Public base URL (used in e-mails and absolute links).
    #[arg(long, env = "WIKICRAFT_BASE_URL", default_value = "http://127.0.0.1:8787")]
    pub base_url: String,

    /// Directory for local data (search index, uploads, local blob storage).
    #[arg(long, env = "WIKICRAFT_DATA_DIR", default_value = "./data")]
    pub data_dir: PathBuf,

    /// Directory with the built frontend (`web/dist`). Served at `/`.
    #[arg(long, env = "WIKICRAFT_STATIC_DIR", default_value = "./web/dist")]
    pub static_dir: PathBuf,

    /// S3 bucket for attachments (local filesystem storage when unset).
    #[arg(long, env = "WIKICRAFT_S3_BUCKET")]
    pub s3_bucket: Option<String>,

    /// Custom S3 endpoint (MinIO, Ceph, R2, …).
    #[arg(long, env = "WIKICRAFT_S3_ENDPOINT")]
    pub s3_endpoint: Option<String>,

    /// S3 region (defaults to AWS_REGION / us-east-1).
    #[arg(long, env = "WIKICRAFT_S3_REGION")]
    pub s3_region: Option<String>,

    /// S3 access key id (defaults to AWS_ACCESS_KEY_ID).
    #[arg(long, env = "WIKICRAFT_S3_ACCESS_KEY_ID")]
    pub s3_access_key_id: Option<String>,

    /// S3 secret access key (defaults to AWS_SECRET_ACCESS_KEY).
    #[arg(long, env = "WIKICRAFT_S3_SECRET_ACCESS_KEY", hide_env_values = true)]
    pub s3_secret_access_key: Option<String>,

    /// Maximum upload size in megabytes (attachments and imports).
    #[arg(long, env = "WIKICRAFT_MAX_UPLOAD_MB", default_value_t = 2048)]
    pub max_upload_mb: u64,

    /// Mark session cookies `Secure` (enable behind HTTPS).
    #[arg(long, env = "WIKICRAFT_SECURE_COOKIES", default_value_t = false)]
    pub secure_cookies: bool,

    /// Session lifetime in days.
    #[arg(long, env = "WIKICRAFT_SESSION_DAYS", default_value_t = 30)]
    pub session_days: i64,

    /// Sign out sessions that were inactive for this many minutes (0 = only the absolute
    /// lifetime `WIKICRAFT_SESSION_DAYS` applies).
    #[arg(long, env = "WIKICRAFT_SESSION_IDLE_MINUTES", default_value_t = 0)]
    pub session_idle_minutes: u64,

    /// Outgoing mail server: `smtps://user:pass@mail.example.com` (TLS, port 465),
    /// `smtp://user:pass@mail.example.com:587?tls=required` (STARTTLS) or
    /// `smtp://localhost:1025` (unencrypted, local relays/dev only). URL-encode special
    /// characters in the password.
    /// Enables invitations, password reset and notification e-mails.
    #[arg(long, env = "WIKICRAFT_SMTP_URL", hide_env_values = true)]
    pub smtp_url: Option<String>,

    /// Virus scanning of uploads and imported files with ClamAV: `tcp://clamd:3310` or
    /// `unix:///run/clamav/clamd.ctl`. Infected files are refused.
    #[arg(long, env = "WIKICRAFT_CLAMAV")]
    pub clamav: Option<String>,

    /// Accept files when the virus scanner can't be reached (default: refuse them).
    #[arg(long, env = "WIKICRAFT_CLAMAV_FAIL_OPEN", default_value_t = false)]
    pub clamav_fail_open: bool,

    /// Allow webhooks to private / loopback addresses (off by default: SSRF protection).
    #[arg(long, env = "WIKICRAFT_WEBHOOK_ALLOW_PRIVATE", default_value_t = false)]
    pub webhook_allow_private: bool,

    /// Allow link previews of private / loopback addresses (off by default: SSRF protection;
    /// independent of the webhook setting). Only for closed intranets.
    #[arg(long, env = "WIKICRAFT_UNFURL_ALLOW_PRIVATE", default_value_t = false)]
    pub unfurl_allow_private: bool,

    /// Sender of e-mails.
    #[arg(long, env = "WIKICRAFT_MAIL_FROM", default_value = "WikiCraft <wikicraft@localhost>")]
    pub mail_from: String,

    /// External sign-in providers as a JSON list (OIDC, GitHub, LDAP, SAML; see README).
    #[arg(long, env = "WIKICRAFT_AUTH_PROVIDERS", hide_env_values = true)]
    pub auth_providers: Option<String>,

    /// File with the provider list (JSON), e.g. a mounted Kubernetes secret.
    #[arg(long, env = "WIKICRAFT_AUTH_PROVIDERS_FILE")]
    pub auth_providers_file: Option<PathBuf>,

    /// OpenID Connect issuer URL (enables SSO), e.g. https://keycloak.example.com/realms/acme
    #[arg(long, env = "WIKICRAFT_OIDC_ISSUER")]
    pub oidc_issuer: Option<String>,

    /// OIDC client id.
    #[arg(long, env = "WIKICRAFT_OIDC_CLIENT_ID")]
    pub oidc_client_id: Option<String>,

    /// OIDC client secret.
    #[arg(long, env = "WIKICRAFT_OIDC_CLIENT_SECRET", hide_env_values = true)]
    pub oidc_client_secret: Option<String>,

    /// Label of the SSO button on the login page.
    #[arg(long, env = "WIKICRAFT_OIDC_NAME", default_value = "Single sign-on")]
    pub oidc_name: String,

    /// Create accounts for unknown SSO users on first login (otherwise only existing users may log in).
    #[arg(long, env = "WIKICRAFT_OIDC_AUTO_CREATE", default_value_t = true, action = clap::ArgAction::Set)]
    pub oidc_auto_create: bool,

    /// Multi-replica mode: `none` (single instance) or `postgres` (relay live editing between
    /// replicas via LISTEN/NOTIFY).
    #[arg(long, env = "WIKICRAFT_CLUSTER", default_value = "none")]
    pub cluster: String,

    /// Search backend: `tantivy` (embedded, single node) or `postgres` (shared by all replicas).
    #[arg(long, env = "WIKICRAFT_SEARCH", default_value = "tantivy")]
    pub search: String,

    /// Log format: `text` (human readable) or `json` (one JSON object per line, for log shippers).
    #[arg(long, env = "WIKICRAFT_LOG_FORMAT", default_value = "text")]
    pub log_format: String,

    /// Bearer token protecting `/api/metrics` (Prometheus). Unprotected when unset.
    #[arg(long, env = "WIKICRAFT_METRICS_TOKEN")]
    pub metrics_token: Option<String>,

    /// Disable password logins (SSO only).
    #[arg(long, env = "WIKICRAFT_DISABLE_PASSWORD_LOGIN", default_value_t = false)]
    pub disable_password_login: bool,

    /// Anthropic API key: enables "Ask the wiki" (answers from the pages a person can see).
    /// Unset = the feature is off and no content ever leaves the server.
    #[arg(long, env = "WIKICRAFT_AI_API_KEY", hide_env_values = true)]
    pub ai_api_key: Option<String>,

    /// Claude model for "Ask the wiki".
    #[arg(long, env = "WIKICRAFT_AI_MODEL", default_value = "claude-opus-5-5")]
    pub ai_model: String,

    /// Base URL of the Anthropic API (a gateway or proxy may sit in between).
    #[arg(long, env = "WIKICRAFT_AI_BASE_URL", default_value = "https://api.anthropic.com")]
    pub ai_base_url: String,

    /// OpenAI-compatible embeddings API for semantic retrieval, e.g. `https://api.openai.com/v1`
    /// or Ollama's `http://localhost:11434/v1`. Unset = keyword retrieval only, and no page text
    /// leaves the server for it. Needs the pgvector extension in the database.
    #[arg(long, env = "WIKICRAFT_EMBEDDINGS_URL")]
    pub embeddings_url: Option<String>,

    /// Embedding model, e.g. `text-embedding-3-small`, `nomic-embed-text`, `voyage-3.5`.
    #[arg(long, env = "WIKICRAFT_EMBEDDINGS_MODEL")]
    pub embeddings_model: Option<String>,

    /// API key for the embeddings API (sent as a bearer token; not needed for a local Ollama).
    #[arg(long, env = "WIKICRAFT_EMBEDDINGS_API_KEY", hide_env_values = true)]
    pub embeddings_api_key: Option<String>,

    /// Ask for vectors of this size (for models that can shorten theirs, like text-embedding-3-*).
    #[arg(long, env = "WIKICRAFT_EMBEDDINGS_DIMENSIONS")]
    pub embeddings_dimensions: Option<u32>,

    /// Text put before each question, for models trained with prefixes
    /// (nomic-embed-text: `search_query: `, E5: `query: `).
    #[arg(long, env = "WIKICRAFT_EMBEDDINGS_QUERY_PREFIX", default_value = "")]
    pub embeddings_query_prefix: String,

    /// Text put before each passage (nomic-embed-text: `search_document: `, E5: `passage: `).
    #[arg(long, env = "WIKICRAFT_EMBEDDINGS_DOCUMENT_PREFIX", default_value = "")]
    pub embeddings_document_prefix: String,
}

impl Config {
    pub fn index_dir(&self) -> PathBuf {
        self.data_dir.join("index")
    }

    pub fn blob_dir(&self) -> PathBuf {
        self.data_dir.join("blobs")
    }

    pub fn upload_dir(&self) -> PathBuf {
        self.data_dir.join("uploads")
    }
}
