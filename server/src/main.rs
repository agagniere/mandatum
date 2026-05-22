mod config;
mod db;
mod git;
mod mcp;
mod metrics;
mod spawner;
mod sse;
mod tools;

use axum::{
    extract::{Path, Query, State},
    http::{Method, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;

use config::MandatumConfig;
use db::Database;
use metrics::Metrics;
use sse::SseBroadcaster;

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub broadcaster: SseBroadcaster,
    pub repo_path: Option<String>,
    pub base_branch: String,
    pub metrics: Arc<Metrics>,
    pub log_dir: Option<std::path::PathBuf>,
    pub config: Option<Arc<MandatumConfig>>,
}

struct Config {
    db_path:     String,
    rest_port:   u16,
    mcp_port:    u16,
    ui_path:     Option<String>,
    repo_path:   Option<String>,
    base_branch: String,
    config_path: Option<String>,
}

impl Config {
    fn from_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let mut cfg = Config {
            db_path:     "tasks.db".to_string(),
            rest_port:   3001,
            mcp_port:    3002,
            ui_path:     None,
            repo_path:   None,
            base_branch: "master".to_string(),
            config_path: None,
        };
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--db" | "-d" => {
                    i += 1;
                    cfg.db_path = args.get(i).cloned().unwrap_or_else(|| {
                        eprintln!("Error: --db requires a path"); std::process::exit(1)
                    });
                }
                "--rest-port" | "-r" => {
                    i += 1;
                    cfg.rest_port = args.get(i).and_then(|v| v.parse().ok()).unwrap_or_else(|| {
                        eprintln!("Error: --rest-port requires a number"); std::process::exit(1)
                    });
                }
                "--mcp-port" | "-m" => {
                    i += 1;
                    cfg.mcp_port = args.get(i).and_then(|v| v.parse().ok()).unwrap_or_else(|| {
                        eprintln!("Error: --mcp-port requires a number"); std::process::exit(1)
                    });
                }
                "--ui" | "-u" => {
                    i += 1;
                    cfg.ui_path = Some(args.get(i).cloned().unwrap_or_else(|| {
                        eprintln!("Error: --ui requires a path"); std::process::exit(1)
                    }));
                }
                "--repo" => {
                    i += 1;
                    cfg.repo_path = Some(args.get(i).cloned().unwrap_or_else(|| {
                        eprintln!("Error: --repo requires a path"); std::process::exit(1)
                    }));
                }
                "--base-branch" | "-b" => {
                    i += 1;
                    cfg.base_branch = args.get(i).cloned().unwrap_or_else(|| {
                        eprintln!("Error: --base-branch requires a branch name"); std::process::exit(1)
                    });
                }
                "--config" | "-c" => {
                    i += 1;
                    cfg.config_path = Some(args.get(i).cloned().unwrap_or_else(|| {
                        eprintln!("Error: --config requires a path"); std::process::exit(1)
                    }));
                }
                "--help" | "-h" => {
                    println!("Usage: mandatum-server [OPTIONS]");
                    println!();
                    println!("Options:");
                    println!("  -d, --db <path>           SQLite database path       [default: tasks.db]");
                    println!("  -r, --rest-port <port>    REST API port               [default: 3001]");
                    println!("  -m, --mcp-port <port>     MCP/SSE port                [default: 3002]");
                    println!("  -u, --ui <path>           Serve React app from path   [default: ui/dist if it exists]");
                    println!("      --repo <path>          Git repo to auto-merge into on task done");
                    println!("  -b, --base-branch <name>  Default branch to merge into [default: master]");
                    println!("  -c, --config <path>       YAML config for agent spawning [default: mandatum.yaml]");
                    println!("  -h, --help                Print this help");
                    std::process::exit(0);
                }
                unknown => {
                    eprintln!("Error: unknown argument '{}'", unknown);
                    eprintln!("Run with --help for usage.");
                    std::process::exit(1);
                }
            }
            i += 1;
        }
        // Default ui_path: serve ui/dist if it exists and --ui was not given
        if cfg.ui_path.is_none() && std::path::Path::new("ui/dist").is_dir() {
            cfg.ui_path = Some("ui/dist".to_string());
        }
        // Default config_path: mandatum.yaml if it exists and --config was not given
        if cfg.config_path.is_none() && std::path::Path::new("mandatum.yaml").is_file() {
            cfg.config_path = Some("mandatum.yaml".to_string());
        }
        cfg
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("mandatum_server=info".parse().unwrap()),
        )
        .init();

    let cfg = Config::from_args();

    // Load YAML config if available
    let mandatum_cfg: Option<Arc<MandatumConfig>> = cfg.config_path.as_deref().map(|path| {
        match config::MandatumConfig::from_file(path) {
            Ok(c) => { tracing::info!("Config       → {}", path); Arc::new(c) }
            Err(e) => { eprintln!("Error loading config {}: {}", path, e); std::process::exit(1) }
        }
    });

    // Create log directory for spawned agents
    let log_dir = std::path::Path::new("logs").join("agents");
    if mandatum_cfg.is_some() {
        std::fs::create_dir_all(&log_dir)
            .unwrap_or_else(|e| tracing::warn!("Could not create log dir: {}", e));
    }

    // project_dir from YAML is the fallback repo path when --repo is not given
    let repo_path = cfg.repo_path.clone().or_else(|| {
        mandatum_cfg.as_ref().and_then(|c| c.project_dir.clone())
    });

    let db = Database::new(&cfg.db_path).await.expect("Failed to open database");
    let broadcaster = SseBroadcaster::new();
    let metrics = Arc::new(Metrics::new());
    let log_dir_opt = mandatum_cfg.as_ref().map(|_| log_dir.clone());
    let state = Arc::new(AppState {
        db,
        broadcaster,
        repo_path: repo_path.clone(),
        base_branch: cfg.base_branch.clone(),
        metrics,
        log_dir: log_dir_opt,
        config: mandatum_cfg.clone(),
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE, Method::OPTIONS])
        .allow_headers(Any);

    let mut rest = Router::new()
        .route("/api/info", get(info_handler))
        .route("/api/tasks", get(list_tasks_handler).post(create_task_handler))
        .route("/api/tasks/reap", axum::routing::post(reap_tasks_handler))
        .route("/api/tasks/:id", get(get_task_handler).patch(update_task_handler).delete(delete_task_handler))
        .route("/api/tasks/:id/commits", get(list_commits_handler))
        .route("/api/tasks/:id/reset", axum::routing::post(reset_task_handler))
        .route("/api/activity", get(list_activity_handler))
        .route("/api/agents", get(list_agents_handler))
        .route("/api/agents/:id/log", get(agent_log_handler))
        .route("/api/agents/:id/stop", axum::routing::post(stop_agent_handler).delete(unstop_agent_handler))
        .route("/api/stats", get(stats_handler))
        .route("/events", get(sse::sse_handler));

    if let Some(ref ui_path) = cfg.ui_path {
        let index = format!("{}/index.html", ui_path);
        let serve = ServeDir::new(ui_path).fallback(ServeFile::new(index));
        rest = rest.fallback_service(serve);
    }

    let rest = rest.layer(cors.clone()).with_state(state.clone());

    // Start agent spawner if config is loaded
    if let Some(cfg_data) = mandatum_cfg {
        let sp = Arc::new(spawner::Spawner::new(
            cfg_data,
            state.clone(),
            log_dir,
        ));
        tokio::spawn(async move { sp.run().await });
        tracing::info!("Spawner      → active (polling every 5s)");
    }

    // Background watchdog: reap stale tasks every 60 seconds
    let reap_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Ok(ids) = reap_state.db.reap_stale_tasks(10).await {
                for id in ids {
                    tracing::info!("Reaped stale task {}", id);
                    reap_state.broadcaster.broadcast(
                        serde_json::json!({"event":"task_reaped","data":{"task_id":id}}).to_string()
                    );
                }
            }
        }
    });

    let mcp = mcp::create_router(state).layer(cors);

    tracing::info!("Database  → {}", cfg.db_path);
    tracing::info!("REST API  → http://0.0.0.0:{}", cfg.rest_port);
    tracing::info!("MCP/SSE   → http://0.0.0.0:{}/sse", cfg.mcp_port);
    if let Some(ref p) = cfg.ui_path {
        tracing::info!("React UI  → http://0.0.0.0:{} (serving {})", cfg.rest_port, p);
    }
    if let Some(ref p) = repo_path {
        tracing::info!("Auto-merge → enabled (repo: {}, base: {})", p, cfg.base_branch);
    }

    let rest_listener = tokio::net::TcpListener::bind(("0.0.0.0", cfg.rest_port)).await
        .unwrap_or_else(|e| { eprintln!("Error: cannot bind REST port {} — {}", cfg.rest_port, e); std::process::exit(1) });
    let mcp_listener  = tokio::net::TcpListener::bind(("0.0.0.0", cfg.mcp_port)).await
        .unwrap_or_else(|e| { eprintln!("Error: cannot bind MCP port {} — {}", cfg.mcp_port, e); std::process::exit(1) });

    let _ = tokio::join!(
        axum::serve(rest_listener, rest),
        axum::serve(mcp_listener, mcp),
    );
}

// ── REST handlers ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TaskFilters {
    status: Option<String>,
    role: Option<String>,
    agent_id: Option<String>,
}

async fn list_tasks_handler(
    State(s): State<Arc<AppState>>,
    Query(f): Query<TaskFilters>,
) -> impl IntoResponse {
    match s.db.list_tasks(f.status, f.role, f.agent_id).await {
        Ok(tasks) => Json(tasks).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_task_handler(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s.db.get_task_with_activity(&id).await {
        Ok(Some(t)) => Json(t).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct CreateTaskBody {
    title: String,
    description: Option<String>,
    priority: Option<String>,
    assigned_role: Option<String>,
    branch_name: Option<String>,
    tags: Option<Vec<String>>,
    dependencies: Option<Vec<String>>,
}

async fn create_task_handler(
    State(s): State<Arc<AppState>>,
    Json(body): Json<CreateTaskBody>,
) -> impl IntoResponse {
    let branch_name = body.branch_name
        .as_deref()
        .filter(|s| !s.is_empty());
    match s.db.create_task(
        None,
        &body.title,
        body.description.as_deref(),
        body.priority.as_deref().unwrap_or("medium"),
        body.assigned_role.as_deref(),
        branch_name,
        &body.tags.unwrap_or_default(),
        &body.dependencies.unwrap_or_default(),
    ).await {
        Ok(task) => {
            s.broadcaster.broadcast(
                serde_json::json!({"event":"task_created","data":task}).to_string()
            );
            s.metrics.task_created();
            (StatusCode::CREATED, Json(task)).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct UpdateTaskBody {
    title: Option<String>,
    description: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    assigned_role: Option<String>,
    assigned_agent_id: Option<String>,
    #[serde(default)]
    clear_agent: bool,
    output_path: Option<String>,
    tags: Option<Vec<String>>,
    branch_name: Option<String>,
    base_branch: Option<String>,
    latest_commit: Option<String>,
    pr_url: Option<String>,
    worktree_path: Option<String>,
    dependencies: Option<Vec<String>>,
}

async fn update_task_handler(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateTaskBody>,
) -> impl IntoResponse {
    // Pre-fetch only when status is changing so we can emit from/to metric.
    let prev_status = if body.status.is_some() {
        s.db.get_task(&id).await.ok().flatten().map(|t| t.status)
    } else {
        None
    };

    match s.db.update_task(
        &id,
        body.title.as_deref(),
        body.description.as_deref(),
        body.status.as_deref(),
        body.priority.as_deref(),
        body.assigned_role.as_deref(),
        if body.clear_agent { Some("") } else { body.assigned_agent_id.as_deref() },
        body.output_path.as_deref(),
        body.tags.as_deref(),
        body.branch_name.as_deref(),
        body.base_branch.as_deref(),
        body.latest_commit.as_deref(),
        body.pr_url.as_deref(),
        body.worktree_path.as_deref(),
        body.dependencies.as_deref(),
    ).await {
        Ok(Some(task)) => {
            s.broadcaster.broadcast(
                serde_json::json!({"event":"task_updated","data":task}).to_string()
            );
            if let (Some(ref from), Some(ref to)) = (&prev_status, &body.status) {
                if from != to {
                    s.metrics.task_status_changed(from, to);
                }
            }
            if body.status.as_deref() == Some("done") {
                s.metrics.task_done(&task.title, &task.created_at);
                if let (Some(ref claimed_at), Some(ref role)) = (&task.claimed_at, &task.assigned_role) {
                    if let Ok(claimed) = chrono::DateTime::parse_from_rfc3339(claimed_at) {
                        let secs = chrono::Utc::now()
                            .signed_duration_since(claimed.with_timezone(&chrono::Utc))
                            .num_seconds();
                        if secs >= 0 {
                            s.metrics.task_claim_duration_seconds(role, secs as f64);
                        }
                    }
                }
            }
            if let Ok(counts) = s.db.count_tasks_by_status().await {
                s.metrics.queue_sizes(&counts);
            }
            // Auto-merge when task is marked done via REST
            if body.status.as_deref() == Some("done") {
                if let (Some(ref repo_path), Some(ref branch)) = (&s.repo_path, &task.branch_name) {
                    let base = s.base_branch.clone();
                    let merge_msg = format!("Merge '{}': {}", branch, task.title);
                    match git::merge_branch(repo_path, branch, &base, &merge_msg).await {
                        Ok(hash) => {
                            tracing::info!("Auto-merged {} into {} ({})", branch, base, &hash[..hash.len().min(8)]);
                            let _ = s.db.add_activity(&id, None, None, "merged",
                                Some(&format!("Merged '{}' into '{}' ({})", branch, base, &hash[..hash.len().min(8)])))
                                .await;
                            git::cleanup_task_worktrees(repo_path, branch).await;
                        }
                        Err(e) => {
                            tracing::warn!("Auto-merge failed for task {}: {}", id, e);
                            let _ = s.db.update_task(
                                &id, None, None, Some("blocked"), None, None, None, None, None,
                                None, None, None, None, None, None,
                            ).await;
                            let _ = s.db.add_activity(&id, None, None, "merge_failed",
                                Some(&format!("Auto-merge failed: {}", e)))
                                .await;
                            return (StatusCode::CONFLICT, e).into_response();
                        }
                    }
                }
            }
            Json(task).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn delete_task_handler(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s.db.delete_task(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn list_activity_handler(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    match s.db.list_activity(100).await {
        Ok(entries) => Json(entries).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn list_agents_handler(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    match s.db.list_agents().await {
        Ok(agents) => Json(agents).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn list_commits_handler(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s.db.list_commits_for_task(&id).await {
        Ok(commits) => Json(commits).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn stats_handler(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    match s.db.get_stats().await {
        Ok(stats) => Json(stats).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn reap_tasks_handler(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    match s.db.reap_stale_tasks(10).await {
        Ok(ids) => {
            for id in &ids {
                s.broadcaster.broadcast(
                    serde_json::json!({"event":"task_reaped","data":{"task_id":id}}).to_string()
                );
            }
            Json(serde_json::json!({"reaped": ids})).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn stop_agent_handler(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s.db.set_agent_stop(&id, true).await {
        Ok(Some(agent)) => {
            s.broadcaster.broadcast(
                serde_json::json!({"event":"agent_updated","data":agent}).to_string()
            );
            Json(agent).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn unstop_agent_handler(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s.db.set_agent_stop(&id, false).await {
        Ok(Some(agent)) => {
            s.broadcaster.broadcast(
                serde_json::json!({"event":"agent_updated","data":agent}).to_string()
            );
            Json(agent).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn reset_task_handler(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match s.db.reset_task(&id).await {
        Ok(Some(task)) => {
            s.broadcaster.broadcast(
                serde_json::json!({"event":"task_updated","data":task}).to_string()
            );
            Json(task).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn info_handler(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let pipeline: Vec<serde_json::Value> = s.config.as_deref()
        .map(|c| c.agents.iter().map(|a| serde_json::json!({
            "role": a.role,
            "transition": { "success": a.transition.success, "failure": a.transition.failure }
        })).collect())
        .unwrap_or_else(|| vec![
            serde_json::json!({"role": "coder",       "transition": {"success": "reviewer",    "failure": null}}),
            serde_json::json!({"role": "reviewer",    "transition": {"success": "tester",      "failure": "coder"}}),
            serde_json::json!({"role": "tester",      "transition": {"success": "docs_writer", "failure": "coder"}}),
            serde_json::json!({"role": "docs_writer", "transition": {"success": "done",        "failure": null}}),
        ]);
    Json(serde_json::json!({
        "repo_path": s.repo_path,
        "base_branch": s.base_branch,
        "pipeline": pipeline,
    }))
}

async fn agent_log_handler(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let Some(ref log_dir) = s.log_dir else {
        return Json(Vec::<String>::new()).into_response();
    };
    // Sanitise: only allow alphanumerics, hyphens, underscores in agent_id
    if !id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return (StatusCode::BAD_REQUEST, "invalid agent id").into_response();
    }
    let path = log_dir.join(format!("{}.log", id));
    match tokio::fs::read_to_string(&path).await {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            Json(lines).into_response()
        }
        Err(_) => Json(Vec::<String>::new()).into_response(),
    }
}
