use axum::extract::{MatchedPath, Path};
use axum::routing::{delete, get, put};
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use heed::{types::Str, Env};
use heed::{Database, EnvOpenOptions};
use hyper::Request;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::sync::Arc;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::trace::TraceLayer;
use tracing::info_span;

struct AppState {
    kv_env: Env,
    kv: Database<Str, Str>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let addr = std::env::var("SOCKET_ADDRESS").unwrap_or_else(|_| String::from("0.0.0.0:3000"));

    tracing::info!("listening on {}", addr);

    // Run with hyper
    axum::Server::bind(&addr.parse().unwrap())
        .serve(app().into_make_service())
        .await
        .unwrap();
}

fn app() -> Router {
    let db_path = std::env::var("DB_PATH").unwrap_or_else(|_| String::from("db/heed.mdb"));

    // Create dir
    fs::create_dir_all(&db_path).unwrap();

    // Create env
    let env = EnvOpenOptions::new().open(&db_path).unwrap();

    // We will open the default unamed database
    let kv: Database<Str, Str> = env.create_database(None).unwrap();

    // Create shared state to pass around the db ref
    let shared_state = Arc::new(AppState { kv_env: env, kv });

    Router::<Arc<AppState>>::new()
        // GET /
        .route("/", get(get_all))
        // GET /:key
        .route("/:key", get(get_key))
        // POST /
        .route("/", post(create_key))
        // PUT /:key
        .route("/:key", put(update_key))
        // DELETE /
        .route("/", delete(delete_all))
        // DELETE /:key
        .route("/:key", delete(delete_key))
        // Add panic recovery
        .layer(CatchPanicLayer::new())
        // Add tracing middleware
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                // Log the matched route's path (with placeholders not filled in).
                // Use request.uri() or OriginalUri if you want the real path.
                let matched_path = request
                    .extensions()
                    .get::<MatchedPath>()
                    .map(MatchedPath::as_str);

                info_span!(
                    "http_request",
                    method = ?request.method(),
                    matched_path
                )
            }),
        )
        // Add shared state
        .with_state(shared_state)
}

async fn get_all(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rtxn = state.kv_env.read_txn().unwrap();
    let values = state.kv.iter(&rtxn).unwrap();

    let ok_values: Vec<_> = values.filter_map(Result::ok).collect();

    (StatusCode::OK, Json(json!(ok_values)))
}

async fn get_key(State(state): State<Arc<AppState>>, Path(key): Path<String>) -> impl IntoResponse {
    let rtxn = state.kv_env.read_txn().unwrap();

    let value = state.kv.get(&rtxn, &key);

    match value {
        Ok(Some(value)) => (StatusCode::OK, Json(json!({ "key": key, "value": value }))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Key not found" })),
        ),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Internal server error" })),
        ),
    }
}

#[derive(Serialize, Deserialize)]
struct KVPayload {
    key: String,
    value: String,
}

async fn create_key(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<KVPayload>,
) -> impl IntoResponse {
    let rtxn = state.kv_env.read_txn().unwrap();

    let value = state.kv.get(&rtxn, &payload.key);

    // Check if the key already exists
    if let Ok(Some(_)) = value {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Key already exists" })),
        );
    }

    // If an error occurs during the retrieval process
    if let Err(_) = value {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Internal server error" })),
        );
    }

    let mut wtxn = state.kv_env.write_txn().unwrap();

    state
        .kv
        .put(&mut wtxn, &payload.key, &payload.value)
        .unwrap();

    wtxn.commit().unwrap();

    (
        StatusCode::CREATED,
        Json(json!({ "key": payload.key, "value": payload.value })),
    )
}

async fn update_key(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
    Json(payload): Json<KVPayload>,
) -> impl IntoResponse {
    let mut wtxn = state.kv_env.write_txn().unwrap();

    let value = state.kv.put(&mut wtxn, &key, &payload.value);

    match value {
        Ok(_) => {
            wtxn.commit().unwrap();

            (
                StatusCode::OK,
                Json(json!({ "key": key, "value": payload.value })),
            )
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Internal server error" })),
        ),
    }
}

async fn delete_all(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut wtxn = state.kv_env.write_txn().unwrap();

    state.kv.clear(&mut wtxn).unwrap();

    wtxn.commit().unwrap();

    StatusCode::OK
}

async fn delete_key(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let mut wtxn = state.kv_env.write_txn().unwrap();

    let value = state.kv.delete(&mut wtxn, &key);

    match value {
        Ok(true) => {
            wtxn.commit().unwrap();

            (StatusCode::OK, Json(json!({ "key": key })))
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Key not found" })),
        ),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Internal server error" })),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{self, Request, StatusCode},
    };
    use serde_json::{json, Value};
    use tower::Service; // for `call`
    use tower::ServiceExt; // for `oneshot` and `ready`

    async fn setup_tests() -> Router {
        // set env var to use a different db
        std::env::set_var("DB_PATH", "db/heed_test.mdb");

        let mut app = app();

        // Ensure db is cleared
        let request = Request::builder()
            .uri("/")
            .method(http::Method::DELETE)
            .body(Body::empty())
            .unwrap();

        app.ready().await.unwrap().call(request).await.unwrap();

        app
    }

    // You can use `ready()` and `call()` to avoid using `clone()`
    // in multiple request
    #[tokio::test]
    async fn get_all() {
        let mut app = setup_tests().await;

        let request = Request::builder().uri("/").body(Body::empty()).unwrap();
        let response = app.ready().await.unwrap().call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn get_missing_key() {
        let mut app = setup_tests().await;

        let request = Request::builder()
            .uri("/this-shouldn't-be-found")
            .body(Body::empty())
            .unwrap();
        let response = app.ready().await.unwrap().call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn create_and_get_existing_key() {
        let mut app = setup_tests().await;

        let insert_body = json!({"key": "foo", "value": "bar"});

        // Create key
        let request = Request::builder()
            .method(http::Method::POST)
            .uri("/")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(insert_body.to_string()))
            .unwrap();

        app.ready().await.unwrap().call(request).await.unwrap();

        // Get key
        let request = Request::builder().uri("/foo").body(Body::empty()).unwrap();
        let response = app.ready().await.unwrap().call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(body, insert_body)
    }

    #[tokio::test]
    async fn create_key() {
        let mut app = setup_tests().await;

        let request = Request::builder()
            .method(http::Method::POST)
            .uri("/")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"key": "foo", "value": "bar"}).to_string(),
            ))
            .unwrap();

        let response = app.ready().await.unwrap().call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn create_duplicate() {
        let mut app = setup_tests().await;

        let insert_body = json!({"key": "foo", "value": "bar"});

        // Create key
        let request = Request::builder()
            .method(http::Method::POST)
            .uri("/")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(insert_body.to_string()))
            .unwrap();

        app.ready().await.unwrap().call(request).await.unwrap();

        // Create duplicate key
        let request = Request::builder()
            .method(http::Method::POST)
            .uri("/")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(insert_body.to_string()))
            .unwrap();

        let response = app.ready().await.unwrap().call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn update_key() {
        let mut app = setup_tests().await;

        let insert_body = json!({"key": "foo", "value": "bar"});

        // Create key
        let request = Request::builder()
            .method(http::Method::POST)
            .uri("/")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(insert_body.to_string()))
            .unwrap();

        app.ready().await.unwrap().call(request).await.unwrap();

        // Update key
        let update_body = json!({"key": "foo", "value": "baz"});

        let request = Request::builder()
            .method(http::Method::PUT)
            .uri("/foo")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(update_body.to_string()))
            .unwrap();

        let response = app.ready().await.unwrap().call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Get key
        let request = Request::builder().uri("/foo").body(Body::empty()).unwrap();
        let response = app.ready().await.unwrap().call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(body, update_body)
    }

    #[tokio::test]
    async fn delete_all() {
        let mut app = app();

        let request = Request::builder()
            .method(http::Method::DELETE)
            .uri("/")
            .body(Body::empty())
            .unwrap();

        let response = app.ready().await.unwrap().call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let request = Request::builder().uri("/").body(Body::empty()).unwrap();
        let response = app.ready().await.unwrap().call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(body, json!([]));
    }

    #[tokio::test]
    async fn delete_key() {
        let mut app = setup_tests().await;

        // Create key
        let request = Request::builder()
            .method(http::Method::POST)
            .uri("/")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"key": "foo", "value": "bar"}).to_string(),
            ))
            .unwrap();

        app.ready().await.unwrap().call(request).await.unwrap();

        // Delete key
        let request = Request::builder()
            .method(http::Method::DELETE)
            .uri("/foo")
            .body(Body::empty())
            .unwrap();

        let response = app.ready().await.unwrap().call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        // Get key
        let request = Request::builder().uri("/foo").body(Body::empty()).unwrap();
        let response = app.ready().await.unwrap().call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
