use crate::{engine::Engine, model::*, scenarios};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{Mutex, RwLock};
#[derive(rust_embed::RustEmbed)]
#[folder = "frontend/dist/"]
struct Assets;
#[derive(Clone)]
struct App {
    engine: Arc<RwLock<Arc<Engine>>>,
    token: String,
    host: String,
    executable: PathBuf,
    reset: Arc<Mutex<()>>,
}
type ApiResult = Result<Json<Value>, (StatusCode, String)>;
fn bad(e: impl ToString) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, e.to_string())
}
async fn guard(State(app): State<App>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let origin = format!("http://{}", app.host);
    if headers.get(header::HOST).and_then(|h| h.to_str().ok()) != Some(app.host.as_str())
        || headers
            .get(header::ORIGIN)
            .is_some_and(|h| h.to_str().ok() != Some(origin.as_str()))
    {
        return (StatusCode::FORBIDDEN, "Host or Origin rejected").into_response();
    }
    if req.uri().path().starts_with("/api/")
        && headers
            .get("x-txnstage-token")
            .and_then(|h| h.to_str().ok())
            != Some(app.token.as_str())
    {
        return (StatusCode::UNAUTHORIZED, "Launch token required").into_response();
    }
    let mut response = next.run(req).await;
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    h.insert("x-content-type-options", "nosniff".parse().unwrap());
    h.insert("referrer-policy", "no-referrer".parse().unwrap());
    h.insert("content-security-policy","default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'".parse().unwrap());
    response
}
async fn state(State(app): State<App>) -> Json<Value> {
    let engine = app.engine.read().await.clone();
    Json(engine.snapshot().await)
}
async fn presets() -> Json<Vec<Scenario>> {
    Json(scenarios::builtins())
}
async fn execute(State(app): State<App>, Json(op): Json<Operation>) -> ApiResult {
    let engine = app.engine.read().await.clone();
    engine.submit(op).await.map_err(bad)?;
    Ok(Json(json!({"accepted":true})))
}
async fn cancel(State(app): State<App>, Path(connection): Path<String>) -> ApiResult {
    let e = app.engine.read().await.clone();
    if connection == "all" {
        e.stop().await;
    } else {
        e.cancel(&connection, "cancelled").await;
    }
    Ok(Json(json!({"cancelled":true})))
}
async fn fresh(app: &App, scenario: Scenario) -> Result<Arc<Engine>, (StatusCode, String)> {
    scenario.validate().map_err(bad)?;
    let _guard = app.reset.lock().await;
    let old = app.engine.read().await.clone();
    old.stop().await;
    let engine = Engine::new(scenario, app.executable.clone())
        .await
        .map_err(bad)?;
    *app.engine.write().await = engine.clone();
    Ok(engine)
}
async fn reset(State(app): State<App>, Json(scenario): Json<Scenario>) -> ApiResult {
    fresh(&app, scenario).await?;
    Ok(Json(json!({"reset":true})))
}
async fn replay(State(app): State<App>, Json(scenario): Json<Scenario>) -> ApiResult {
    let e = fresh(&app, scenario).await?;
    e.replay().await.map_err(bad)?;
    Ok(Json(json!({"replayed":true})))
}
async fn recording(State(app): State<App>) -> Result<Json<Recording>, (StatusCode, String)> {
    let engine = app.engine.read().await.clone();
    let recording = engine.recording().await;
    recording.validate().map_err(bad)?;
    Ok(Json(recording))
}
async fn validate(Json(scenario): Json<Scenario>) -> Result<Json<Scenario>, (StatusCode, String)> {
    scenario.validate().map_err(bad)?;
    Ok(Json(scenario))
}
async fn inspect(Json(record): Json<Recording>) -> ApiResult {
    record.validate().map_err(bad)?;
    let current = environment();
    let mut differences = Vec::new();
    for key in [
        "sqlite_version",
        "sqlite_source_id",
        "os",
        "arch",
        "txnstage",
    ] {
        if current[key] != record.environment[key] {
            differences.push(
                json!({"field":key,"recorded":record.environment[key],"current":current[key]}),
            );
        }
    }
    Ok(Json(json!({"recording":record,"differences":differences})))
}
async fn asset(req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let Some(file) = Assets::get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") {
        "application/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    };
    ([(header::CONTENT_TYPE, mime)], file.data.into_owned()).into_response()
}
pub async fn serve(port: u16, executable: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let host = listener.local_addr()?.to_string();
    let token = uuid::Uuid::new_v4().to_string();
    let engine = Engine::new(scenarios::builtins().remove(0), executable.clone()).await?;
    let app = App {
        engine: Arc::new(RwLock::new(engine)),
        token: token.clone(),
        host: host.clone(),
        executable,
        reset: Arc::new(Mutex::new(())),
    };
    let routes = Router::new()
        .route("/api/state", get(state))
        .route("/api/scenarios", get(presets))
        .route("/api/execute", post(execute))
        .route("/api/cancel/{connection}", post(cancel))
        .route("/api/reset", post(reset))
        .route("/api/replay", post(replay))
        .route("/api/recording", get(recording))
        .route("/api/inspect", post(inspect))
        .route("/api/validate", post(validate))
        .fallback(asset)
        .layer(DefaultBodyLimit::max(MAX_ARTIFACT))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app.clone());
    println!(
        "TxnStage v{}\nhttp://{host}/#token={token}\nDisposable databases only. Press Ctrl+C to stop and clean up.",
        env!("CARGO_PKG_VERSION")
    );
    let shutdown_app = app.clone();
    axum::serve(listener, routes)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            let e = shutdown_app.engine.read().await.clone();
            e.stop().await;
        })
        .await?;
    let e = app.engine.read().await.clone();
    e.shutdown().await;
    Ok(())
}
