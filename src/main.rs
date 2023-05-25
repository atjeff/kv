use axum::extract::Path;
use axum::routing::get;
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use heed::{types::Str, Env};
use heed::{Database, EnvOpenOptions};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::Path as StdPath;
use std::sync::Arc;

struct AppState {
    kv_env: Env,
    kv: Database<Str, Str>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // Create dir
    fs::create_dir_all(StdPath::new("target").join("heed.mdb")).unwrap();

    // Create env
    let env = EnvOpenOptions::new()
        .open(StdPath::new("target").join("heed.mdb"))
        .unwrap();

    // We will open the default unamed database
    let kv: Database<Str, Str> = env.create_database(None).unwrap();

    // Create shared state to pass around the db ref
    let shared_state = Arc::new(AppState { kv_env: env, kv });

    // Build our application with a route
    let app = Router::new()
        .route("/", get(get_all))
        .route("/:key", get(get_key))
        .route("/", post(create_key))
        .with_state(shared_state);

    let addr = &"0.0.0.0:3000".parse().unwrap();

    tracing::info!("listening on {}", addr);

    // Run with hyper
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn get_all(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rtxn = state.kv_env.read_txn().unwrap();
    let values = state.kv.iter(&rtxn).unwrap();

    let ok_values: Vec<_> = values.filter_map(Result::ok).collect();

    (StatusCode::OK, Json(json!(ok_values)))
}

async fn get_key(State(state): State<Arc<AppState>>, Path(key): Path<String>) -> impl IntoResponse {
    let rtxn = state.kv_env.read_txn().unwrap();

    let value = state.kv.get(&rtxn, &key).unwrap().unwrap().to_string();

    (StatusCode::OK, Json(KVPayload { key, value }))
}

async fn create_key(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<KVPayload>,
) -> impl IntoResponse {
    let mut wtxn = state.kv_env.write_txn().unwrap();

    state
        .kv
        .put(&mut wtxn, &payload.key, &payload.value)
        .unwrap();

    wtxn.commit().unwrap();

    StatusCode::CREATED
}

#[derive(Serialize, Deserialize)]
struct KVPayload {
    key: String,
    value: String,
}
