//! Sampling capability discovery for the active engine.
//!
//! llama.cpp exposes generation defaults at `/props`. janas-server currently
//! exposes only the OpenAI model surface, so the same parser also understands
//! capability metadata future Janas builds may attach to `/v1/models`. When no
//! engine advertises a parameter, the studio returns a deliberately safe map:
//! Min-P stays hidden while the established sampling controls remain visible.

use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::Value;
use tauri::State;

use crate::{
    engines::EngineSupervisor,
    error::AppResult,
    settings::SettingsStore,
};

pub const SAMPLING_PARAMETERS: [&str; 6] = [
    "temperature",
    "top_p",
    "top_k",
    "min_p",
    "repeat_penalty",
    "max_tokens",
];

/// `/props` has a complete generation-settings object, so a missing key there
/// means unsupported. `/v1/config` and `/v1/models` may expose only capability
/// overlays; those are merged onto the safe defaults.
const ENDPOINTS: [(&str, bool); 5] = [
    ("/props", false),
    ("/v1/config", true),
    ("/v1/capabilities", true),
    ("/capabilities", true),
    ("/v1/models", true),
];

/// The conservative map used when the engine is stopped, unreachable, or does
/// not advertise its sampling surface yet.
pub fn fallback_capabilities() -> BTreeMap<String, bool> {
    SAMPLING_PARAMETERS
        .iter()
        .map(|key| ((*key).to_string(), *key != "min_p"))
        .collect()
}

/// Query the active engine without blocking the UI thread. Every request has a
/// short timeout and the whole probe has a hard deadline, so a stopped or
/// black-holed engine cannot stall the panel.
#[tauri::command]
pub async fn get_backend_capabilities(
    supervisor: State<'_, Arc<EngineSupervisor>>,
    store: State<'_, Arc<SettingsStore>>,
) -> AppResult<BTreeMap<String, bool>> {
    let settings = store.get();
    let active = supervisor.active_info().await;
    let host = active
        .as_ref()
        .map(|info| info.host.clone())
        .unwrap_or_else(|| settings.connect_host().to_string());
    let port = active.as_ref().map(|info| info.port).unwrap_or(settings.port);

    Ok(detect_capabilities(&host, port, settings.api_key.trim())
        .await
        .unwrap_or_else(fallback_capabilities))
}

async fn detect_capabilities(host: &str, port: u16, api_key: &str) -> Option<BTreeMap<String, bool>> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(450))
        .timeout(Duration::from_millis(700))
        .no_proxy()
        .build()
        .ok()?;
    let base = format!("http://{}:{}", host.trim(), port);
    let deadline = Instant::now() + Duration::from_millis(1_800);

    for (path, overlay) in ENDPOINTS {
        if Instant::now() >= deadline {
            break;
        }
        let Some(value) = fetch_json(&client, &format!("{base}{path}"), api_key).await else {
            continue;
        };
        if let Some(capabilities) = parse_capabilities(&value, overlay) {
            return Some(capabilities);
        }
    }
    None
}

async fn fetch_json(client: &reqwest::Client, url: &str, api_key: &str) -> Option<Value> {
    let mut request = client.get(url);
    if !api_key.is_empty() {
        request = request.bearer_auth(api_key);
    }
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json::<Value>().await.ok()
}

/// Parse a JSON document into the six parameter flags. `overlay` preserves the
/// safe defaults for keys the engine does not mention; strict mode maps every
/// missing key to false, which is what a full generation-settings object means.
fn parse_capabilities(value: &Value, overlay: bool) -> Option<BTreeMap<String, bool>> {
    let mut capabilities: BTreeMap<String, bool> = if overlay {
        fallback_capabilities()
    } else {
        SAMPLING_PARAMETERS
            .iter()
            .map(|key| ((*key).to_string(), false))
            .collect()
    };
    let mut found = false;
    scan(value, &mut capabilities, &mut found, 0);
    found.then_some(capabilities)
}

fn scan(
    value: &Value,
    capabilities: &mut BTreeMap<String, bool>,
    found: &mut bool,
    depth: u8,
) {
    if depth > 8 {
        return;
    }
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if let Some(parameter) = parameter_for_key(key) {
                    *found = true;
                    capabilities.insert(parameter.to_string(), enabled(value));
                }
                if is_capability_collection(key) {
                    mark_collection(value, capabilities, found);
                }
                scan(value, capabilities, found, depth + 1);
            }
        }
        Value::Array(items) => {
            for item in items {
                scan(item, capabilities, found, depth + 1);
            }
        }
        _ => {}
    }
}

fn mark_collection(
    value: &Value,
    capabilities: &mut BTreeMap<String, bool>,
    found: &mut bool,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                if let Some(name) = item.as_str() {
                    mark_name(name, capabilities, found);
                } else if let Some(object) = item.as_object() {
                    for key in ["name", "id", "parameter"] {
                        if let Some(name) = object.get(key).and_then(Value::as_str) {
                            mark_name(name, capabilities, found);
                            break;
                        }
                    }
                }
            }
        }
        Value::String(text) => {
            for name in text.split([',', ';', ' ', '\n']) {
                mark_name(name, capabilities, found);
            }
        }
        _ => {}
    }
}

fn mark_name(name: &str, capabilities: &mut BTreeMap<String, bool>, found: &mut bool) {
    if let Some(parameter) = parameter_for_key(name) {
        *found = true;
        capabilities.insert(parameter.to_string(), true);
    }
}

fn is_capability_collection(key: &str) -> bool {
    matches!(
        normalize_key(key).as_str(),
        "capabilities"
            | "supported_parameters"
            | "supported_sampling_parameters"
            | "sampling_parameters"
            | "sampling"
            | "sampling_params"
            | "parameters"
            | "generation_settings"
            | "default_generation_settings"
    )
}

fn parameter_for_key(raw: &str) -> Option<&'static str> {
    match normalize_key(raw).as_str() {
        "temperature" => Some("temperature"),
        "top_p" | "top_p_sampling" | "topp" => Some("top_p"),
        "top_k" | "topk" => Some("top_k"),
        "min_p" | "minp" => Some("min_p"),
        "repeat_penalty" | "repetition_penalty" => Some("repeat_penalty"),
        "max_tokens" | "max_completion_tokens" | "n_predict" | "max_new_tokens"
        | "max_reply_tokens" => Some("max_tokens"),
        _ => None,
    }
}

fn normalize_key(raw: &str) -> String {
    raw.trim().to_ascii_lowercase().replace('-', "_")
}

fn enabled(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Object(object) => object
            .get("enabled")
            .or_else(|| object.get("supported"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fallback_hides_min_p_and_keeps_temperature() {
        let capabilities = fallback_capabilities();
        assert_eq!(capabilities["temperature"], true);
        assert_eq!(capabilities["min_p"], false);
        assert_eq!(capabilities["top_k"], true);
    }

    #[test]
    fn strict_props_map_missing_and_null_to_false() {
        let value = json!({
            "default_generation_settings": {
                "temperature": 0.7,
                "top_p": 0.9,
                "top_k": 40,
                "min_p": null,
                "repeat_penalty": 1.05
            }
        });
        let capabilities = parse_capabilities(&value, false).expect("capability map");
        assert!(capabilities["temperature"]);
        assert!(capabilities["top_p"]);
        assert!(capabilities["top_k"]);
        assert!(!capabilities["min_p"]);
        assert!(capabilities["repeat_penalty"]);
        assert!(!capabilities["max_tokens"]);
    }

    #[test]
    fn forward_compatible_model_capabilities_overlay_safe_defaults() {
        let value = json!({
            "object": "list",
            "data": [{
                "id": "future-model",
                "capabilities": {
                    "sampling": ["temperature", "top_p", "min_p", "n_predict"],
                    "tools": true
                }
            }]
        });
        let capabilities = parse_capabilities(&value, true).expect("capability map");
        assert!(capabilities["temperature"]);
        assert!(capabilities["min_p"]);
        assert!(capabilities["max_tokens"]);
        assert!(capabilities["top_k"]);
    }

    #[test]
    fn a_model_without_sampling_keys_is_not_a_capability_source() {
        let value = json!({
            "object": "list",
            "data": [{
                "id": "qwen3-0.6b",
                "description": "Qwen3 with tool support"
            }]
        });
        assert!(parse_capabilities(&value, true).is_none());
    }

    #[test]
    fn explicit_false_and_dash_case_are_honoured() {
        let value = json!({
            "capabilities": {
                "temperature": true,
                "top-p": false,
                "min-p": null
            }
        });
        let capabilities = parse_capabilities(&value, true).expect("capability map");
        assert!(capabilities["temperature"]);
        assert!(!capabilities["top_p"]);
        assert!(!capabilities["min_p"]);
    }

    #[tokio::test]
    async fn probes_a_local_engine_and_reads_forward_compatible_model_capabilities() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fake engine");
        let port = listener.local_addr().expect("fake engine address").port();
        let server = std::thread::spawn(move || {
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().expect("accept probe");
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request);
                let request = String::from_utf8_lossy(&request);
                let (status, body) = if request.starts_with("GET /v1/models ") {
                    (
                        "200 OK",
                        r#"{"object":"list","data":[{"id":"future","capabilities":{"min_p":true,"temperature":true}}]}"#,
                    )
                } else {
                    ("404 Not Found", r#"{"error":"unknown"}"#)
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let capabilities = detect_capabilities("127.0.0.1", port, "")
            .await
            .expect("capability response");
        assert!(capabilities["min_p"]);
        assert!(capabilities["temperature"]);
        server.join().expect("fake engine finished");
    }
}
