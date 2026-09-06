mod tasks;

use axum::{
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};

#[derive(Serialize)]
struct Health {
    status: &'static str,
    service: &'static str,
}

/// Uniform JSON error body for the UI.
#[derive(Serialize)]
struct ApiError {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(self)).into_response()
    }
}

impl From<String> for ApiError {
    fn from(e: String) -> Self {
        ApiError { error: e }
    }
}

#[derive(Deserialize)]
struct ListQuery {
    scope: Option<String>,
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        service: "win-timer",
    })
}

async fn list_tasks(q: Query<ListQuery>) -> Result<Json<serde_json::Value>, ApiError> {
    let all = q.scope.as_deref() == Some("all");
    let list = tasks::list_tasks(all)?;
    Ok(Json(json!({ "tasks": list })))
}

async fn create_task(
    Json(req): Json<tasks::CreateTaskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let located = tasks::create_task(&req).map_err(ApiError::from)?;
    let (path, name) = split_located(&located);
    Ok(Json(json!({ "ok": true, "name": name, "path": path })))
}

fn split_located(located: &str) -> (String, String) {
    // located looks like "\TaskName"
    match located.rfind('\\') {
        Some(idx) if idx + 1 < located.len() => {
            let mut path = located[..=idx].to_string();
            if path.is_empty() {
                path.push('\\');
            }
            (path, located[idx + 1..].to_string())
        }
        _ => ("\\".to_string(), located.to_string()),
    }
}

#[derive(Deserialize)]
struct TaskPathQuery {
    path: Option<String>,
}

async fn task_verb(
    Path((verb, name)): Path<(String, String)>,
    q: Query<TaskPathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    tasks::task_verb(&verb, &name, q.path.as_deref()).map_err(ApiError::from)?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_task(
    Path(name): Path<String>,
    q: Query<TaskPathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    tasks::delete_task(&name, q.path.as_deref()).map_err(ApiError::from)?;
    Ok(Json(json!({ "ok": true })))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "win_timer=debug,tower_http=debug".into()),
        )
        .init();

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/tasks", get(list_tasks).post(create_task))
        .route("/api/tasks/{name}", delete(delete_task))
        .route("/api/tasks/{verb}/{name}", post(task_verb))
        .layer(cors);

    let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("绑定 127.0.0.1:8080 失败，请确认端口未被占用");
    tracing::info!("win-timer backend listening on http://{addr}");
    axum::serve(listener, app).await.unwrap();
}
