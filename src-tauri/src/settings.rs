use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{OnceLock, RwLock},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// Everything the studio needs to find and launch the Janas tools. Stored as
/// JSON in the platform config directory and editable from the Settings view.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// A full path, or a name looked up on PATH (e.g. "janas-server").
    pub engine_path: String,
    /// "janas-convert" for the studio's converter contract, or "gguf2jns" which
    /// upstream Janas ships. The launcher adapts the arguments automatically.
    pub converter_path: String,
    pub host: String,
    pub port: u16,
    /// Where downloaded and converted models are kept.
    pub models_dir: String,
    /// The .jns file handed to janas-server on launch.
    pub model_path: String,
    /// Additional janas-server arguments, parsed with shell-word rules.
    pub extra_args: String,
    /// Optional `--api-key` for the local server. Never leaves this machine.
    pub api_key: String,
    /// `--ctx`: context window in tokens.
    pub ctx_size: u32,
    /// `--cache`: GiB reserved for streamed experts. 0 means automatic.
    pub cache_gib: f32,
    /// `--mtp` / `--no-mtp`: "auto", "off" or "file".
    pub mtp_mode: String,
    /// Optional MTP block file when `mtp_mode` is "file".
    pub mtp_path: String,
    /// `--draft`: a small model of the same family used for speculation.
    pub draft_path: String,
    /// `--prompt-on`: auto, cpu, gpu, npu, npu+gpu, or a comma-separated list.
    pub prompt_on: String,
    /// `--reply-on`: where reply tokens are computed.
    pub reply_on: String,
    /// `--drafts-on`: where speculative passes are checked.
    pub drafts_on: String,
    /// `--think`: auto, on or off.
    pub think: String,
    /// `--npu-int8`: eight-bit activations for NPU prompt reading.
    pub npu_int8: bool,
    /// `--mode`: auto, eco or max.
    pub engine_mode: String,
    /// `JANAS_ATTN`: auto, exact (float) or fast (16-bit query).
    pub janas_attention: String,
    /// `JANAS_KV`: auto, 8 or 16 bits for both keys and values.
    pub janas_kv_cache: String,
    /// A full path to a .gguf model for llama-server.
    pub llama_model_path: String,
    /// Optional `--mmproj` GGUF. llama.cpp needs this vision projector to
    /// accept image content parts in chat requests.
    pub llama_mmproj_path: String,
    /// llama-server's loopback port. Kept separate from Janas's port so the
    /// two engines can be switched without editing the other configuration.
    pub llama_port: u16,
    /// `--n-gpu-layers`: 0 keeps everything on the CPU. Values are capped to
    /// the selected GGUF's block count when its metadata can be read; 999 is
    /// the fallback that asks llama.cpp to offload every layer.
    pub llama_gpu_layers: i32,
    /// `--ctx-size` for llama-server.
    pub llama_ctx_size: u32,
    /// `--flash-attn`: auto, on or off.
    pub llama_flash_attn: String,
    /// `--cache-type-k`: f32, f16, bf16, q8_0, q4_0, q4_1, iq4_nl, q5_0 or q5_1.
    pub llama_kv_cache_k: String,
    /// `--cache-type-v`: the same set, with f16 as the safe default.
    pub llama_kv_cache_v: String,
    /// llama.cpp only: run MoE cache misses on host experts through the
    /// llama_moe_cache pipeline instead of uploading every miss to the GPU.
    /// janas-server never reads this setting.
    pub llama_direct_cpu_moe: bool,
    /// llama.cpp only: maximum CPU threads for direct MoE compute. Zero keeps
    /// llama.cpp's own default. janas-server never reads this setting.
    pub llama_cpu_threads: u32,
    /// Additional llama-server flags, parsed with shell-word rules.
    pub llama_extra_args: String,
    /// Janas: request/keep the SSD streaming path when the build advertises it.
    pub ssd_streaming: bool,
    /// Janas: optional SSD cache directory, when the build advertises it.
    pub ssd_cache_dir: String,
    /// Server-side default Top-K, mirrored from the sampling panel.
    pub top_k: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            engine_path: "janas-server".into(),
            converter_path: "janas-convert".into(),
            host: "127.0.0.1".into(),
            port: 8080,
            models_dir: String::new(),
            model_path: String::new(),
            extra_args: String::new(),
            api_key: String::new(),
            ctx_size: 16_384,
            cache_gib: 0.0,
            mtp_mode: "auto".into(),
            mtp_path: String::new(),
            draft_path: String::new(),
            prompt_on: "auto".into(),
            reply_on: "auto".into(),
            drafts_on: "auto".into(),
            think: "auto".into(),
            npu_int8: false,
            engine_mode: "auto".into(),
            janas_attention: "auto".into(),
            janas_kv_cache: "auto".into(),
            llama_model_path: String::new(),
            llama_mmproj_path: String::new(),
            llama_port: 8081,
            llama_gpu_layers: 0,
            llama_ctx_size: 16_384,
            llama_flash_attn: "auto".into(),
            llama_kv_cache_k: "f16".into(),
            llama_kv_cache_v: "f16".into(),
            llama_direct_cpu_moe: false,
            llama_cpu_threads: 0,
            llama_extra_args: String::new(),
            ssd_streaming: false,
            ssd_cache_dir: String::new(),
            top_k: 40,
        }
    }
}

impl AppSettings {
    pub fn connect_host(&self) -> &str {
        match self.host.trim() {
            "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
            other => other,
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.connect_host(), self.port)
    }

    pub fn model_display_name(&self) -> Option<String> {
        Path::new(self.model_path.trim())
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .filter(|stem| !stem.is_empty())
    }

    pub fn llama_model_display_name(&self) -> Option<String> {
        Path::new(self.llama_model_path.trim())
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .filter(|stem| !stem.is_empty())
    }

    /// Keep deserialized files from older builds in a range the adapters can
    /// safely turn into command-line arguments.
    pub fn normalize(&mut self) {
        self.port = self.port.clamp(1, 65_535);
        self.llama_port = self.llama_port.clamp(1, 65_535);
        self.llama_gpu_layers = self.llama_gpu_layers.clamp(0, 999);
        self.llama_ctx_size = self.llama_ctx_size.clamp(512, 1_048_576);
        self.janas_attention = normalize_janas_attention(&self.janas_attention);
        self.janas_kv_cache = normalize_janas_kv_cache(&self.janas_kv_cache);
        self.llama_flash_attn = normalize_flash_attn(&self.llama_flash_attn);
        self.llama_kv_cache_k = normalize_kv_cache(&self.llama_kv_cache_k);
        self.llama_kv_cache_v = normalize_kv_cache(&self.llama_kv_cache_v);
        self.llama_cpu_threads = self.llama_cpu_threads.min(256);
        self.llama_model_path = self.llama_model_path.trim().to_string();
        self.llama_mmproj_path = self.llama_mmproj_path.trim().to_string();
        self.llama_extra_args = self.llama_extra_args.trim().to_string();
        self.ssd_cache_dir = self.ssd_cache_dir.trim().to_string();
        self.top_k = self.top_k.clamp(1, 100);
    }

    /// Align the offload setting with the layers the selected model actually
    /// contains. This is deliberately best-effort: an unreadable model must
    /// never prevent unrelated settings from saving.
    pub fn clamp_gpu_layers_to_model(&mut self) {
        if self.llama_gpu_layers <= 0 {
            return;
        }
        if let Some(layers) = crate::gguf::layer_count(&self.llama_model_path) {
            self.llama_gpu_layers = self
                .llama_gpu_layers
                .min(layers.min(i32::MAX as u32) as i32);
        }
    }
}

pub const KV_CACHE_TYPES: [&str; 9] = [
    "f32", "f16", "bf16", "q8_0", "q4_0", "q4_1", "iq4_nl", "q5_0", "q5_1",
];

pub fn normalize_flash_attn(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" => "on".into(),
        "off" => "off".into(),
        _ => "auto".into(),
    }
}

pub fn normalize_kv_cache(value: &str) -> String {
    let lower = value.trim().to_ascii_lowercase();
    if KV_CACHE_TYPES.contains(&lower.as_str()) {
        lower
    } else {
        "f16".into()
    }
}

pub fn normalize_janas_attention(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "exact" | "float" => "exact".into(),
        "fast" | "int16" => "fast".into(),
        _ => "auto".into(),
    }
}

pub fn normalize_janas_kv_cache(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "8" | "8-bit" | "8bit" => "8".into(),
        "16" | "16-bit" | "16bit" => "16".into(),
        _ => "auto".into(),
    }
}

pub struct SettingsStore {
    path: PathBuf,
    settings: RwLock<AppSettings>,
}

impl SettingsStore {
    pub fn load(config_dir: PathBuf, data_dir: PathBuf) -> AppResult<Self> {
        fs::create_dir_all(&config_dir)?;
        let default_models = data_dir.join("models");
        fs::create_dir_all(&default_models)?;

        let path = config_dir.join("settings.json");
        let mut defaults = AppSettings::default();
        defaults.models_dir = default_models.to_string_lossy().to_string();

        let mut settings = if path.exists() {
            match fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str::<AppSettings>(&raw).ok())
            {
                Some(stored) => stored,
                None => defaults,
            }
        } else {
            defaults
        };

        if settings.models_dir.trim().is_empty() {
            settings.models_dir = default_models.to_string_lossy().to_string();
        }
        settings.normalize();
        settings.clamp_gpu_layers_to_model();
        if settings.host.trim().is_empty() {
            settings.host = "127.0.0.1".into();
        }
        let _ = fs::create_dir_all(&settings.models_dir);

        // A first run is friendlier when an already-converted model is picked
        // up automatically instead of asking the user to type a path.
        if settings.model_path.trim().is_empty() {
            if let Some(first) = scan_models(&settings.models_dir, None, 3).into_iter().next() {
                settings.model_path = first.path;
            }
        }

        let store = Self {
            path,
            settings: RwLock::new(settings),
        };
        store.persist();
        Ok(store)
    }

    pub fn get(&self) -> AppSettings {
        self.settings
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn save(&self, next: AppSettings) -> AppResult<AppSettings> {
        let mut clean = next;
        clean.normalize();
        clean.clamp_gpu_layers_to_model();
        clean.host = if clean.host.trim().is_empty() {
            "127.0.0.1".into()
        } else {
            clean.host.trim().to_string()
        };
        if clean.models_dir.trim().is_empty() {
            return Err(AppError::message("The models folder cannot be empty."));
        }
        fs::create_dir_all(clean.models_dir.trim())?;
        clean.models_dir = clean.models_dir.trim().to_string();

        {
            let mut guard = self
                .settings
                .write()
                .map_err(|_| AppError::message("Settings are locked by another operation."))?;
            *guard = clean.clone();
        }
        self.persist();
        Ok(clean)
    }

    fn persist(&self) {
        let current = self.get();
        let Ok(json) = serde_json::to_string_pretty(&current) else {
            return;
        };
        let tmp = self.path.with_extension("json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &self.path);
        }
    }
}

pub fn expand_home(input: &str) -> PathBuf {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        return Path::new(&home).join(rest);
    }
    PathBuf::from(trimmed)
}

/// The bundle's resource directory, set once at startup. Binaries shipped
/// inside the AppImage are found here so the user never has to link them.
static BUNDLED_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_bundled_dir(path: PathBuf) {
    let _ = BUNDLED_DIR.set(path);
}

fn bundled_binary(name: &str) -> Option<PathBuf> {
    let dir = BUNDLED_DIR.get()?;
    [dir.join(name), dir.join("binaries").join(name)]
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Resolve a binary specified either as a path or as a bare name on PATH.
pub fn resolve_binary(input: &str) -> Option<PathBuf> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = expand_home(trimmed);
    if candidate.components().count() > 1 || candidate.is_absolute() {
        return candidate.is_file().then_some(candidate);
    }

    let path_var = std::env::var_os("PATH")?;
    let mut names = vec![trimmed.to_string()];
    if cfg!(windows) && !trimmed.to_ascii_lowercase().ends_with(".exe") {
        names.push(format!("{trimmed}.exe"));
    }
    for dir in std::env::split_paths(&path_var) {
        for name in &names {
            let full = dir.join(name);
            if full.is_file() {
                return Some(full);
            }
        }
    }
    for name in &names {
        if let Some(full) = bundled_binary(name) {
            return Some(full);
        }
    }
    None
}

#[derive(Debug, Clone, Serialize)]
pub struct BinaryProbe {
    pub ok: bool,
    pub resolved_path: Option<String>,
    pub version: Option<String>,
    pub message: String,
}

/// Run `<binary> --version` with a short deadline, then report what was found.
/// A non-zero exit is not fatal: many local tools only implement `--help`.
pub fn probe_binary(input: &str) -> BinaryProbe {
    let Some(path) = resolve_binary(input) else {
        return BinaryProbe {
            ok: false,
            resolved_path: None,
            version: None,
            message: "Not found on PATH or at this location.".into(),
        };
    };

    let mut command = Command::new(&path);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }

    let resolved = path.to_string_lossy().to_string();
    let Ok(mut child) = command.spawn() else {
        return BinaryProbe {
            ok: true,
            resolved_path: Some(resolved.clone()),
            version: None,
            message: "Binary found, but it could not be executed.".into(),
        };
    };

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                break;
            }
            Err(_) => break,
        }
    }

    let output = child.wait_with_output().ok();
    let version = output
        .as_ref()
        .and_then(|out| {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let text = if stdout.trim().is_empty() {
                stderr.to_string()
            } else {
                stdout.to_string()
            };
            text.lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(|line| line.chars().take(120).collect::<String>())
        });

    BinaryProbe {
        ok: true,
        resolved_path: Some(resolved),
        version,
        message: "Binary found and executable.".into(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalModel {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
}

/// Find `.jns` files under the models folder (and an explicitly selected file).
pub fn scan_models(dir: &str, extra_file: Option<&str>, max_depth: usize) -> Vec<LocalModel> {
    fn walk(dir: &Path, depth: usize, max_depth: usize, out: &mut Vec<LocalModel>) {
        if depth > max_depth || out.len() >= 200 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, depth + 1, max_depth, out);
                continue;
            }
            if path
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("jns"))
                .unwrap_or(false)
            {
                let size_bytes = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| "model.jns".into());
                out.push(LocalModel {
                    name,
                    path: path.to_string_lossy().to_string(),
                    size_bytes,
                });
            }
        }
    }

    let mut found = Vec::new();
    if !dir.trim().is_empty() {
        walk(&expand_home(dir), 0, max_depth, &mut found);
    }
    if let Some(extra) = extra_file.filter(|value| !value.trim().is_empty()) {
        let path = expand_home(extra);
        if path.is_file()
            && path
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("jns"))
                .unwrap_or(false)
            && !found.iter().any(|model| Path::new(&model.path) == path)
        {
            let size_bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            found.push(LocalModel {
                name: path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| "model.jns".into()),
                path: path.to_string_lossy().to_string(),
                size_bytes,
            });
        }
    }

    found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    found
}

