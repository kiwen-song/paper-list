use crate::models::{CompMeta, Tag};
use crate::storage;
use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::header::{self, HeaderMap, HeaderValue};
use axum::http::{Response, StatusCode};
use rand::RngCore;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

#[derive(Clone, Default)]
pub struct AppState {
    pub config_lock: Arc<Mutex<()>>,
    pub meta_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Deserialize)]
pub struct LoginBody {
    password: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordBody {
    #[serde(rename = "oldPassword")]
    old_password: String,
    #[serde(rename = "newPassword")]
    new_password: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateCompetitionBody {
    name: String,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BulkDeleteBody {
    names: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReorderBody {
    names: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct StatusBody {
    status: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoveTagBody {
    index: usize,
}

#[derive(Debug, Deserialize)]
pub struct SettingsBody {
    title: Option<String>,
    subtitle: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RenameBody {
    #[serde(rename = "newPath")]
    new_path: String,
}

pub async fn login(State(state): State<AppState>, Json(body): Json<LoginBody>) -> Response<Body> {
    let _guard = state.config_lock.lock().expect("config lock poisoned");
    let mut config = storage::load_config();
    if storage::hash_password(&body.password) != config.admin_password_hash {
        return json_error(StatusCode::UNAUTHORIZED, "wrong password");
    }

    config.session_token = generate_token();
    if storage::save_config(&config).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save");
    }

    let cookie = format!(
        "session={}; Path=/; Max-Age=2592000; HttpOnly; SameSite=Lax",
        config.session_token
    );
    let mut response = json(StatusCode::OK, serde_json::json!({ "ok": true }));
    response
        .headers_mut()
        .insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    response
}

pub async fn logout(State(state): State<AppState>) -> Response<Body> {
    let _guard = state.config_lock.lock().expect("config lock poisoned");
    let mut config = storage::load_config();
    config.session_token.clear();
    if storage::save_config(&config).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save");
    }

    let mut response = json(StatusCode::OK, serde_json::json!({ "ok": true }));
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("session=; Path=/; Max-Age=-1"),
    );
    response
}

pub async fn auth(State(state): State<AppState>, headers: HeaderMap) -> Response<Body> {
    json(
        StatusCode::OK,
        serde_json::json!({ "admin": is_admin(&state, &headers) }),
    )
}

pub async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ChangePasswordBody>,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if body.new_password.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "invalid");
    }

    let _guard = state.config_lock.lock().expect("config lock poisoned");
    let mut config = storage::load_config();
    if storage::hash_password(&body.old_password) != config.admin_password_hash {
        return json_error(StatusCode::UNAUTHORIZED, "wrong old password");
    }
    config.admin_password_hash = storage::hash_password(&body.new_password);
    config.session_token.clear();
    if storage::save_config(&config).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save");
    }

    let mut response = json(StatusCode::OK, serde_json::json!({ "ok": true }));
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("session=; Path=/; Max-Age=-1"),
    );
    response
}

pub async fn settings() -> Response<Body> {
    let config = storage::load_config();
    json(
        StatusCode::OK,
        serde_json::json!({
            "title": config.site_title,
            "subtitle": config.site_subtitle,
        }),
    )
}

pub async fn update_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SettingsBody>,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }

    let _guard = state.config_lock.lock().expect("config lock poisoned");
    let mut config = storage::load_config();
    if let Some(title) = body.title.filter(|value| !value.is_empty()) {
        config.site_title = title;
    }
    if let Some(subtitle) = body.subtitle.filter(|value| !value.is_empty()) {
        config.site_subtitle = subtitle;
    }
    if storage::save_config(&config).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save");
    }
    ok()
}

pub async fn stats() -> Response<Body> {
    json(StatusCode::OK, storage::build_stats())
}

pub async fn competitions() -> Response<Body> {
    json(StatusCode::OK, storage::list_competitions())
}

pub async fn create_competition(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateCompetitionBody>,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if body.name.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "name required");
    }

    let Ok(dir) = storage::competition_dir(&body.name) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid name");
    };
    if dir.exists() {
        return json_error(StatusCode::CONFLICT, "already exists");
    }
    if fs::create_dir_all(&dir).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to create");
    }

    let _guard = state.meta_lock.lock().expect("meta lock poisoned");
    let mut meta = storage::load_meta();
    meta.insert(
        body.name,
        CompMeta {
            status: body.status.unwrap_or_else(|| "planned".to_string()),
            tags: Vec::new(),
            order: Some(meta.len()),
        },
    );
    if storage::save_meta(&meta).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save");
    }
    ok()
}

pub async fn delete_competition(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let Ok(dir) = storage::competition_dir(&name) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid name");
    };
    if !dir.exists() {
        return json_error(StatusCode::NOT_FOUND, "not found");
    }
    if fs::remove_dir_all(&dir).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to delete");
    }

    let _guard = state.meta_lock.lock().expect("meta lock poisoned");
    let mut meta = storage::load_meta();
    if meta.remove(&name).is_some() && storage::save_meta(&meta).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save");
    }
    ok()
}

pub async fn bulk_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BulkDeleteBody>,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if body.names.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "names required");
    }

    let _guard = state.meta_lock.lock().expect("meta lock poisoned");
    let mut meta = storage::load_meta();
    let mut meta_changed = false;

    for name in body.names {
        if let Ok(dir) = storage::competition_dir(&name)
            && dir.exists()
        {
            let _ = fs::remove_dir_all(dir);
        }
        if meta.remove(&name).is_some() {
            meta_changed = true;
        }
    }

    if meta_changed && storage::save_meta(&meta).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save");
    }
    ok()
}

pub async fn reorder_competitions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ReorderBody>,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if body.names.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "names required");
    }

    let existing: std::collections::HashSet<String> = storage::list_competitions()
        .into_iter()
        .map(|competition| competition.name)
        .collect();
    let mut seen = std::collections::HashSet::new();
    for name in &body.names {
        if !existing.contains(name) || !seen.insert(name.clone()) {
            return json_error(StatusCode::BAD_REQUEST, "invalid order");
        }
    }
    if seen.len() != existing.len() {
        return json_error(StatusCode::BAD_REQUEST, "incomplete order");
    }

    let _guard = state.meta_lock.lock().expect("meta lock poisoned");
    let mut meta = storage::load_meta();
    for (index, name) in body.names.iter().enumerate() {
        let comp = meta.entry(name.clone()).or_insert_with(|| CompMeta {
            status: "completed".to_string(),
            tags: Vec::new(),
            order: None,
        });
        comp.order = Some(index);
    }
    if storage::save_meta(&meta).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save");
    }
    ok()
}

pub async fn update_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<StatusBody>,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }

    let _guard = state.meta_lock.lock().expect("meta lock poisoned");
    let mut meta = storage::load_meta();
    let comp = meta.entry(name).or_insert_with(|| CompMeta {
        status: String::new(),
        tags: Vec::new(),
        order: None,
    });
    comp.status = body.status;
    if storage::save_meta(&meta).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save");
    }
    ok()
}

pub async fn add_tag(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(tag): Json<Tag>,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if tag.text.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "invalid");
    }

    let _guard = state.meta_lock.lock().expect("meta lock poisoned");
    let mut meta = storage::load_meta();
    let comp = meta.entry(name).or_insert_with(|| CompMeta {
        status: "completed".to_string(),
        tags: Vec::new(),
        order: None,
    });
    comp.tags.push(tag);
    if storage::save_meta(&meta).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save");
    }
    ok()
}

pub async fn remove_tag(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<RemoveTagBody>,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }

    let _guard = state.meta_lock.lock().expect("meta lock poisoned");
    let mut meta = storage::load_meta();
    let Some(comp) = meta.get_mut(&name) else {
        return json_error(StatusCode::BAD_REQUEST, "not found");
    };
    if body.index >= comp.tags.len() {
        return json_error(StatusCode::BAD_REQUEST, "not found");
    }
    comp.tags.remove(body.index);
    if storage::save_meta(&meta).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save");
    }
    ok()
}

pub async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    mut multipart: Multipart,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let Ok(dir) = storage::competition_dir(&name) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid name");
    };
    if !dir.exists() {
        return json_error(StatusCode::NOT_FOUND, "not found");
    }

    let mut payload = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            match field.bytes().await {
                Ok(bytes) => {
                    payload = Some(bytes.to_vec());
                    break;
                }
                Err(_) => return json_error(StatusCode::BAD_REQUEST, "no file"),
            }
        }
    }
    let Some(payload) = payload else {
        return json_error(StatusCode::BAD_REQUEST, "no file");
    };

    let cursor = Cursor::new(payload);
    let mut archive = match ZipArchive::new(cursor) {
        Ok(archive) => archive,
        Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid zip"),
    };

    for index in 0..archive.len() {
        let Ok(mut file) = archive.by_index(index) else {
            continue;
        };
        let Some(enclosed_name) = file.enclosed_name().map(|path| path.to_path_buf()) else {
            continue;
        };
        let rel_path = upload_target_rel_path(&enclosed_name);
        let Ok(target) = storage::competition_child_path(&name, &rel_path) else {
            continue;
        };

        if file.is_dir() {
            let _ = fs::create_dir_all(target);
            continue;
        }
        if let Some(parent) = target.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(mut out) = fs::File::create(target) {
            let _ = std::io::copy(&mut file, &mut out);
        }
    }
    ok()
}

pub async fn pdf(Path(name): Path<String>) -> Response<Body> {
    let Ok(path) = storage::competition_child(&name, "thesis.pdf") else {
        return json_error(StatusCode::BAD_REQUEST, "invalid name");
    };
    if !path.exists() {
        return json_error(StatusCode::NOT_FOUND, "thesis.pdf not found");
    }
    file_response(path, "application/pdf", None)
}

pub async fn download(Path(name): Path<String>) -> Response<Body> {
    let Ok(dir) = storage::competition_dir(&name) else {
        return json_error(StatusCode::BAD_REQUEST, "invalid name");
    };
    if !dir.exists() {
        return json_error(StatusCode::NOT_FOUND, "Competition not found");
    }

    let mut buffer = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buffer);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for entry in walkdir::WalkDir::new(&dir)
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            let Ok(rel) = path.strip_prefix(storage::DATA_DIR) else {
                continue;
            };
            let rel_name = storage::path_to_slash(rel);
            if rel_name.is_empty() {
                continue;
            }

            if entry.file_type().is_dir() {
                let _ = zip.add_directory(format!("{rel_name}/"), options);
                continue;
            }

            let Ok(mut file) = fs::File::open(path) else {
                continue;
            };
            if zip.start_file(rel_name, options).is_ok() {
                let _ = std::io::copy(&mut file, &mut zip);
            }
        }
        if zip.finish().is_err() {
            return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to zip");
        }
    }

    let zip_name = format!("{name}.zip");
    let ascii_name = format!("competition-{}.zip", hex::encode(name.as_bytes()));
    let disposition = format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        ascii_name,
        urlencoding::encode(&zip_name)
    );
    bytes_response(
        buffer.into_inner(),
        "application/zip",
        Some(disposition.as_str()),
    )
}

pub async fn list_files(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    match storage::list_files(&name) {
        Ok(files) => json(StatusCode::OK, files),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            json_error(StatusCode::NOT_FOUND, "not found")
        }
        Err(_) => json_error(StatusCode::BAD_REQUEST, "invalid path"),
    }
}

pub async fn download_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((name, path)): Path<(String, String)>,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let Ok(target) = storage::competition_child(&name, &path) else {
        return json_error(StatusCode::BAD_REQUEST, "path outside competition");
    };
    let Ok(metadata) = target.metadata() else {
        return json_error(StatusCode::NOT_FOUND, "file not found");
    };
    if metadata.is_dir() {
        return json_error(StatusCode::NOT_FOUND, "file not found");
    }

    let file_name = target
        .file_name()
        .map(|part| part.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    let ascii_name = format!("file-{}", hex::encode(file_name.as_bytes()));
    let disposition = format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        ascii_name,
        urlencoding::encode(&file_name)
    );
    file_response(target, "application/octet-stream", Some(disposition))
}

pub async fn delete_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((name, path)): Path<(String, String)>,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let Ok(target) = storage::competition_child(&name, &path) else {
        return json_error(StatusCode::BAD_REQUEST, "path outside competition");
    };
    let result = if target.is_dir() {
        fs::remove_dir_all(target)
    } else if target.exists() {
        fs::remove_file(target)
    } else {
        Ok(())
    };
    if result.is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to delete");
    }
    ok()
}

pub async fn rename_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((name, path)): Path<(String, String)>,
    Json(body): Json<RenameBody>,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if body.new_path.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "newPath required");
    }

    let Ok(source) = storage::competition_child(&name, &path) else {
        return json_error(StatusCode::BAD_REQUEST, "path outside competition");
    };
    let Ok(target) = storage::competition_child(&name, &body.new_path) else {
        return json_error(StatusCode::BAD_REQUEST, "newPath outside competition");
    };
    if !source.exists() {
        return json_error(StatusCode::NOT_FOUND, "source not found");
    }
    if let Some(parent) = target.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to create parent");
    }
    if fs::rename(source, target).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to rename");
    }
    ok()
}

pub async fn backup_metadata(State(state): State<AppState>, headers: HeaderMap) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    match fs::read(storage::METADATA_FILE) {
        Ok(data) => bytes_response(
            data,
            "application/json",
            Some("attachment; filename=\"metadata.json\""),
        ),
        Err(_) => json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read metadata"),
    }
}

pub async fn restore_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response<Body> {
    if !is_admin(&state, &headers) {
        return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }

    let mut payload = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            match field.bytes().await {
                Ok(bytes) => {
                    payload = Some(bytes.to_vec());
                    break;
                }
                Err(_) => return json_error(StatusCode::BAD_REQUEST, "no file"),
            }
        }
    }
    let Some(payload) = payload else {
        return json_error(StatusCode::BAD_REQUEST, "no file");
    };

    let mut imported = match serde_json::from_slice::<HashMap<String, CompMeta>>(&payload) {
        Ok(imported) => imported,
        Err(_) => return json_error(StatusCode::BAD_REQUEST, "invalid json"),
    };
    for comp in imported.values_mut() {
        if comp.tags.is_empty() {
            comp.tags = Vec::new();
        }
    }

    let _guard = state.meta_lock.lock().expect("meta lock poisoned");
    if storage::save_meta(&imported).is_err() {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to save");
    }
    ok()
}

fn upload_target_rel_path(path: &FsPath) -> PathBuf {
    let mut components = path.components();
    let Some(first) = components.next() else {
        return PathBuf::new();
    };
    let first_path = PathBuf::from(first.as_os_str());
    let remaining = components.as_path();
    if !remaining.as_os_str().is_empty()
        && storage::competition_dir(&first_path.to_string_lossy())
            .map(|path| path.exists())
            .unwrap_or(false)
    {
        remaining.to_path_buf()
    } else {
        path.to_path_buf()
    }
}

fn is_admin(state: &AppState, headers: &HeaderMap) -> bool {
    let _guard = state.config_lock.lock().expect("config lock poisoned");
    let config = storage::load_config();
    let token = session_token(headers).unwrap_or_default();
    !token.is_empty() && !config.session_token.is_empty() && token == config.session_token
}

fn session_token(headers: &HeaderMap) -> Option<String> {
    if let Some(token) = headers
        .get("x-session")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
    {
        return Some(token.to_string());
    }

    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookie| {
            cookie.split(';').find_map(|part| {
                let trimmed = part.trim();
                trimmed
                    .strip_prefix("session=")
                    .map(|value| value.to_string())
            })
        })
}

fn generate_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn ok() -> Response<Body> {
    json(StatusCode::OK, serde_json::json!({ "ok": true }))
}

fn json_error(status: StatusCode, message: &str) -> Response<Body> {
    json(status, serde_json::json!({ "error": message }))
}

fn json<T: Serialize>(status: StatusCode, value: T) -> Response<Body> {
    let body = serde_json::to_vec(&value).expect("serializable response");
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("valid response")
}

fn bytes_response(data: Vec<u8>, content_type: &str, disposition: Option<&str>) -> Response<Body> {
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type);
    if let Some(disposition) = disposition {
        builder = builder.header(header::CONTENT_DISPOSITION, disposition);
    }
    builder.body(Body::from(data)).expect("valid response")
}

fn file_response(path: PathBuf, content_type: &str, disposition: Option<String>) -> Response<Body> {
    match fs::File::open(path) {
        Ok(mut file) => {
            let mut data = Vec::new();
            if file.read_to_end(&mut data).is_err() {
                return json_error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read file");
            }
            bytes_response(data, content_type, disposition.as_deref())
        }
        Err(_) => json_error(StatusCode::NOT_FOUND, "file not found"),
    }
}
