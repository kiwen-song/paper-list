mod handlers;
mod models;
mod storage;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::response::{Html, IntoResponse};
use axum::routing::{delete, get, post, put};
use handlers::AppState;
use std::fs;

const INDEX_HTML: &str = include_str!("../public/index.html");
const ADMIN_HTML: &str = include_str!("../public/admin.html");

async fn index_page() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn admin_page() -> Html<&'static str> {
    Html(ADMIN_HTML)
}

async fn fallback_page() -> impl IntoResponse {
    Html(INDEX_HTML)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(storage::DATA_DIR)?;

    {
        let config = storage::load_config();
        storage::save_config(&config)?;
    }

    {
        let mut meta = storage::load_meta();
        storage::migrate_legacy_awards(&mut meta);
    }

    let state = AppState::default();

    let app = Router::new()
        .route("/", get(index_page))
        .route("/index.html", get(index_page))
        .route("/admin.html", get(admin_page))
        .route("/api/login", post(handlers::login))
        .route("/api/logout", post(handlers::logout))
        .route("/api/auth", get(handlers::auth))
        .route("/api/change-password", post(handlers::change_password))
        .route("/api/settings", get(handlers::settings))
        .route("/api/settings", put(handlers::update_settings))
        .route("/api/stats", get(handlers::stats))
        .route(
            "/api/competitions",
            get(handlers::competitions).post(handlers::create_competition),
        )
        .route(
            "/api/competitions/order",
            put(handlers::reorder_competitions),
        )
        .route("/api/competitions/bulk-delete", post(handlers::bulk_delete))
        .route(
            "/api/competitions/{name}",
            delete(handlers::delete_competition),
        )
        .route("/api/competitions/{name}/pdf", get(handlers::pdf))
        .route("/api/competitions/{name}/download", get(handlers::download))
        .route("/api/competitions/{name}/files", get(handlers::list_files))
        .route(
            "/api/competitions/{name}/files/{*path}",
            get(handlers::download_file)
                .delete(handlers::delete_file)
                .put(handlers::rename_file),
        )
        .route(
            "/api/competitions/{name}/status",
            put(handlers::update_status),
        )
        .route(
            "/api/competitions/{name}/tags",
            post(handlers::add_tag).delete(handlers::remove_tag),
        )
        .route("/api/competitions/{name}/upload", post(handlers::upload))
        .route("/api/backup/metadata", get(handlers::backup_metadata))
        .route("/api/backup/metadata", post(handlers::restore_metadata))
        .fallback(get(fallback_page))
        .layer(DefaultBodyLimit::max(100 * 1024 * 1024))
        .with_state(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| storage::PORT.to_string());
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("Server running at http://localhost:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}
