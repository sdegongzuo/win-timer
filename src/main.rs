mod tasks;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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

/// 缓存的有效期（列表与执行历史共用）。
/// 枚举全部计划任务要 spawn PowerShell 逐个取信息，实测 scope=all 约 6.7 秒；
/// 历史接口即使"历史记录未启用"也要约 2.6 秒。TTL 内直接复用上次结果，
/// 避免前端轮询把 CPU 打满。
const CACHE_TTL: Duration = Duration::from_secs(30);

struct CachedList {
    fetched_at: Instant,
    tasks: Arc<Vec<tasks::TaskSummary>>,
}

struct CachedHistory {
    fetched_at: Instant,
    payload: Arc<tasks::HistoryPayload>,
}

/// 共享状态：列表按 root / all 两种 scope 各缓存一份；
/// 执行历史按 task 过滤参数缓存（空串 = 全部）。
#[derive(Default)]
struct AppState {
    cache_root: Mutex<Option<CachedList>>,
    cache_all: Mutex<Option<CachedList>>,
    cache_history: Mutex<HashMap<String, CachedHistory>>,
    /// 缓存代数：每次 invalidate 自增。PowerShell 枚举要数秒，期间若发生写操作，
    /// 完成后的旧列表不得写回缓存——否则新任务最长 30 秒不可见。
    generation: AtomicU64,
}

impl AppState {
    fn cache(&self, all: bool) -> &Mutex<Option<CachedList>> {
        if all {
            &self.cache_all
        } else {
            &self.cache_root
        }
    }

    /// 任何写操作（创建/删除/启停/编辑）之后都让缓存失效，
    /// 保证前端动作后的刷新能立刻看到最新状态。
    fn invalidate(&self) {
        *self.cache_root.lock().unwrap() = None;
        *self.cache_all.lock().unwrap() = None;
        self.generation.fetch_add(1, Ordering::Release);
    }
}

async fn list_tasks(
    State(state): State<Arc<AppState>>,
    q: Query<ListQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let all = q.scope.as_deref() == Some("all");

    // 命中未过期缓存：直接返回，不启动 PowerShell。
    let hit = {
        let guard = state.cache(all).lock().unwrap();
        match guard.as_ref() {
            Some(c) if c.fetched_at.elapsed() < CACHE_TTL => Some(Arc::clone(&c.tasks)),
            _ => None,
        }
    };
    if let Some(tasks) = hit {
        return Ok(Json(json!({ "tasks": tasks.as_ref() })));
    }

    // 未命中：PowerShell 是同步阻塞调用，必须丢进阻塞线程池，
    // 否则会占死 tokio 工作线程长达数秒。
    let gen_before = state.generation.load(Ordering::Acquire);
    let list = tokio::task::spawn_blocking(move || tasks::list_tasks(all))
        .await
        .map_err(|e| ApiError {
            error: format!("后台枚举任务失败: {e}"),
        })?
        .map_err(ApiError::from)?;
    let list = Arc::new(list);

    // 枚举期间若无写操作使缓存失效，才允许写回；
    // 否则刚 invalidate 过的缓存会被"过期前发起"的旧列表重新填满。
    if state.generation.load(Ordering::Acquire) == gen_before {
        *state.cache(all).lock().unwrap() = Some(CachedList {
            fetched_at: Instant::now(),
            tasks: Arc::clone(&list),
        });
    }

    Ok(Json(json!({ "tasks": list.as_ref() })))
}

async fn create_task(
    State(state): State<Arc<AppState>>,
    Json(req): Json<tasks::CreateTaskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // PowerShell 调用是秒级同步阻塞，统一丢进阻塞线程池，避免占死 tokio 工作线程。
    let located = tokio::task::spawn_blocking(move || tasks::create_task(&req))
        .await
        .map_err(|e| ApiError {
            error: format!("后台执行创建任务失败: {e}"),
        })?
        .map_err(ApiError::from)?;
    state.invalidate();
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
    State(state): State<Arc<AppState>>,
    Path((verb, name)): Path<(String, String)>,
    q: Query<TaskPathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    tokio::task::spawn_blocking(move || tasks::task_verb(&verb, &name, q.path.as_deref()))
        .await
        .map_err(|e| ApiError {
            error: format!("后台执行任务操作失败: {e}"),
        })?
        .map_err(ApiError::from)?;
    state.invalidate();
    Ok(Json(json!({ "ok": true })))
}

async fn delete_task(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    q: Query<TaskPathQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    tokio::task::spawn_blocking(move || tasks::delete_task(&name, q.path.as_deref()))
        .await
        .map_err(|e| ApiError {
            error: format!("后台执行删除任务失败: {e}"),
        })?
        .map_err(ApiError::from)?;
    state.invalidate();
    Ok(Json(json!({ "ok": true })))
}

async fn update_task(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    q: Query<TaskPathQuery>,
    Json(req): Json<tasks::UpdateTaskRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    tokio::task::spawn_blocking(move || tasks::update_task(&name, q.path.as_deref(), &req))
        .await
        .map_err(|e| ApiError {
            error: format!("后台执行编辑任务失败: {e}"),
        })?
        .map_err(ApiError::from)?;
    state.invalidate();
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct HistoryQuery {
    /// 可选：按任务名精确过滤
    task: Option<String>,
}

async fn get_history(
    State(state): State<Arc<AppState>>,
    q: Query<HistoryQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // 缓存键：任务名（空串 = 全部任务）。读事件日志即使历史未启用也要约 2.6 秒。
    let key = q.task.clone().unwrap_or_default();
    let hit = {
        let guard = state.cache_history.lock().unwrap();
        match guard.get(&key) {
            Some(c) if c.fetched_at.elapsed() < CACHE_TTL => Some(Arc::clone(&c.payload)),
            _ => None,
        }
    };
    if let Some(p) = hit {
        return Ok(Json(json!({
            "history_enabled": p.history_enabled,
            "rows": p.rows.as_slice(),
        })));
    }

    let payload = tokio::task::spawn_blocking(move || tasks::task_history(q.task.as_deref()))
        .await
        .map_err(|e| ApiError {
            error: format!("后台读取执行历史失败: {e}"),
        })?
        .map_err(ApiError::from)?;
    let payload = Arc::new(payload);

    // 写入缓存前清掉同键的过期项，防止 map 无限增长。
    {
        let mut guard = state.cache_history.lock().unwrap();
        guard.retain(|_, c| c.fetched_at.elapsed() < CACHE_TTL);
        guard.insert(
            key,
            CachedHistory {
                fetched_at: Instant::now(),
                payload: Arc::clone(&payload),
            },
        );
    }

    Ok(Json(json!({
        "history_enabled": payload.history_enabled,
        "rows": payload.rows.as_slice(),
    })))
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
        .route("/api/tasks/{name}", put(update_task).delete(delete_task))
        .route("/api/tasks/{verb}/{name}", post(task_verb))
        .route("/api/history", get(get_history))
        .layer(cors)
        .with_state(Arc::new(AppState::default()));

    // 端口刻意选在 49152-65535 动态段，避开 8080/3000/5173 等常见开发端口。
    // 改动此端口时，同步改 web/vite.config.ts 的 proxy target 与 web/src/config.ts。
    let addr: SocketAddr = "127.0.0.1:58081".parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("绑定 127.0.0.1:58081 失败，请确认端口未被占用");
    tracing::info!("win-timer backend listening on http://{addr}");
    axum::serve(listener, app).await.unwrap();
}
