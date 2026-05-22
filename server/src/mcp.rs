use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    routing::{get, post},
    Json, Router,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{convert::Infallible, sync::Arc, time::Duration};

use crate::tools::{handle_tool_call, tool_definitions, ToolContext};
use crate::AppState;

const CURRENT_PROTOCOL_VERSION: &str = "2025-03-26";
const LEGACY_PROTOCOL_VERSION: &str = "2024-11-05";
const MCP_SESSION_HEADER: &str = "mcp-session-id";

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

enum DispatchOutcome {
    Response {
        response: JsonRpcResponse,
        session_id: Option<String>,
    },
    Accepted,
}

impl JsonRpcResponse {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn err(id: Option<Value>, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    fn err_with_data(
        id: Option<Value>,
        code: i64,
        message: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: Some(data),
            }),
        }
    }
}

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(mcp_stream_handler).post(handle_post))
        .route("/message", post(handle_post))
        .route("/sse", get(mcp_sse_handler))
        .with_state(state)
}

async fn handle_post(
    State(state): State<Arc<AppState>>,
    method: Method,
    headers: HeaderMap,
    Json(req): Json<JsonRpcRequest>,
) -> Response {
    if method != Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }

    match dispatch(req, &headers, &state).await {
        DispatchOutcome::Accepted => StatusCode::ACCEPTED.into_response(),
        DispatchOutcome::Response {
            response,
            session_id,
        } => {
            let mut reply_headers = HeaderMap::new();
            if let Some(session_id) = session_id {
                if let Ok(value) = HeaderValue::from_str(&session_id) {
                    reply_headers.insert(MCP_SESSION_HEADER, value);
                }
            }
            (reply_headers, Json(response)).into_response()
        }
    }
}

async fn dispatch(
    req: JsonRpcRequest,
    headers: &HeaderMap,
    state: &Arc<AppState>,
) -> DispatchOutcome {
    let id = req.id.clone();
    match req.method.as_str() {
        "initialize" => initialize(id, req.params, headers),
        "notifications/initialized" => DispatchOutcome::Accepted,
        "ping" => DispatchOutcome::Response {
            response: JsonRpcResponse::ok(id, json!({})),
            session_id: session_id_from_headers(headers),
        },
        "tools/list" => DispatchOutcome::Response {
            response: JsonRpcResponse::ok(id, json!({ "tools": tool_definitions() })),
            session_id: session_id_from_headers(headers),
        },
        "tools/call" => {
            let params = req.params.unwrap_or(json!({}));
            let name = match params["name"].as_str() {
                Some(name) => name.to_string(),
                None => {
                    return DispatchOutcome::Response {
                        response: JsonRpcResponse::err(id, -32602, "Missing tool name"),
                        session_id: session_id_from_headers(headers),
                    };
                }
            };
            let arguments = params["arguments"].clone();
            let ctx = ToolContext {
                db: state.db.clone(),
                broadcaster: state.broadcaster.clone(),
                repo_path: state.repo_path.clone(),
                base_branch: state.base_branch.clone(),
                metrics: state.metrics.clone(),
                config: state.config.clone(),
            };

            let response = match handle_tool_call(&name, arguments, &ctx).await {
                Ok(result) => JsonRpcResponse::ok(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": result.to_string()
                        }]
                    }),
                ),
                Err(error) => JsonRpcResponse::err(id, -32603, error),
            };

            DispatchOutcome::Response {
                response,
                session_id: session_id_from_headers(headers),
            }
        }
        _ => DispatchOutcome::Response {
            response: JsonRpcResponse::err(id, -32601, format!("Method not found: {}", req.method)),
            session_id: session_id_from_headers(headers),
        },
    }
}

fn initialize(
    id: Option<Value>,
    params: Option<Value>,
    headers: &HeaderMap,
) -> DispatchOutcome {
    let requested = params
        .as_ref()
        .and_then(|value| value.get("protocolVersion"))
        .and_then(Value::as_str)
        .unwrap_or(LEGACY_PROTOCOL_VERSION);

    let negotiated = negotiate_protocol_version(requested);
    if negotiated.is_none() {
        return DispatchOutcome::Response {
            response: JsonRpcResponse::err_with_data(
                id,
                -32602,
                "Unsupported protocol version",
                json!({
                    "supported": [CURRENT_PROTOCOL_VERSION, LEGACY_PROTOCOL_VERSION],
                    "requested": requested,
                }),
            ),
            session_id: None,
        };
    }

    let session_id = session_id_from_headers(headers)
        .unwrap_or_else(|| format!("mandatum-{}", uuid::Uuid::new_v4()));

    DispatchOutcome::Response {
        response: JsonRpcResponse::ok(
            id,
            json!({
                "protocolVersion": negotiated.unwrap(),
                "capabilities": {
                    "tools": { "listChanged": false }
                },
                "serverInfo": {
                    "name": "mandatum-task-tracker",
                    "version": "0.1.0"
                }
            }),
        ),
        session_id: Some(session_id),
    }
}

fn negotiate_protocol_version(requested: &str) -> Option<&'static str> {
    match requested {
        CURRENT_PROTOCOL_VERSION => Some(CURRENT_PROTOCOL_VERSION),
        LEGACY_PROTOCOL_VERSION => Some(LEGACY_PROTOCOL_VERSION),
        // Be tolerant of newer clients that only need tools over HTTP.
        version if version >= CURRENT_PROTOCOL_VERSION => Some(CURRENT_PROTOCOL_VERSION),
        _ => None,
    }
}

fn session_id_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(MCP_SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

async fn mcp_stream_handler() -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    mcp_sse_response(headers)
}

async fn mcp_sse_handler(
    State(_state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    sse_stream()
}

fn mcp_sse_response(headers: HeaderMap) -> Response {
    (headers, sse_stream()).into_response()
}

fn sse_stream() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        yield Ok::<Event, Infallible>(
            Event::default().event("endpoint").data(json!({"endpoint":"/message"}).to_string())
        );
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        loop {
            interval.tick().await;
            yield Ok::<Event, Infallible>(Event::default().event("ping").data("{}"));
        }
    };

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::Database, sse::SseBroadcaster, AppState};
    use axum::body::{to_bytes, Body};
    use tower::util::ServiceExt;

    async fn test_router() -> Router {
        let db_path = format!("/tmp/mandatum-mcp-test-{}.db", uuid::Uuid::new_v4());
        let state = Arc::new(AppState {
            db: Database::new(&db_path).await.expect("db should open"),
            broadcaster: SseBroadcaster::new(),
            repo_path: None,
            base_branch: "main".to_string(),
            metrics: Arc::new(crate::metrics::Metrics::new()),
            log_dir: None,
        });
        create_router(state)
    }

    #[tokio::test]
    async fn initialize_negotiates_current_protocol_and_sets_session_header() {
        let router = test_router().await;
        let request = axum::http::Request::builder()
            .method(Method::POST)
            .uri("/")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-03-26",
                        "capabilities": {},
                        "clientInfo": {
                            "name": "test-client",
                            "version": "1.0.0"
                        }
                    }
                })
                .to_string(),
            ))
            .expect("request should build");

        let response = router.oneshot(request).await.expect("request should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key(MCP_SESSION_HEADER));

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should collect");
        let payload: Value = serde_json::from_slice(&body).expect("body should be valid json");
        assert_eq!(payload["result"]["protocolVersion"], "2025-03-26");
    }

    #[tokio::test]
    async fn initialized_notification_returns_accepted_without_body() {
        let router = test_router().await;
        let request = axum::http::Request::builder()
            .method(Method::POST)
            .uri("/")
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .header(MCP_SESSION_HEADER, "mandatum-test-session")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized"
                })
                .to_string(),
            ))
            .expect("request should build");

        let response = router.oneshot(request).await.expect("request should succeed");

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should collect");
        assert!(body.is_empty());
    }
}
