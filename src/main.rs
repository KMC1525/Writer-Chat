mod chat;
mod config;
mod files;
mod store;

use axum::{
    extract::{DefaultBodyLimit, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use config::Config;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub http: reqwest::Client,
}

const INDEX_HTML: &str = include_str!("../web/index.html");
const STYLE_CSS: &str = include_str!("../web/style.css");
const APP_JS: &str = include_str!("../web/app.js");

fn asset(body: &'static str, ctype: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(ctype)),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
        ],
        body,
    )
        .into_response()
}

/// Presets de tamanho da resposta, em palavras. 0 = automático (o modelo decide).
fn length_presets() -> Value {
    json!([
        { "id": "auto",   "label": "Automático",   "words": 0 },
        { "id": "short",  "label": "Curta",        "words": 150 },
        { "id": "medium", "label": "Média",        "words": 400 },
        { "id": "long",   "label": "Longa",        "words": 800 },
        { "id": "xlong",  "label": "Muito longa",  "words": 1500 },
        { "id": "epic",   "label": "Capítulo",     "words": 3000 }
    ])
}

async fn api_config(State(st): State<AppState>) -> Json<Value> {
    Json(json!({
        "model": st.cfg.model,
        "quant": st.cfg.quant,
        "backend_url": st.cfg.backend_url,
        "default_system": chat::DEFAULT_SYSTEM,
        "length_presets": length_presets(),
        "max_attach_chars": st.cfg.max_attach_chars,
    }))
}

/// Lista modelos do servidor de inferência e indica se ele está acessível.
async fn api_models(State(st): State<AppState>) -> Json<Value> {
    let mut rb = st.http.get(format!("{}/v1/models", st.cfg.backend_url)).timeout(Duration::from_secs(4));
    if let Some(k) = &st.cfg.api_key {
        rb = rb.bearer_auth(k);
    }
    match rb.send().await {
        Ok(r) if r.status().is_success() => {
            let v: Value = r.json().await.unwrap_or(Value::Null);
            let ids: Vec<String> = v["data"]
                .as_array()
                .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            Json(json!({ "online": true, "models": ids }))
        }
        Ok(r) => Json(json!({ "online": false, "models": [], "error": format!("HTTP {}", r.status()) })),
        Err(e) => Json(json!({ "online": false, "models": [], "error": e.to_string() })),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cfg = Config::from_env();
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("cliente HTTP");
    let addr = cfg.addr;
    tracing::info!("Writer-Chat → http://{addr}");
    tracing::info!("modelo: {} ({}) via {}", cfg.model, cfg.quant, cfg.backend_url);
    let st = AppState { cfg: Arc::new(cfg), http };

    let app = Router::new()
        .route("/", get(|| async { asset(INDEX_HTML, "text/html; charset=utf-8") }))
        .route("/style.css", get(|| async { asset(STYLE_CSS, "text/css; charset=utf-8") }))
        .route("/app.js", get(|| async { asset(APP_JS, "text/javascript; charset=utf-8") }))
        .route("/api/config", get(api_config))
        .route("/api/models", get(api_models))
        .route("/api/chat", post(chat::chat))
        .route("/api/upload", post(files::upload))
        .route("/api/conversations", get(store::list))
        .route(
            "/api/conversations/{id}",
            get(store::get).put(store::put).delete(store::delete),
        )
        .layer(DefaultBodyLimit::max(200 * 1024 * 1024))
        .with_state(st);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap_or_else(|e| {
        eprintln!("não foi possível escutar em {addr}: {e}");
        std::process::exit(1);
    });
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .unwrap();
}
