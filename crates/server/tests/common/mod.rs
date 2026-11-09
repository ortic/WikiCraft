//! Test harness: a fresh database per test + the full app on an ephemeral port.

#![allow(dead_code)]

pub mod peer;

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{Value, json};
use wikicraft::app::AppState;
use wikicraft::config::Config;

pub const ADMIN_URL: &str = "postgres://wikicraft:wikicraft@127.0.0.1:55432/postgres";

pub struct TestApp {
    pub base: String,
    pub state: AppState,
    pub _data: tempfile::TempDir,
}

pub async fn spawn() -> TestApp {
    spawn_with(|_| {}).await
}

/// Like [`spawn`], with a hook to adjust the configuration (base URL is already the real address).
pub async fn spawn_with(tweak: impl FnOnce(&mut Config)) -> TestApp {
    let _ = tracing_subscriber::fmt().with_env_filter("wikicraft=debug,warn").with_test_writer().try_init();
    let db_name = format!("wikicraft_test_{}", uuid::Uuid::new_v4().simple());
    let admin = sqlx::PgPool::connect(ADMIN_URL).await.expect("postgres reachable (docker container wikicraft-pg)");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {db_name}"))).execute(&admin).await.unwrap();
    let data = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut config = Config {
        database_url: format!("postgres://wikicraft:wikicraft@127.0.0.1:55432/{db_name}"),
        bind: addr,
        base_url: format!("http://{addr}"),
        data_dir: data.path().to_path_buf(),
        static_dir: data.path().join("static"),
        s3_bucket: None,
        s3_endpoint: None,
        s3_region: None,
        s3_access_key_id: None,
        s3_secret_access_key: None,
        max_upload_mb: 512,
        secure_cookies: false,
        session_days: 30,
        auth_providers: None,
        auth_providers_file: None,
        oidc_issuer: None,
        oidc_client_id: None,
        oidc_client_secret: None,
        oidc_name: "Single sign-on".into(),
        oidc_auto_create: true,
        disable_password_login: false,
        log_format: "text".into(),
        search: "tantivy".into(),
        cluster: "none".into(),
        metrics_token: None,
        smtp_url: None,
        session_idle_minutes: 0,
        webhook_allow_private: true,
        mail_from: "WikiCraft <wiki@acme.test>".into(),
    };
    tweak(&mut config);
    let state = AppState::new(config).await.unwrap();
    tokio::spawn(wikicraft::collab::persist_loop(state.clone()));
    wikicraft::cluster::spawn(&state);
    wikicraft::jobs::spawn_worker(&state);
    wikicraft::mail::spawn_worker(&state);
    wikicraft::webhooks::spawn_worker(&state);
    let app = wikicraft::routes::router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    TestApp { base: format!("http://{addr}"), state, _data: data }
}

pub fn client() -> reqwest::Client {
    let mut h = HeaderMap::new();
    h.insert("x-wikicraft", HeaderValue::from_static("1"));
    reqwest::Client::builder().cookie_store(true).default_headers(h).build().unwrap()
}

/// A client without the CSRF header (to test that it's enforced).
pub fn bare_client() -> reqwest::Client {
    reqwest::Client::builder().cookie_store(true).build().unwrap()
}

impl TestApp {
    pub fn url(&self, path: &str) -> String {
        format!("{}/api{}", self.base, path)
    }

    /// Runs first-time setup; returns a logged-in owner client.
    pub async fn setup(&self) -> reqwest::Client {
        let c = client();
        let r = c
            .post(self.url("/setup"))
            .json(&json!({"workspaceName": "Acme", "name": "Owner", "email": "owner@acme.test", "password": "correct horse"}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
        c
    }

    /// Creates a user as `admin` and returns a logged-in client for it.
    pub async fn user(&self, admin: &reqwest::Client, email: &str, role: &str) -> (reqwest::Client, String) {
        let r = admin
            .post(self.url("/users"))
            .json(&json!({"email": email, "name": email.split('@').next().unwrap(), "password": "password123", "role": role}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
        let id = r.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_string();
        let c = client();
        let r = c.post(self.url("/auth/login")).json(&json!({"email": email, "password": "password123"})).send().await.unwrap();
        assert_eq!(r.status(), 200);
        (c, id)
    }

    pub async fn get(&self, c: &reqwest::Client, path: &str) -> (u16, Value) {
        let r = c.get(self.url(path)).send().await.unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn post(&self, c: &reqwest::Client, path: &str, body: Value) -> (u16, Value) {
        let r = c.post(self.url(path)).json(&body).send().await.unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn put(&self, c: &reqwest::Client, path: &str, body: Value) -> (u16, Value) {
        let r = c.put(self.url(path)).json(&body).send().await.unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn patch(&self, c: &reqwest::Client, path: &str, body: Value) -> (u16, Value) {
        let r = c.patch(self.url(path)).json(&body).send().await.unwrap();
        let s = r.status().as_u16();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn delete(&self, c: &reqwest::Client, path: &str) -> u16 {
        c.delete(self.url(path)).send().await.unwrap().status().as_u16()
    }

    /// Polls until `f` returns Some, or panics after ~10 s.
    pub async fn eventually<T, F, Fut>(&self, mut f: F) -> T
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Option<T>>,
    {
        for _ in 0..100 {
            if let Some(v) = f().await {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("condition not reached in time");
    }
}
