use aes_gcm::{aead::Aead, aead::KeyInit, Aes256Gcm};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::cdp::client::CdpClient;
use super::cdp::types::{
    AttachToTargetParams, AttachToTargetResult, CloseTargetParams, CreateTargetParams,
    CreateTargetResult, EvaluateParams,
};
use super::cookies::{self, Cookie};
use crate::validation::{is_valid_session_name, sanitize_session_component, session_name_error};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageState {
    pub cookies: Vec<Cookie>,
    pub origins: Vec<OriginStorage>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginStorage {
    pub origin: String,
    pub local_storage: Vec<StorageEntry>,
    #[serde(default)]
    pub session_storage: Vec<StorageEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageEntry {
    pub name: String,
    pub value: String,
}

fn collect_frame_origins(tree: &Value, origins: &mut HashSet<String>) {
    if let Some(frame) = tree.get("frame") {
        if let Some(url_str) = frame.get("url").and_then(|v| v.as_str()) {
            if let Ok(parsed) = url::Url::parse(url_str) {
                let origin = parsed.origin().ascii_serialization();
                if origin != "null" && !origin.is_empty() {
                    origins.insert(origin);
                }
            }
        }
    }
    if let Some(children) = tree.get("childFrames").and_then(|v| v.as_array()) {
        for child in children {
            collect_frame_origins(child, origins);
        }
    }
}

/// Parse the JS-evaluated origin storage data into an OriginStorage struct.
fn parse_origin_storage(data: &Value) -> Option<OriginStorage> {
    if !data.is_object() {
        return None;
    }
    let origin = data
        .get("origin")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if origin.is_empty() || origin == "null" {
        return None;
    }
    let local_storage: Vec<StorageEntry> = data
        .get("localStorage")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let session_storage: Vec<StorageEntry> = data
        .get("sessionStorage")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    Some(OriginStorage {
        origin,
        local_storage,
        session_storage,
    })
}

fn parse_storage_capture(
    result: super::cdp::types::EvaluateResult,
    strict: bool,
) -> Result<Option<OriginStorage>, String> {
    let data = result.result.value.unwrap_or(Value::Null);
    if !strict {
        return Ok(parse_origin_storage(&data));
    }
    let capture_error = || "Failed to capture origin storage".to_string();
    if result.exception_details.is_some()
        || data.get("captureFailed").and_then(Value::as_bool) != Some(false)
    {
        return Err(capture_error());
    }
    let origin = data
        .get("origin")
        .and_then(Value::as_str)
        .ok_or_else(capture_error)?;
    let local_storage: Vec<StorageEntry> = serde_json::from_value(
        data.get("localStorage")
            .cloned()
            .ok_or_else(capture_error)?,
    )
    .map_err(|_| capture_error())?;
    let session_storage: Vec<StorageEntry> = serde_json::from_value(
        data.get("sessionStorage")
            .cloned()
            .ok_or_else(capture_error)?,
    )
    .map_err(|_| capture_error())?;
    if origin == "null" {
        return Ok(None);
    }
    if origin.is_empty() || url::Url::parse(origin).is_err() {
        return Err(capture_error());
    }
    Ok(Some(OriginStorage {
        origin: origin.to_string(),
        local_storage,
        session_storage,
    }))
}

fn storage_capture_result<T>(result: Result<T, String>, strict: bool) -> Result<Option<T>, String> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(_) if strict => Err("Failed to capture browser storage".to_string()),
        Err(_) => Ok(None),
    }
}

/// Evaluate storage without exposing page exception messages in capture errors.
async fn eval_origin_storage(
    client: &CdpClient,
    session_id: &str,
    origin_js: &str,
    strict: bool,
) -> Result<Option<OriginStorage>, String> {
    let result = client
        .send_command_typed::<_, super::cdp::types::EvaluateResult>(
            "Runtime.evaluate",
            &EvaluateParams {
                expression: origin_js.to_string(),
                return_by_value: Some(true),
                await_promise: Some(false),
            },
            Some(session_id),
        )
        .await;
    match storage_capture_result(result, strict)? {
        Some(result) => parse_storage_capture(result, strict),
        None => Ok(None),
    }
}

/// Create a temporary CDP target, navigate it to each origin to collect localStorage,
/// then close it. Uses Fetch interception to serve blank HTML instead of making real
/// network requests.
async fn collect_storage_via_temp_target(
    client: &CdpClient,
    origins: &[String],
    origin_js: &str,
    strict: bool,
) -> Result<Vec<OriginStorage>, String> {
    let create_result: CreateTargetResult = client
        .send_command_typed(
            "Target.createTarget",
            &CreateTargetParams {
                url: "about:blank".to_string(),
            },
            None,
        )
        .await?;

    let target_id = create_result.target_id;

    // Ensure the target is closed even if attach or later steps fail
    let result = collect_storage_in_target(client, &target_id, origins, origin_js, strict).await;

    let _ = client
        .send_command_typed::<_, Value>(
            "Target.closeTarget",
            &CloseTargetParams { target_id },
            None,
        )
        .await;

    result
}

async fn collect_storage_in_target(
    client: &CdpClient,
    target_id: &str,
    origins: &[String],
    origin_js: &str,
    strict: bool,
) -> Result<Vec<OriginStorage>, String> {
    let attach_result: AttachToTargetResult = client
        .send_command_typed(
            "Target.attachToTarget",
            &AttachToTargetParams {
                target_id: target_id.to_string(),
                flatten: true,
            },
            None,
        )
        .await?;

    let temp_session = &attach_result.session_id;

    client
        .send_command_no_params("Page.enable", Some(temp_session))
        .await?;
    client
        .send_command_no_params("Runtime.enable", Some(temp_session))
        .await?;

    // Blank HTML response body, pre-encoded to avoid repeated base64 work per request
    let blank_html_b64 = base64::engine::general_purpose::STANDARD.encode("<html></html>");

    let fetch_result = client
        .send_command(
            "Fetch.enable",
            Some(json!({ "patterns": [{ "urlPattern": "*" }] })),
            Some(temp_session),
        )
        .await;

    storage_capture_result(fetch_result, strict)?;

    let mut event_rx = client.subscribe();
    let mut results = Vec::new();

    for target_origin in origins {
        let nav_url = format!("{}/", target_origin.trim_end_matches('/'));
        let navigation = client
            .send_command(
                "Page.navigate",
                Some(json!({ "url": nav_url })),
                Some(temp_session),
            )
            .await;
        let Some(navigation) = storage_capture_result(navigation, strict)? else {
            continue;
        };
        if strict && navigation.get("errorText").is_some() {
            return Err("Failed to navigate storage capture target".to_string());
        }

        // Fulfill intercepted requests with blank HTML until the page loads
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
        let mut page_loaded = false;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(tokio::time::Duration::from_secs(2), event_rx.recv()).await {
                Ok(Ok(evt)) if evt.session_id.as_deref() == Some(temp_session) => {
                    if evt.method == "Fetch.requestPaused" {
                        if let Some(request_id) =
                            evt.params.get("requestId").and_then(|v| v.as_str())
                        {
                            let fulfilled = client
                                .send_command(
                                    "Fetch.fulfillRequest",
                                    Some(json!({
                                        "requestId": request_id,
                                        "responseCode": 200,
                                        "responseHeaders": [
                                            { "name": "Content-Type", "value": "text/html" }
                                        ],
                                        "body": &blank_html_b64
                                    })),
                                    Some(temp_session),
                                )
                                .await;
                            storage_capture_result(fulfilled, strict)?;
                        } else if strict {
                            return Err("Invalid storage capture event".to_string());
                        }
                    } else if evt.method == "Page.loadEventFired" {
                        page_loaded = true;
                        break;
                    }
                }
                Ok(Ok(_)) => continue,  // event for a different session
                Ok(Err(_)) => continue, // lagged or closed — retry within deadline
                Err(_) => break,        // outer timeout elapsed
            }
        }

        if !page_loaded {
            if strict {
                return Err("Storage capture target did not load".to_string());
            }
            continue;
        }

        if let Some(storage) = eval_origin_storage(client, temp_session, origin_js, strict).await? {
            if strict && storage.origin != *target_origin {
                return Err("Storage capture origin mismatch".to_string());
            }
            if !storage.local_storage.is_empty() || !storage.session_storage.is_empty() {
                results.push(storage);
            }
        } else if strict {
            return Err("Missing storage capture origin".to_string());
        }
    }

    Ok(results)
}

/// Export browser state, using owner-only file permissions on Unix.
/// Existing files are restricted before their contents are replaced; symlink paths are rejected.
/// On Windows, files retain the destination directory's inherited ACLs.
pub async fn save_state(
    client: &CdpClient,
    session_id: &str,
    path: Option<&str>,
    session_name: Option<&str>,
    session_id_str: &str,
    visited_origins: &HashSet<String>,
) -> Result<String, String> {
    save_state_inner(
        client,
        session_id,
        path,
        session_name,
        session_id_str,
        visited_origins,
        false,
        false,
    )
    .await
}

async fn save_state_inner(
    client: &CdpClient,
    session_id: &str,
    path: Option<&str>,
    session_name: Option<&str>,
    session_id_str: &str,
    visited_origins: &HashSet<String>,
    exclusive: bool,
    strict_capture: bool,
) -> Result<String, String> {
    let encryption_key = resolve_state_save_key()?;
    let cookies = cookies::get_all_cookies(client, session_id).await;
    let cookies = if strict_capture {
        cookies.map_err(|_| "Failed to capture browser cookies".to_string())?
    } else {
        cookies?
    };

    let origin_js = r#"(() => {
        const result = { origin: location.origin, localStorage: [], sessionStorage: [], captureFailed: false };
        try {
            for (let i = 0; i < localStorage.length; i++) {
                const key = localStorage.key(i);
                result.localStorage.push({ name: key, value: localStorage.getItem(key) });
            }
        } catch(e) { result.captureFailed = true; }
        try {
            for (let i = 0; i < sessionStorage.length; i++) {
                const key = sessionStorage.key(i);
                result.sessionStorage.push({ name: key, value: sessionStorage.getItem(key) });
            }
        } catch(e) { result.captureFailed = true; }
        return result;
    })()"#;

    // Merge visited origins with current frame tree origins
    let mut all_origins = visited_origins.clone();
    let tree_result = client
        .send_command_no_params("Page.getFrameTree", Some(session_id))
        .await;
    if let Some(tree_result) = storage_capture_result(tree_result, strict_capture)? {
        if let Some(tree) = tree_result
            .get("frameTree")
            .filter(|tree| tree.get("frame").is_some())
        {
            collect_frame_origins(tree, &mut all_origins);
        } else if strict_capture {
            return Err("Invalid storage capture frame tree".to_string());
        }
    }

    // 1. Collect localStorage from the current page
    let mut origins = Vec::new();
    let mut current_origin = String::new();

    if let Some(storage) =
        eval_origin_storage(client, session_id, origin_js, strict_capture).await?
    {
        current_origin = storage.origin.clone();
        if !storage.local_storage.is_empty() || !storage.session_storage.is_empty() {
            origins.push(storage);
        }
    }

    // 2. Collect localStorage from remaining origins via a disposable temp target
    all_origins.remove(&current_origin);
    if !all_origins.is_empty() {
        let remaining: Vec<String> = all_origins.into_iter().collect();
        let temp_origins =
            collect_storage_via_temp_target(client, &remaining, origin_js, strict_capture).await;
        if let Some(temp_origins) = storage_capture_result(temp_origins, strict_capture)? {
            origins.extend(temp_origins);
        }
    }

    let state = StorageState { cookies, origins };
    let json_str = serde_json::to_string_pretty(&state)
        .map_err(|e| format!("Failed to serialize state: {}", e))?;

    let mut save_path = match path {
        Some(p) => p.to_string(),
        None => {
            let dir = get_sessions_dir();
            ensure_private_state_dir(&dir).map_err(|e| {
                format!("Failed to create state directory {}: {}", dir.display(), e)
            })?;
            let name = session_name.unwrap_or("default");
            if !is_valid_session_name(name) {
                return Err(session_name_error(name));
            }
            dir.join(format!("{}-{}.json", name, session_id_str))
                .to_string_lossy()
                .to_string()
        }
    };

    if let Some(key) = encryption_key {
        let encrypted = encrypt_data(json_str.as_bytes(), &key)?;
        save_path.push_str(".enc");
        write_private_state(Path::new(&save_path), &encrypted, exclusive)
            .map_err(|e| format!("Failed to write state to {}: {}", save_path, e))?;
    } else {
        write_private_state(Path::new(&save_path), json_str.as_bytes(), exclusive)
            .map_err(|e| format!("Failed to write state to {}: {}", save_path, e))?;
    }

    Ok(save_path)
}

fn resolve_state_save_key() -> Result<Option<String>, String> {
    let key = match std::env::var("AGENT_BROWSER_ENCRYPTION_KEY") {
        Ok(key) => key,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(_) => {
            return Err("AGENT_BROWSER_ENCRYPTION_KEY must be 64 ASCII hex characters".to_string())
        }
    };
    if key.is_empty() {
        return Ok(None);
    }
    if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("AGENT_BROWSER_ENCRYPTION_KEY must be 64 ASCII hex characters".to_string());
    }
    Ok(Some(key))
}

fn ensure_private_state_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let dir = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(path)?;
        dir.set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "State directory must be a real directory",
            ));
        }
    }
    Ok(())
}

fn write_private_state(path: &Path, data: &[u8], exclusive: bool) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true);
    if exclusive {
        options.create_new(true);
    } else {
        options.create(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "State path must not be a reparse point",
            ));
        }
    }
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "State path must be a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.set_len(0)?;
    file.write_all(data)
}

fn auto_state_candidate_path(tmp_dir: &Path, base_name: &str) -> Result<PathBuf, String> {
    let mut random = [0u8; 16];
    getrandom::getrandom(&mut random)
        .map_err(|e| format!("Failed to generate state candidate name: {}", e))?;
    let suffix: String = random.iter().map(|byte| format!("{:02x}", byte)).collect();
    Ok(tmp_dir.join(format!("{}-candidate-{}.json", base_name, suffix)))
}

/// Save and validate a private candidate before rotating the last working state.
/// Candidate creation is exclusive, so collisions never overwrite an existing file.
pub async fn save_auto_state_transactional(
    client: &CdpClient,
    session_id: &str,
    session_name: &str,
    session_id_str: &str,
    visited_origins: &HashSet<String>,
) -> Result<String, String> {
    save_state_transactional(
        client,
        session_id,
        session_name,
        session_id_str,
        visited_origins,
        false,
    )
    .await
}

/// Save a checkpoint only when all required storage captures succeed.
/// Capture errors leave the last working state untouched; this is not an atomic browser snapshot.
pub async fn save_checkpoint_state_transactional(
    client: &CdpClient,
    session_id: &str,
    session_name: &str,
    session_id_str: &str,
    visited_origins: &HashSet<String>,
) -> Result<String, String> {
    save_state_transactional(
        client,
        session_id,
        session_name,
        session_id_str,
        visited_origins,
        true,
    )
    .await
}

async fn save_state_transactional(
    client: &CdpClient,
    session_id: &str,
    session_name: &str,
    session_id_str: &str,
    visited_origins: &HashSet<String>,
    strict_capture: bool,
) -> Result<String, String> {
    let encryption_key = resolve_state_save_key()?;
    if !is_valid_session_name(session_name) {
        return Err(session_name_error(session_name));
    }

    let dir = get_sessions_dir();
    ensure_private_state_dir(&dir)
        .map_err(|e| format!("Failed to create state directory {}: {}", dir.display(), e))?;

    let tmp_dir = dir.join(".tmp");
    ensure_private_state_dir(&tmp_dir).map_err(|e| {
        format!(
            "Failed to create temporary state directory {}: {}",
            tmp_dir.display(),
            e
        )
    })?;

    let base_name = format!("{}-{}", session_name, session_id_str);
    let final_json_path = dir.join(format!("{}.json", base_name));
    let final_path = if encryption_key.is_some() {
        PathBuf::from(format!("{}.enc", final_json_path.to_string_lossy()))
    } else {
        final_json_path
    };
    let candidate_json_path = auto_state_candidate_path(&tmp_dir, &base_name)?;
    let candidate_arg = candidate_json_path.to_string_lossy().to_string();

    let candidate_path = save_state_inner(
        client,
        session_id,
        Some(&candidate_arg),
        Some(session_name),
        session_id_str,
        visited_origins,
        true,
        strict_capture,
    )
    .await?;

    if let Err(err) = validate_state_file(&candidate_path) {
        let _ = fs::remove_file(&candidate_path);
        return Err(err);
    }

    let previous_path = PathBuf::from(format!("{}.previous", final_path.to_string_lossy()));
    if final_path.exists() {
        let _ = fs::remove_file(&previous_path);
        fs::rename(&final_path, &previous_path).map_err(|e| {
            format!(
                "Failed to rotate previous state {} to {}: {}",
                final_path.display(),
                previous_path.display(),
                e
            )
        })?;
    }

    let candidate = PathBuf::from(&candidate_path);
    if let Err(err) = fs::rename(&candidate, &final_path) {
        if previous_path.exists() && !final_path.exists() {
            let _ = fs::rename(&previous_path, &final_path);
        }
        return Err(format!(
            "Failed to promote state {} to {}: {}",
            candidate.display(),
            final_path.display(),
            err
        ));
    }
    if previous_path.exists() {
        let _ = fs::remove_file(&previous_path);
    }

    Ok(final_path.to_string_lossy().to_string())
}

fn read_state_json(path: &str) -> Result<String, String> {
    if is_encrypted_state(std::path::Path::new(path)) {
        let key = std::env::var("AGENT_BROWSER_ENCRYPTION_KEY").map_err(|_| {
            "Encrypted state file requires AGENT_BROWSER_ENCRYPTION_KEY".to_string()
        })?;
        let data =
            fs::read(path).map_err(|e| format!("Failed to read state from {}: {}", path, e))?;
        let decrypted = decrypt_data(&data, &key)?;
        Ok(String::from_utf8(decrypted)
            .map_err(|e| format!("Decrypted state is not valid UTF-8: {}", e))?)
    } else {
        match fs::read_to_string(path) {
            Ok(s) => Ok(s),
            Err(e) => {
                if let Ok(key) = std::env::var("AGENT_BROWSER_ENCRYPTION_KEY") {
                    let enc_path = format!("{}.enc", path);
                    if let Ok(data) = fs::read(&enc_path) {
                        let decrypted = decrypt_data(&data, &key)?;
                        Ok(String::from_utf8(decrypted)
                            .map_err(|de| format!("Decrypted state is not valid UTF-8: {}", de))?)
                    } else {
                        Err(format!("Failed to read state from {}: {}", path, e))
                    }
                } else {
                    Err(format!("Failed to read state from {}: {}", path, e))
                }
            }
        }
    }
}

pub fn validate_state_file(path: &str) -> Result<(), String> {
    let json_str = read_state_json(path)?;
    let _: StorageState =
        serde_json::from_str(&json_str).map_err(|e| format!("Invalid state file: {}", e))?;
    Ok(())
}

pub async fn load_state(client: &CdpClient, session_id: &str, path: &str) -> Result<(), String> {
    let json_str = read_state_json(path)?;

    let state: StorageState =
        serde_json::from_str(&json_str).map_err(|e| format!("Invalid state file: {}", e))?;

    // Load cookies
    if !state.cookies.is_empty() {
        let cookie_values: Vec<Value> = state
            .cookies
            .iter()
            .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
            .collect();
        cookies::set_cookies(client, session_id, cookie_values, None).await?;
    }

    // Load storage per origin
    for origin in &state.origins {
        if origin.local_storage.is_empty() && origin.session_storage.is_empty() {
            continue;
        }

        // Navigate to origin to set storage
        let navigate_url = format!("{}/", origin.origin.trim_end_matches('/'));
        client
            .send_command(
                "Page.navigate",
                Some(json!({ "url": navigate_url })),
                Some(session_id),
            )
            .await?;

        // Brief wait for navigation
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        for entry in &origin.local_storage {
            let js = format!(
                "localStorage.setItem({}, {})",
                serde_json::to_string(&entry.name).unwrap_or_default(),
                serde_json::to_string(&entry.value).unwrap_or_default(),
            );
            let _ = client
                .send_command_typed::<_, super::cdp::types::EvaluateResult>(
                    "Runtime.evaluate",
                    &EvaluateParams {
                        expression: js,
                        return_by_value: Some(true),
                        await_promise: Some(false),
                    },
                    Some(session_id),
                )
                .await;
        }

        for entry in &origin.session_storage {
            let js = format!(
                "sessionStorage.setItem({}, {})",
                serde_json::to_string(&entry.name).unwrap_or_default(),
                serde_json::to_string(&entry.value).unwrap_or_default(),
            );
            let _ = client
                .send_command_typed::<_, super::cdp::types::EvaluateResult>(
                    "Runtime.evaluate",
                    &EvaluateParams {
                        expression: js,
                        return_by_value: Some(true),
                        await_promise: Some(false),
                    },
                    Some(session_id),
                )
                .await;
        }
    }

    Ok(())
}

fn is_state_file(path: &std::path::Path) -> bool {
    let fname = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    fname.ends_with(".json")
        || fname.ends_with(".json.enc")
        || fname.ends_with(".json.previous")
        || fname.ends_with(".json.enc.previous")
}

fn is_encrypted_state(path: &std::path::Path) -> bool {
    let path = path.to_string_lossy();
    path.ends_with(".json.enc") || path.ends_with(".json.enc.previous")
}

pub fn state_list() -> Result<Value, String> {
    let dir = get_sessions_dir();
    if !dir.exists() {
        return Ok(json!({ "files": [], "directory": dir.to_string_lossy() }));
    }

    let mut files = Vec::new();

    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({ "files": [], "directory": dir.to_string_lossy() }));
        }
        Err(error) => return Err(format!("Failed to read sessions dir: {}", error)),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if is_state_file(&path) {
            let metadata = fs::metadata(&path).ok();
            let filename = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = metadata
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let encrypted = is_encrypted_state(&path);

            files.push(json!({
                "filename": filename,
                "path": path.to_string_lossy(),
                "size": size,
                "modified": modified,
                "encrypted": encrypted,
            }));
        }
    }

    Ok(json!({ "files": files, "directory": dir.to_string_lossy() }))
}

pub fn state_show(path: &str) -> Result<Value, String> {
    let encrypted = is_encrypted_state(std::path::Path::new(path));
    let json_str = if encrypted {
        let key = std::env::var("AGENT_BROWSER_ENCRYPTION_KEY").map_err(|_| {
            "Encrypted state file requires AGENT_BROWSER_ENCRYPTION_KEY".to_string()
        })?;
        let data = fs::read(path).map_err(|e| format!("Failed to read state file: {}", e))?;
        let decrypted = decrypt_data(&data, &key)?;
        String::from_utf8(decrypted)
            .map_err(|e| format!("Decrypted state is not valid UTF-8: {}", e))?
    } else {
        fs::read_to_string(path).map_err(|e| format!("Failed to read state file: {}", e))?
    };

    let state: StorageState =
        serde_json::from_str(&json_str).map_err(|e| format!("Invalid state file: {}", e))?;

    let metadata = fs::metadata(path).ok();
    let filename = std::path::Path::new(path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    Ok(json!({
        "filename": filename,
        "path": path,
        "size": metadata.as_ref().map(|m| m.len()).unwrap_or(0),
        "modified": metadata.as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "encrypted": encrypted,
        "summary": format!("{} cookies, {} origins", state.cookies.len(), state.origins.len()),
        "state": state,
    }))
}

pub fn state_clear(path: Option<&str>) -> Result<Value, String> {
    if let Some(p) = path {
        fs::remove_file(p).map_err(|e| format!("Failed to delete state: {}", e))?;
        return Ok(json!({ "deleted": p }));
    }

    let dir = get_sessions_dir();
    if !dir.exists() {
        return Ok(json!({ "deleted": 0 }));
    }

    let mut count = 0;
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_state_file(&path) {
                let _ = fs::remove_file(&path);
                count += 1;
            }
        }
    }

    Ok(json!({ "deleted": count }))
}

pub fn state_clean(max_age_days: u64) -> Result<Value, String> {
    let dir = get_sessions_dir();
    if !dir.exists() {
        return Ok(json!({ "cleaned": 0, "keptCount": 0, "days": max_age_days }));
    }

    let now = std::time::SystemTime::now();
    let max_age = std::time::Duration::from_secs(max_age_days * 86400);
    let mut deleted = 0;
    let mut kept = 0;

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_state_file(&path) {
                continue;
            }

            if let Ok(metadata) = fs::metadata(&path) {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(age) = now.duration_since(modified) {
                        if age > max_age {
                            let _ = fs::remove_file(&path);
                            deleted += 1;
                            continue;
                        }
                    }
                }
            }
            kept += 1;
        }
    }

    Ok(json!({ "cleaned": deleted, "keptCount": kept, "days": max_age_days }))
}

pub fn state_rename(old_path: &str, new_name: &str) -> Result<Value, String> {
    let old = PathBuf::from(old_path);
    if !old.exists() {
        return Err(format!("State file not found: {}", old_path));
    }

    let fallback = PathBuf::from(".");
    let dir = old.parent().unwrap_or(&fallback);
    let new_path = dir.join(format!("{}.json", new_name));

    fs::rename(&old, &new_path).map_err(|e| format!("Failed to rename state: {}", e))?;

    Ok(json!({
        "renamed": true,
        "from": old_path,
        "to": new_path.to_string_lossy(),
    }))
}

fn encrypt_data(data: &[u8], key_str: &str) -> Result<Vec<u8>, String> {
    let mut hasher = Sha256::new();
    hasher.update(key_str.as_bytes());
    let key_bytes = hasher.finalize();
    let cipher =
        Aes256Gcm::new_from_slice(&key_bytes).map_err(|e| format!("Invalid key: {}", e))?;

    let mut nonce = [0u8; 12];
    getrandom::getrandom(&mut nonce).map_err(|e| format!("Failed to generate nonce: {}", e))?;
    let ciphertext = cipher
        .encrypt(aes_gcm::Nonce::from_slice(&nonce), data)
        .map_err(|e| format!("Encryption failed: {}", e))?;

    let mut result = Vec::with_capacity(12 + ciphertext.len());
    result.extend_from_slice(&nonce);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

fn decrypt_data(data: &[u8], key_str: &str) -> Result<Vec<u8>, String> {
    if data.len() < 13 {
        return Err("Ciphertext too short".to_string());
    }
    let (nonce_bytes, ciphertext) = data.split_at(12);

    let mut hasher = Sha256::new();
    hasher.update(key_str.as_bytes());
    let key_bytes = hasher.finalize();
    let cipher =
        Aes256Gcm::new_from_slice(&key_bytes).map_err(|e| format!("Invalid key: {}", e))?;
    let plaintext = cipher
        .decrypt(aes_gcm::Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|e| format!("Decryption failed: {}", e))?;
    Ok(plaintext)
}

pub fn find_auto_state_file(session_name: &str) -> Option<String> {
    if !is_valid_session_name(session_name) {
        return None;
    }

    let dir = get_sessions_dir();
    if !dir.exists() {
        return None;
    }
    let prefix = format!("{}-", session_name);
    let mut best_path: Option<(String, std::time::SystemTime)> = None;

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let fname = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let is_match = fname.starts_with(&prefix)
                && (fname.ends_with(".json") || fname.ends_with(".json.enc"));
            if !is_match {
                continue;
            }
            let modified = fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(std::time::UNIX_EPOCH);
            if best_path.as_ref().is_none_or(|(_, t)| modified > *t) {
                best_path = Some((path.to_string_lossy().to_string(), modified));
            }
        }
    }
    best_path.map(|(p, _)| p)
}

/// Dispatch a state management command from its JSON payload.
/// Returns `Some(result)` for recognised state_* actions, `None` otherwise.
pub fn dispatch_state_command(cmd: &Value) -> Option<Result<Value, String>> {
    let action = cmd.get("action").and_then(|v| v.as_str())?;
    match action {
        "state_list" => Some(state_list()),
        "state_show" => Some(
            cmd.get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing 'path' parameter".to_string())
                .and_then(state_show),
        ),
        "state_clear" => {
            let path = cmd.get("path").and_then(|v| v.as_str());
            Some(state_clear(path))
        }
        "state_clean" => {
            let days = cmd.get("days").and_then(|v| v.as_u64()).unwrap_or(30);
            Some(state_clean(days))
        }
        "state_rename" => Some(
            cmd.get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "Missing 'path' parameter".to_string())
                .and_then(|path| {
                    cmd.get("name")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "Missing 'name' parameter".to_string())
                        .and_then(|name| state_rename(path, name))
                }),
        ),
        _ => None,
    }
}

/// Return the agent-browser state root (`~/.agent-browser`, falling back to
/// `<tempdir>/agent-browser` when the home directory can't be resolved).
/// This is the parent of `sessions/`, auth storage, and the encryption key.
pub fn get_state_dir() -> PathBuf {
    let base = if let Some(home) = dirs::home_dir() {
        home.join(".agent-browser")
    } else {
        std::env::temp_dir().join("agent-browser")
    };

    if let Ok(namespace) = std::env::var("AGENT_BROWSER_NAMESPACE") {
        let namespace = sanitize_session_component(&namespace);
        if !namespace.is_empty() {
            return base.join("namespaces").join(namespace).join("state");
        }
    }

    base
}

pub fn get_sessions_dir() -> PathBuf {
    get_state_dir().join("sessions")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_save_key_resolution() {
        let guard = crate::test_utils::EnvGuard::new(&["AGENT_BROWSER_ENCRYPTION_KEY"]);
        guard.remove("AGENT_BROWSER_ENCRYPTION_KEY");
        assert_eq!(resolve_state_save_key().unwrap(), None);
        guard.set("AGENT_BROWSER_ENCRYPTION_KEY", "");
        assert_eq!(resolve_state_save_key().unwrap(), None);
        let valid = "aB09".repeat(16);
        guard.set("AGENT_BROWSER_ENCRYPTION_KEY", &valid);
        assert_eq!(resolve_state_save_key().unwrap(), Some(valid));
        for invalid in [
            "secret-invalid-key".to_string(),
            "a".repeat(63),
            "a".repeat(65),
            "g".repeat(64),
            "é".repeat(32),
        ] {
            guard.set("AGENT_BROWSER_ENCRYPTION_KEY", &invalid);
            let error = resolve_state_save_key().unwrap_err();
            assert_eq!(
                error,
                "AGENT_BROWSER_ENCRYPTION_KEY must be 64 ASCII hex characters"
            );
            assert!(!error.contains(&invalid));
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_state_save_key_rejects_non_unicode_without_leaking() {
        use std::os::unix::ffi::OsStringExt;

        let _guard = crate::test_utils::EnvGuard::new(&["AGENT_BROWSER_ENCRYPTION_KEY"]);
        unsafe {
            std::env::set_var(
                "AGENT_BROWSER_ENCRYPTION_KEY",
                std::ffi::OsString::from_vec(vec![0xff; 64]),
            );
        }
        assert_eq!(
            resolve_state_save_key().unwrap_err(),
            "AGENT_BROWSER_ENCRYPTION_KEY must be 64 ASCII hex characters"
        );
    }

    fn capture_result(data: Value) -> super::super::cdp::types::EvaluateResult {
        serde_json::from_value(json!({ "result": { "type": "object", "value": data } })).unwrap()
    }

    #[test]
    fn test_strict_storage_capture_preserves_entries_and_empty_origins() {
        for entries in [json!([]), json!([{ "name": "auth", "value": "secret" }])] {
            let storage = parse_storage_capture(
                capture_result(json!({
                    "origin": "https://example.com",
                    "localStorage": entries,
                    "sessionStorage": [],
                    "captureFailed": false
                })),
                true,
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                serde_json::to_value(storage.local_storage).unwrap(),
                entries
            );
        }
    }

    #[test]
    fn test_strict_storage_capture_rejects_malformed_and_partial_results() {
        let valid = json!({
            "origin": "https://example.com", "localStorage": [],
            "sessionStorage": [], "captureFailed": false
        });
        for field in ["origin", "localStorage", "sessionStorage", "captureFailed"] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(parse_storage_capture(capture_result(missing), true).is_err());
            let mut malformed = valid.clone();
            malformed[field] = json!({ "secret": "do-not-expose" });
            let error = parse_storage_capture(capture_result(malformed), true).unwrap_err();
            assert_eq!(error, "Failed to capture origin storage");
        }
        let mut partial = valid.clone();
        partial["captureFailed"] = json!(true);
        assert!(parse_storage_capture(capture_result(partial.clone()), true).is_err());
        assert!(parse_storage_capture(capture_result(partial), false)
            .unwrap()
            .is_some());
        for malformed in [Value::Null, json!(42), json!([])] {
            assert!(parse_storage_capture(capture_result(malformed), true).is_err());
        }
        let mut invalid_entry = valid;
        invalid_entry["localStorage"] = json!([{ "name": "auth", "value": null }]);
        assert!(parse_storage_capture(capture_result(invalid_entry), true).is_err());
    }

    #[test]
    fn test_storage_capture_errors_are_sanitized_and_compatibility_is_tolerant() {
        let error =
            storage_capture_result::<()>(Err("secret CDP message".to_string()), true).unwrap_err();
        assert_eq!(error, "Failed to capture browser storage");
        assert!(
            storage_capture_result::<()>(Err("secret".to_string()), false)
                .unwrap()
                .is_none()
        );
        let result = serde_json::from_value(json!({
            "result": { "type": "undefined" },
            "exceptionDetails": { "text": "secret JavaScript exception" }
        }))
        .unwrap();
        assert_eq!(
            parse_storage_capture(result, true).unwrap_err(),
            "Failed to capture origin storage"
        );
    }

    #[test]
    fn test_private_state_write_replaces_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.json");
        write_private_state(&path, b"old state with trailing data", false).unwrap();
        write_private_state(&path, b"{}", false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{}");
    }

    #[test]
    fn test_private_state_candidate_collision_preserves_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["candidate.json", "candidate.json.enc"] {
            let path = dir.path().join(name);
            write_private_state(&path, b"existing state", true).unwrap();
            let error = write_private_state(&path, b"replacement", true).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(path).unwrap(), b"existing state");
        }
    }

    #[test]
    fn test_auto_state_candidate_names_are_unique() {
        let dir = tempfile::tempdir().unwrap();
        let first = auto_state_candidate_path(dir.path(), "auth-session").unwrap();
        let second = auto_state_candidate_path(dir.path(), "auth-session").unwrap();
        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(dir.path()));
        assert!(first
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(".json"));
    }

    #[cfg(unix)]
    #[test]
    fn test_private_state_file_modes_for_new_and_existing_exports() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        for name in ["export.json", "export.json.enc"] {
            let path = dir.path().join(name);
            write_private_state(&path, b"new state", false).unwrap();
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            write_private_state(&path, b"private replacement", false).unwrap();
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(fs::read(&path).unwrap(), b"private replacement");
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_private_state_directories_only_restrict_sessions_and_tmp() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("browser-state");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        let sessions = root.join("sessions");
        let tmp = sessions.join(".tmp");
        for _ in 0..2 {
            ensure_private_state_dir(&sessions).unwrap();
            ensure_private_state_dir(&tmp).unwrap();
            for path in [&sessions, &tmp] {
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o700
                );
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_private_state_directory_rejects_symlinks_without_chmod_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("unrelated");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let link = dir.path().join("sessions");
        symlink(&target, &link).unwrap();
        assert!(ensure_private_state_dir(&link).is_err());
        assert_eq!(
            fs::metadata(target).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_private_state_writes_reject_symlink_and_dangling_candidates() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("unrelated");
        fs::write(&target, b"do not overwrite").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        let link = dir.path().join("candidate.json");
        symlink(&target, &link).unwrap();
        for exclusive in [true, false] {
            assert!(write_private_state(&link, b"secret", exclusive).is_err());
            assert_eq!(fs::read(&target).unwrap(), b"do not overwrite");
            assert_eq!(
                fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o644
            );
        }
        let missing = dir.path().join("missing");
        let dangling = dir.path().join("candidate.json.enc");
        symlink(&missing, &dangling).unwrap();
        assert!(write_private_state(&dangling, b"secret", true).is_err());
        assert!(!missing.exists());
    }

    #[test]
    fn test_storage_state_serialization() {
        let state = StorageState {
            cookies: vec![Cookie {
                name: "session".to_string(),
                value: "abc123".to_string(),
                domain: ".example.com".to_string(),
                path: "/".to_string(),
                expires: 0.0,
                size: 0,
                http_only: true,
                secure: false,
                session: true,
                same_site: Some("Lax".to_string()),
            }],
            origins: vec![OriginStorage {
                origin: "https://example.com".to_string(),
                local_storage: vec![StorageEntry {
                    name: "key".to_string(),
                    value: "val".to_string(),
                }],
                session_storage: vec![],
            }],
        };

        let json = serde_json::to_string_pretty(&state).unwrap();
        let parsed: StorageState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.cookies.len(), 1);
        assert_eq!(parsed.cookies[0].name, "session");
        assert_eq!(parsed.origins.len(), 1);
        assert_eq!(parsed.origins[0].local_storage.len(), 1);
    }

    #[test]
    fn test_storage_state_empty() {
        let state = StorageState {
            cookies: vec![],
            origins: vec![],
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: StorageState = serde_json::from_str(&json).unwrap();
        assert!(parsed.cookies.is_empty());
        assert!(parsed.origins.is_empty());
    }

    #[test]
    fn test_state_show_nonexistent_file() {
        let result = state_show("/tmp/nonexistent-agent-browser-state-file.json");
        assert!(result.is_err());
    }

    #[test]
    fn test_state_clear_nonexistent_file() {
        let result = state_clear(Some("/tmp/nonexistent-agent-browser-state-file.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_state_file_matcher_includes_transactional_backups() {
        assert!(is_state_file(std::path::Path::new("auth.json")));
        assert!(is_state_file(std::path::Path::new("auth.json.enc")));
        assert!(is_state_file(std::path::Path::new("auth.json.previous")));
        assert!(is_state_file(std::path::Path::new(
            "auth.json.enc.previous"
        )));
        assert!(is_encrypted_state(std::path::Path::new(
            "auth.json.enc.previous"
        )));
    }

    #[test]
    fn test_state_clear_removes_transactional_backups() {
        let guard = crate::test_utils::EnvGuard::new(&["HOME", "AGENT_BROWSER_NAMESPACE"]);
        let dir = tempfile::tempdir().unwrap();
        guard.set("HOME", dir.path().to_str().unwrap());
        guard.remove("AGENT_BROWSER_NAMESPACE");

        let sessions = get_sessions_dir();
        fs::create_dir_all(&sessions).unwrap();
        fs::write(sessions.join("auth-test.json"), "{}").unwrap();
        fs::write(sessions.join("auth-test.json.previous"), "{}").unwrap();
        fs::write(sessions.join("auth-test.json.enc.previous"), "encrypted").unwrap();

        let result = state_clear(None).unwrap();

        assert_eq!(result["deleted"], 3);
        assert!(!sessions.join("auth-test.json").exists());
        assert!(!sessions.join("auth-test.json.previous").exists());
        assert!(!sessions.join("auth-test.json.enc.previous").exists());
    }

    #[test]
    fn test_state_rename_nonexistent() {
        let result = state_rename("/tmp/nonexistent-agent-browser-state-file.json", "new-name");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_state_list_returns_json() {
        let result = state_list().unwrap();
        assert!(result.get("files").is_some());
        assert!(result.get("directory").is_some());
    }

    #[test]
    fn test_sessions_dir_path() {
        let dir = get_sessions_dir();
        assert!(dir.to_string_lossy().contains("sessions"));
    }

    #[test]
    fn test_get_state_dir_namespace_scopes_sessions() {
        let _guard = crate::test_utils::EnvGuard::new(&["AGENT_BROWSER_NAMESPACE"]);
        _guard.set("AGENT_BROWSER_NAMESPACE", "Worktree: One");

        let dir = get_state_dir();
        let expected_state_suffix = PathBuf::from(".agent-browser")
            .join("namespaces")
            .join("worktree-one")
            .join("state");
        let expected_sessions_suffix = expected_state_suffix.join("sessions");

        assert!(dir.ends_with(expected_state_suffix));
        assert!(get_sessions_dir().ends_with(expected_sessions_suffix));
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let plain = b"hello world";
        let key = "test-secret-key";
        let encrypted = encrypt_data(plain, key).unwrap();
        assert!(encrypted.len() > 12);
        assert_ne!(&encrypted[12..], plain);
        let decrypted = decrypt_data(&encrypted, key).unwrap();
        assert_eq!(decrypted, plain);
    }

    #[test]
    fn test_decrypt_wrong_key_fails() {
        let plain = b"secret data";
        let encrypted = encrypt_data(plain, "key1").unwrap();
        let result = decrypt_data(&encrypted, "key2");
        assert!(result.is_err());
    }

    #[test]
    fn test_cookie_serde_roundtrip() {
        let cookie = Cookie {
            name: "test".to_string(),
            value: "123".to_string(),
            domain: ".test.com".to_string(),
            path: "/api".to_string(),
            expires: 1700000000.0,
            size: 7,
            http_only: false,
            secure: true,
            session: false,
            same_site: Some("Strict".to_string()),
        };

        let json = serde_json::to_value(&cookie).unwrap();
        assert_eq!(json["name"], "test");
        assert_eq!(json["httpOnly"], false);
        assert_eq!(json["secure"], true);
        assert_eq!(json["sameSite"], "Strict");
    }

    #[test]
    fn test_dispatch_state_command_routes_state_list() {
        let cmd = serde_json::json!({ "action": "state_list" });
        let result = dispatch_state_command(&cmd);
        assert!(result.is_some());
        assert!(result.unwrap().is_ok());
    }

    #[test]
    fn test_dispatch_state_command_returns_none_for_unknown() {
        let cmd = serde_json::json!({ "action": "navigate" });
        assert!(dispatch_state_command(&cmd).is_none());
    }

    #[test]
    fn test_dispatch_state_command_returns_none_for_missing_action() {
        let cmd = serde_json::json!({});
        assert!(dispatch_state_command(&cmd).is_none());
    }

    #[test]
    fn test_dispatch_state_show_missing_path() {
        let cmd = serde_json::json!({ "action": "state_show" });
        let result = dispatch_state_command(&cmd).unwrap();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Missing 'path' parameter");
    }

    #[test]
    fn test_dispatch_state_rename_missing_params() {
        let cmd = serde_json::json!({ "action": "state_rename" });
        let result = dispatch_state_command(&cmd).unwrap();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Missing 'path' parameter");

        let cmd = serde_json::json!({ "action": "state_rename", "path": "/tmp/test.json" });
        let result = dispatch_state_command(&cmd).unwrap();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Missing 'name' parameter");
    }
}
