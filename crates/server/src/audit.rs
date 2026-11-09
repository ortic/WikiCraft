//! Append-only audit log of security-relevant actions.

use serde_json::Value;
use uuid::Uuid;

use crate::app::AppState;

pub async fn log(state: &AppState, workspace: Uuid, actor: Option<Uuid>, action: &str, target_type: Option<&str>, target_id: Option<Uuid>, data: Value) {
    let res = sqlx::query("INSERT INTO audit_events (workspace_id, actor_id, action, target_type, target_id, data) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(workspace)
        .bind(actor)
        .bind(action)
        .bind(target_type)
        .bind(target_id)
        .bind(data)
        .execute(&state.db)
        .await;
    if let Err(e) = res {
        tracing::error!("audit log write failed: {e}");
    }
}
