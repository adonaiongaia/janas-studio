//! Dual-engine orchestration.
//!
//! Janas remains the studio's original engine. llama.cpp is added as a second,
//! runtime-downloaded engine. Both are driven through the same [`EngineRunner`]
//! trait so the UI only has to ask for a target engine, never for a process.
//!
//! Binaries never live in the AppImage mount. They are resolved from
//! `app_local_data_dir()/bin`, which is writable even when the AppImage itself
//! is mounted read-only under `/tmp/.mount_*`.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, RwLock},
    time::Duration,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::{watch, Mutex},
};

use crate::{
    engine::{EngineLog, EngineManager, EngineSnapshot, EngineStatus},
    error::{AppError, AppResult},
    settings::AppSettings,
    util::{now_ms, truncate_line},
};

pub mod binary;
pub mod janas;
pub mod llama;

const ENGINE_PID_FILE: &str = "engine.pid";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    Janas,
    Llama,
}

impl EngineKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Janas => "Janas",
            Self::Llama => "llama.cpp",
        }
    }

    pub fn storage_name(self) -> &'static str {
        match self {
            Self::Janas => "janas",
            Self::Llama => "llama",
        }
    }

    pub fn binary_stem(self) -> &'static str {
        match self {
            Self::Janas => "janas-server",
            Self::Llama => "llama-server",
        }
    }

    pub fn binary_name(self) -> String {
        if cfg!(windows) {
            format!("{}.exe", self.binary_stem())
        } else {
            self.binary_stem().to_string()
        }
    }

    pub fn parse_public(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "janas" | "janas-server" | "janas_server" => Some(Self::Janas),
            "llama" | "llama.cpp" | "llamacpp" | "llama-server" | "llama_server" => {
                Some(Self::Llama)
            }
            _ => None,
        }
    }
}

/// Public, serializable result of probing an engine binary.
#[derive(Debug, Clone, Serialize, Default)]
pub struct EngineProbe {
    pub version: Option<String>,
    pub build: Option<u64>,
    pub adapter: String,
    pub warning: Option<String>,
}

/// One running engine. The supervisor owns exactly one of these.
#[derive(Clone)]
pub struct EngineLaunch {
    pub kind: EngineKind,
    pub pid: u32,
    pub host: String,
    pub port: u16,
    pub model: String,
    /// Context the child was actually launched with, not the editable setting.
    pub context_window: u32,
    pub started_at_ms: u64,
    pub probe: EngineProbe,
    /// True when the binary can expose Prometheus metrics and the adapter
    /// enabled them. Janas reports through `/proc` only for now.
    pub metrics_enabled: bool,
    /// Resolves when the child has been reaped. `None` for Janas, whose
    /// lifecycle is owned by the existing `EngineManager`.
    pub exit_rx: Option<watch::Receiver<Option<i32>>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActiveEngineInfo {
    pub kind: EngineKind,
    pub pid: u32,
    pub host: String,
    pub port: u16,
    pub model: String,
    /// Context allocated by the running child. The UI budgets against this so
    /// editing the setting without restarting cannot overrun the server.
    pub context_window: u32,
    pub version: Option<String>,
    pub adapter: String,
    pub warning: Option<String>,
    pub started_at_ms: u64,
}

impl From<&EngineLaunch> for ActiveEngineInfo {
    fn from(launch: &EngineLaunch) -> Self {
        Self {
            kind: launch.kind,
            pid: launch.pid,
            host: launch.host.clone(),
            port: launch.port,
            model: launch.model.clone(),
            context_window: launch.context_window,
            version: launch.probe.version.clone(),
            adapter: launch.probe.adapter.clone(),
            warning: launch.probe.warning.clone(),
            started_at_ms: launch.started_at_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineTelemetry {
    pub engine: EngineKind,
    pub pid: Option<u32>,
    pub tokens_per_second: f64,
    pub prompt_tokens_total: u64,
    pub completion_tokens_total: u64,
    pub kv_cache_usage: f64,
    pub read_bytes_per_sec: u64,
    pub read_ops_per_sec: u64,
    pub rss_bytes: u64,
    pub sampled_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdapterWarningEvent {
    pub engine: EngineKind,
    pub message: String,
    pub version: Option<String>,
    pub adapter: String,
}

#[async_trait]
pub trait EngineRunner<R: Runtime>: Send + Sync {
    /// Run the binary's `--version` / `--help` before any launch and select a
    /// CLI layout from what the binary actually advertises.
    async fn probe(
        &self,
        app: &AppHandle<R>,
        settings: &AppSettings,
    ) -> AppResult<EngineProbe>;

    /// Start the engine and begin forwarding stdout/stderr.
    async fn spawn_process(
        &self,
        app: &AppHandle<R>,
        settings: &AppSettings,
    ) -> AppResult<EngineLaunch>;

    /// Stop the process tree. Implementations are idempotent.
    async fn kill_process(&self, app: &AppHandle<R>, launch: &EngineLaunch) -> AppResult<()>;

    /// Publish live counters until the process exits.
    async fn monitor_telemetry(
        &self,
        app: &AppHandle<R>,
        launch: EngineLaunch,
    ) -> AppResult<()>;
}

struct ActiveEngine {
    info: ActiveEngineInfo,
    launch: EngineLaunch,
}

pub struct EngineSupervisor<R: Runtime = tauri::Wry> {
    active: Mutex<Option<ActiveEngine>>,
    /// Serializes switches. It is deliberately held across process shutdown
    /// and launch so two engines can never overlap.
    switch_lock: Mutex<()>,
    snapshot: RwLock<EngineSnapshot>,
    preferred: RwLock<EngineKind>,
    janas: Arc<EngineManager>,
    janas_runner: Arc<dyn EngineRunner<R>>,
    llama_runner: Arc<dyn EngineRunner<R>>,
}

impl EngineSupervisor<tauri::Wry> {
    pub fn new(janas: Arc<EngineManager>) -> Self {
        let snapshot = janas.snapshot();
        Self {
            active: Mutex::new(None),
            switch_lock: Mutex::new(()),
            snapshot: RwLock::new(snapshot),
            preferred: RwLock::new(EngineKind::Janas),
            janas_runner: Arc::new(janas::JanasRunner::new(janas.clone())),
            llama_runner: Arc::new(llama::LlamaRunner::new()),
            janas,
        }
    }
}

#[cfg(test)]
impl EngineSupervisor<tauri::test::MockRuntime> {
    pub fn new_mock(janas: Arc<EngineManager>) -> Self {
        let snapshot = janas.snapshot();
        Self {
            active: Mutex::new(None),
            switch_lock: Mutex::new(()),
            snapshot: RwLock::new(snapshot),
            preferred: RwLock::new(EngineKind::Llama),
            janas_runner: Arc::new(janas::JanasRunner::new(janas.clone())),
            llama_runner: Arc::new(llama::LlamaRunner::new()),
            janas,
        }
    }
}

impl<R: Runtime> EngineSupervisor<R> {
    pub fn preferred_kind(&self) -> EngineKind {
        *self
            .preferred
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn set_preferred(&self, kind: EngineKind) {
        if let Ok(mut guard) = self.preferred.write() {
            *guard = kind;
        }
    }

    pub async fn active_info(&self) -> Option<ActiveEngineInfo> {
        self.active
            .lock()
            .await
            .as_ref()
            .map(|active| active.info.clone())
    }

    /// The command layer uses this to refuse replacing a binary that is still
    /// mapped into a running process.
    pub async fn ensure_inactive(&self, kind: EngineKind) -> AppResult<()> {
        if let Some(active) = self.active_info().await {
            if active.kind == kind {
                return Err(AppError::message(format!(
                    "{} is still running (pid {}). Stop it before updating or rolling back its binary.",
                    kind.label(),
                    active.pid
                )));
            }
        }
        Ok(())
    }

    pub async fn snapshot(&self) -> EngineSnapshot {
        let active = self.active.lock().await;
        if let Some(active) = active.as_ref() {
            if active.info.kind == EngineKind::Janas {
                return self.janas.snapshot();
            }
        }
        self.snapshot
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    pub async fn switch(
        &self,
        app: &AppHandle<R>,
        settings: &AppSettings,
        target: Option<EngineKind>,
    ) -> AppResult<EngineSnapshot> {
        let _switch = self.switch_lock.lock().await;
        let previous = self.active.lock().await.take();

        if let Some(active) = previous {
            if target == Some(active.info.kind) && self.process_alive(&active) {
                *self.active.lock().await = Some(active);
                return Ok(self.snapshot().await);
            }

            let runner = self.runner(active.info.kind);
            let stop_result = runner.kill_process(app, &active.launch).await;
            if let Err(err) = stop_result {
                if pid_alive(active.launch.pid) {
                    let label = active.info.kind.label();
                    *self.active.lock().await = Some(active);
                    return Err(AppError::message(format!(
                        "Could not stop {label}. The other engine was not started because the hardware would stay locked: {err}"
                    )));
                }
            }
            remove_pidfile(app);
            self.publish_active(app, None);
        }

        if target.is_none() {
            let port = settings.port;
            let snapshot = EngineSnapshot {
                status: EngineStatus::Disconnected,
                detail: "Engine stopped".into(),
                pid: None,
                model: None,
                port,
                started_at_ms: None,
            };
            self.publish_snapshot(app, snapshot.clone());
            return Ok(snapshot);
        }

        let kind = target.expect("checked above");
        let runner = self.runner(kind);
        let probe = match runner.probe(app, settings).await {
            Ok(probe) => probe,
            Err(err) => {
                self.publish_start_failure(app, kind, settings, &err.to_string());
                return Err(err);
            }
        };

        if let Some(warning) = probe.warning.as_deref().filter(|text| !text.is_empty()) {
            let _ = app.emit(
                "engine://adapter-warning",
                AdapterWarningEvent {
                    engine: kind,
                    message: warning.to_string(),
                    version: probe.version.clone(),
                    adapter: probe.adapter.clone(),
                },
            );
        }

        let launch = match runner.spawn_process(app, settings).await {
            Ok(launch) => launch,
            Err(err) => {
                self.publish_start_failure(app, kind, settings, &err.to_string());
                return Err(err);
            }
        };

        let info = ActiveEngineInfo::from(&launch);
        *self.active.lock().await = Some(ActiveEngine {
            info: info.clone(),
            launch: launch.clone(),
        });
        self.set_preferred(kind);

        write_pidfile(app, &launch);

        let detail = match kind {
            EngineKind::Janas => format!(
                "{} is reading the model on port {}",
                kind.label(),
                launch.port
            ),
            EngineKind::Llama => format!(
                "{} is loading the model on port {}",
                kind.label(),
                launch.port
            ),
        };
        self.publish_snapshot(app, snapshot_from_launch(&launch, EngineStatus::LoadingModel, detail));
        self.publish_active(app, Some(info));

        let runner_for_monitor = runner.clone();
        let app_for_monitor = app.clone();
        let launch_for_monitor = launch.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(err) = runner_for_monitor
                .monitor_telemetry(&app_for_monitor, launch_for_monitor)
                .await
            {
                let _ = app_for_monitor.emit(
                    "engine://log",
                    EngineLog {
                        stream: "system".into(),
                        line: format!("Telemetry monitor stopped: {err}"),
                        timestamp_ms: now_ms(),
                    },
                );
            }
        });

        Ok(self.snapshot().await)
    }

    fn process_alive(&self, active: &ActiveEngine) -> bool {
        match active.info.kind {
            EngineKind::Janas => self.janas.is_running(),
            EngineKind::Llama => pid_alive(active.info.pid),
        }
    }

    fn runner(&self, kind: EngineKind) -> Arc<dyn EngineRunner<R>> {
        match kind {
            EngineKind::Janas => self.janas_runner.clone(),
            EngineKind::Llama => self.llama_runner.clone(),
        }
    }

    fn publish_start_failure(
        &self,
        app: &AppHandle<R>,
        kind: EngineKind,
        settings: &AppSettings,
        detail: &str,
    ) {
        let port = match kind {
            EngineKind::Janas => settings.port,
            EngineKind::Llama => settings.llama_port,
        };
        let model = match kind {
            EngineKind::Janas => settings.model_display_name(),
            EngineKind::Llama => settings.llama_model_display_name(),
        };
        self.publish_snapshot(
            app,
            EngineSnapshot {
                status: EngineStatus::Disconnected,
                detail: detail.to_string(),
                pid: None,
                model,
                port,
                started_at_ms: None,
            },
        );
        self.publish_active(app, None);
    }

    fn publish_snapshot(&self, app: &AppHandle<R>, snapshot: EngineSnapshot) {
        if let Ok(mut guard) = self.snapshot.write() {
            *guard = snapshot.clone();
        }
        let _ = app.emit("engine://status", snapshot);
    }

    fn publish_active(&self, app: &AppHandle<R>, info: Option<ActiveEngineInfo>) {
        let _ = app.emit("engine://active", info);
    }

    pub async fn mark_llama_ready(&self, app: &AppHandle<R>, pid: u32) {
        let guard = self.active.lock().await;
        let Some(active) = guard.as_ref() else {
            return;
        };
        if active.info.kind != EngineKind::Llama || active.info.pid != pid {
            return;
        }
        let snapshot = snapshot_from_launch(
            &active.launch,
            EngineStatus::Ready,
            format!(
                "llama.cpp is ready on {}:{}",
                active.launch.host, active.launch.port
            ),
        );
        drop(guard);
        self.publish_snapshot(app, snapshot);
    }

    pub async fn mark_llama_failed(&self, app: &AppHandle<R>, pid: u32, detail: impl Into<String>) {
        let mut guard = self.active.lock().await;
        let matches = guard
            .as_ref()
            .map(|active| active.info.kind == EngineKind::Llama && active.info.pid == pid)
            .unwrap_or(false);
        if !matches {
            return;
        }
        let active = guard.take().expect("checked above");
        drop(guard);
        self.publish_active(app, None);
        self.publish_snapshot(
            app,
            EngineSnapshot {
                status: EngineStatus::Disconnected,
                detail: detail.into(),
                pid: None,
                model: Some(active.info.model),
                port: active.info.port,
                started_at_ms: None,
            },
        );
    }

    /// Called by a child reaper. Returns true only if this PID was the active
    /// engine, which prevents an old reaper from clearing a newly started one.
    pub async fn clear_if_pid(&self, app: &AppHandle<R>, pid: u32, code: Option<i32>) -> bool {
        let mut guard = self.active.lock().await;
        let matches = guard
            .as_ref()
            .map(|active| active.info.pid == pid)
            .unwrap_or(false);
        if !matches {
            return false;
        }
        let active = guard.take().expect("checked above");
        drop(guard);

        remove_pidfile(app);
        self.publish_active(app, None);
        let detail = match code {
            Some(0) => format!("{} exited cleanly.", active.info.kind.label()),
            Some(code) => format!(
                "{} exited with code {code}. Check the engine log for details.",
                active.info.kind.label()
            ),
            None => format!(
                "{} exited unexpectedly. Check the engine log for details.",
                active.info.kind.label()
            ),
        };
        self.publish_snapshot(
            app,
            EngineSnapshot {
                status: EngineStatus::Disconnected,
                detail,
                pid: None,
                model: Some(active.info.model),
                port: active.info.port,
                started_at_ms: None,
            },
        );
        let _ = app.emit(
            "engine://exited",
            serde_json::json!({ "engine": active.info.kind, "pid": pid }),
        );
        true
    }

    pub async fn shutdown(&self, app: &AppHandle<R>) {
        let _switch = self.switch_lock.lock().await;
        let active = self.active.lock().await.take();
        if let Some(active) = active {
            let runner = self.runner(active.info.kind);
            let _ = runner.kill_process(app, &active.launch).await;
        }
        remove_pidfile(app);
    }

    pub fn shutdown_blocking(&self, app: &AppHandle<R>) {
        let app = app.clone();
        let _ = tauri::async_runtime::block_on(async {
            tokio::time::timeout(Duration::from_secs(12), self.shutdown(&app)).await
        });
    }
}

fn snapshot_from_launch(
    launch: &EngineLaunch,
    status: EngineStatus,
    detail: impl Into<String>,
) -> EngineSnapshot {
    EngineSnapshot {
        status,
        detail: detail.into(),
        pid: Some(launch.pid),
        model: Some(launch.model.clone()),
        port: launch.port,
        started_at_ms: Some(launch.started_at_ms),
    }
}

// ---------------------------------------------------------------------------
// Managed paths
// ---------------------------------------------------------------------------

#[cfg(test)]
static TEST_BIN_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Point managed-path tests at a disposable directory instead of the real
/// application data directory. Production builds do not compile this.
#[cfg(test)]
pub fn set_test_bin_dir(path: PathBuf) {
    let _ = TEST_BIN_DIR.set(path);
}

/// The single writable root for engine binaries, backups, staging files and
/// the runtime pid file.
pub fn managed_bin_dir<R: Runtime>(app: &AppHandle<R>) -> AppResult<PathBuf> {
    #[cfg(test)]
    if let Some(dir) = TEST_BIN_DIR.get() {
        fs::create_dir_all(dir)
            .map_err(|err| io_error("Could not create the test engine bin directory", dir, err))?;
        return Ok(dir.clone());
    }

    let data = app.path().app_local_data_dir().map_err(|err| {
        AppError::message(format!(
            "Could not resolve the app's local data directory: {err}. On Linux this is normally ~/.local/share/dev.janas.studio."
        ))
    })?;
    let dir = data.join("bin");
    fs::create_dir_all(&dir).map_err(|err| io_error("Could not create the engine bin directory", &dir, err))?;
    Ok(dir)
}

pub fn managed_binary_path<R: Runtime>(app: &AppHandle<R>, kind: EngineKind) -> AppResult<PathBuf> {
    Ok(managed_bin_dir(app)?.join(kind.binary_name()))
}

pub fn backup_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "engine".into());
    target.with_file_name(format!("{name}.bak"))
}

fn runtime_dir<R: Runtime>(app: &AppHandle<R>) -> AppResult<PathBuf> {
    let dir = managed_bin_dir(app)?.join(".runtime");
    fs::create_dir_all(&dir)
        .map_err(|err| io_error("Could not create the engine runtime directory", &dir, err))?;
    Ok(dir)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PidFile {
    kind: EngineKind,
    pid: u32,
    binary: String,
    started_at_ms: u64,
}

pub(crate) fn write_pidfile<R: Runtime>(app: &AppHandle<R>, launch: &EngineLaunch) {
    let Ok(dir) = runtime_dir(app) else {
        return;
    };
    let binary = managed_binary_path(app, launch.kind)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let payload = PidFile {
        kind: launch.kind,
        pid: launch.pid,
        binary,
        started_at_ms: launch.started_at_ms,
    };
    let Ok(json) = serde_json::to_vec_pretty(&payload) else {
        return;
    };
    let path = dir.join(ENGINE_PID_FILE);
    let _ = fs::write(&path, json);
}

pub fn remove_pidfile<R: Runtime>(app: &AppHandle<R>) {
    if let Ok(dir) = runtime_dir(app) {
        let _ = fs::remove_file(dir.join(ENGINE_PID_FILE));
    }
}

/// Kill a process that survived a crash before the new process starts. It only
/// touches a PID when `/proc/<pid>/cmdline` still names the recorded engine
/// binary, so PID reuse cannot kill an unrelated program.
pub fn cleanup_stale_processes<R: Runtime>(app: &AppHandle<R>) -> AppResult<()> {
    let bin = managed_bin_dir(app)?;
    let runtime = runtime_dir(app)?;
    let pid_path = runtime.join(ENGINE_PID_FILE);

    if let Ok(raw) = fs::read_to_string(&pid_path) {
        if let Ok(record) = serde_json::from_str::<PidFile>(&raw) {
            if record.pid > 1 && pid_alive(record.pid) && process_matches(record.pid, &record.binary) {
                signal_group(record.pid, libc::SIGTERM);
                for _ in 0..10 {
                    if !pid_alive(record.pid) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(150));
                }
                if pid_alive(record.pid) {
                    signal_group(record.pid, libc::SIGKILL);
                }
            }
        }
        let _ = fs::remove_file(&pid_path);
    }

    // A crash mid-install leaves a staging directory. Nothing else can be using
    // it during startup, so it is safe to discard.
    let staging = bin.join(".staging");
    if let Ok(entries) = fs::read_dir(&staging) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn process_matches(pid: u32, binary: &str) -> bool {
    let Ok(cmdline) = fs::read(format!("/proc/{pid}/cmdline")) else {
        return false;
    };
    let text = String::from_utf8_lossy(&cmdline);
    if text.is_empty() {
        return false;
    }
    let recorded = Path::new(binary);
    let Some(name) = recorded.file_name() else {
        return false;
    };
    text.contains(&*name.to_string_lossy()) || text.contains(binary)
}

#[cfg(not(target_os = "linux"))]
fn process_matches(_pid: u32, _binary: &str) -> bool {
    // PID reuse makes an unverified kill unsafe outside Linux's /proc. The
    // in-process reaper and normal exit path still stop engines correctly.
    false
}

// ---------------------------------------------------------------------------
// Process and telemetry helpers
// ---------------------------------------------------------------------------

pub fn pid_alive(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(pid as i32, 0) };
        if result == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        // A conservative fallback: the reaper clears state on its own. Windows
        // support can be tightened with OpenProcess without changing callers.
        true
    }
}

pub fn signal_group(pid: u32, signal: libc::c_int) {
    if pid <= 1 {
        return;
    }
    #[cfg(unix)]
    unsafe {
        // The child is started with process_group(0), so its PGID equals its
        // PID and the whole tree (including helper threads/processes) is hit.
        libc::kill(-(pid as i32), signal);
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, signal);
    }
}

pub async fn wait_for_exit(rx: &watch::Receiver<Option<i32>>, timeout: Duration) -> bool {
    let mut rx = rx.clone();
    if rx.borrow().is_some() {
        return true;
    }
    let wait = async {
        loop {
            if rx.changed().await.is_err() {
                return false;
            }
            if rx.borrow().is_some() {
                return true;
            }
        }
    };
    tokio::time::timeout(timeout, wait).await.unwrap_or(false)
}

pub fn spawn_log_reader<R, S>(app: AppHandle<R>, stream: S, stream_name: &'static str)
where
    R: Runtime,
    S: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tauri::async_runtime::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let _ = app.emit(
                        "engine://log",
                        EngineLog {
                            stream: stream_name.into(),
                            line: truncate_line(&line, 4_000),
                            timestamp_ms: now_ms(),
                        },
                    );
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
    });
}

#[cfg(target_os = "linux")]
pub fn process_counters(pid: u32) -> (u64, u64, u64) {
    let (read_bytes, read_ops) = fs::read_to_string(format!("/proc/{pid}/io"))
        .ok()
        .map(|raw| {
            let mut bytes = 0;
            let mut ops = 0;
            for line in raw.lines() {
                if let Some(value) = line.strip_prefix("read_bytes:") {
                    bytes = value.trim().parse().unwrap_or(0);
                } else if let Some(value) = line.strip_prefix("syscr:") {
                    ops = value.trim().parse().unwrap_or(0);
                }
            }
            (bytes, ops)
        })
        .unwrap_or((0, 0));

    let rss_bytes = fs::read_to_string(format!("/proc/{pid}/statm"))
        .ok()
        .and_then(|raw| raw.split_whitespace().nth(1).and_then(|value| value.parse::<u64>().ok()))
        .map(|pages| pages.saturating_mul(4_096))
        .unwrap_or(0);

    (read_bytes, read_ops, rss_bytes)
}

#[cfg(not(target_os = "linux"))]
pub fn process_counters(_pid: u32) -> (u64, u64, u64) {
    (0, 0, 0)
}

pub async fn run_cli_probe(binary: &Path, lib_dir: Option<&Path>) -> AppResult<ProbeOutput> {
    let (version_text, version_code) = run_flag(binary, "--version", lib_dir).await?;
    let (help_text, _help_code) = match run_flag(binary, "--help", lib_dir).await {
        Ok(value) => value,
        Err(_) => (String::new(), None),
    };

    if version_code == Some(127) {
        return Err(shared_library_error(binary, lib_dir));
    }

    let build = parse_build(&version_text).or_else(|| parse_build(&help_text));
    let version_line = first_version_line(&version_text)
        .or_else(|| first_version_line(&help_text))
        .map(|line| truncate_line(&line, 180));

    Ok(ProbeOutput {
        version_line,
        build,
        help_text,
    })
}

#[derive(Debug, Clone, Default)]
pub struct ProbeOutput {
    pub version_line: Option<String>,
    pub build: Option<u64>,
    pub help_text: String,
}

async fn run_flag(
    binary: &Path,
    flag: &str,
    lib_dir: Option<&Path>,
) -> AppResult<(String, Option<i32>)> {
    let mut command = tokio::process::Command::new(binary);
    command
        .arg(flag)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    apply_library_path(&mut command, lib_dir);

    let output = tokio::time::timeout(Duration::from_secs(6), command.output())
        .await
        .map_err(|_| {
            AppError::message(format!(
                "{} {flag} did not finish within 6 seconds. The binary may be waiting for a model or a missing library.",
                binary.display()
            ))
        })?
        .map_err(|err| probe_spawn_error(binary, lib_dir, err))?;

    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        if !text.trim().is_empty() {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    Ok((text, output.status.code()))
}

#[cfg(target_os = "linux")]
pub(crate) fn apply_library_path(command: &mut tokio::process::Command, lib_dir: Option<&Path>) {
    let Some(lib_dir) = lib_dir else {
        return;
    };
    let mut paths = vec![lib_dir.to_path_buf()];
    if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        command.env("LD_LIBRARY_PATH", joined);
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn apply_library_path(_command: &mut tokio::process::Command, _lib_dir: Option<&Path>) {}

pub(crate) fn probe_spawn_error(binary: &Path, lib_dir: Option<&Path>, err: std::io::Error) -> AppError {
    match err.kind() {
        std::io::ErrorKind::NotFound => shared_library_error(binary, lib_dir),
        std::io::ErrorKind::PermissionDenied => AppError::message(format!(
            "{} is not executable. Run `chmod 755 {}` or reinstall the binary from Settings.",
            binary.display(),
            binary.display()
        )),
        _ if err.raw_os_error() == Some(8) => AppError::message(format!(
            "{} was built for a different CPU architecture. Download the release that matches this machine.",
            binary.display()
        )),
        _ => AppError::message(format!(
            "Could not execute {}: {err}. Keep the release's shared libraries beside the binary in the managed bin folder.",
            binary.display()
        )),
    }
}

fn shared_library_error(binary: &Path, lib_dir: Option<&Path>) -> AppError {
    let dir = lib_dir
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "the managed bin folder".into());
    AppError::message(format!(
        "{} could not be executed because a runtime library is missing. llama.cpp release archives include libllama*/libggml* files; keep them in {dir} next to the binary, then retry.",
        binary.display()
    ))
}

pub fn parse_build(text: &str) -> Option<u64> {
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        for marker in ["version:", "build:", "version ", "build "] {
            if let Some(index) = lower.find(marker) {
                let rest = &line[index + marker.len()..];
                if let Some(value) = first_digit_run(rest) {
                    return Some(value);
                }
            }
        }
    }
    for token in text.split(|character: char| !character.is_ascii_alphanumeric()) {
        if let Some(digits) = token.strip_prefix('b') {
            if digits.len() >= 3 && digits.chars().all(|character| character.is_ascii_digit()) {
                if let Ok(value) = digits.parse::<u64>() {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn first_digit_run(text: &str) -> Option<u64> {
    let start = text.find(|character: char| character.is_ascii_digit())?;
    let digits: String = text[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    if digits.len() < 3 {
        return None;
    }
    digits.parse().ok()
}

fn first_version_line(text: &str) -> Option<String> {
    let candidate = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .find(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("version") || lower.contains("build")
        })
        .or_else(|| text.lines().map(str::trim).find(|line| !line.is_empty()))?;
    Some(candidate.to_string())
}

/// True when `help` advertises an exact flag token. Substring checks are too
/// loose: `-c` appears inside `--cache-type-k` and would otherwise select the
/// wrong context flag.
pub(crate) fn advertises(help: &str, flag: &str) -> bool {
    help.split(|character: char| {
        character.is_whitespace()
            || matches!(character, ',' | '[' | ']' | '|' | '(' | ')' | '{' | '}')
    })
    .any(|token| token == flag)
}

pub fn combine_warnings(base: Option<String>, extra: Vec<String>) -> Option<String> {
    let mut all = Vec::new();
    if let Some(base) = base.filter(|text| !text.trim().is_empty()) {
        all.push(base);
    }
    all.extend(extra.into_iter().filter(|text| !text.trim().is_empty()));
    if all.is_empty() {
        None
    } else {
        Some(truncate_line(&all.join(" "), 420))
    }
}

pub fn io_error(action: &str, path: &Path, err: std::io::Error) -> AppError {
    match err.kind() {
        std::io::ErrorKind::NotFound => AppError::message(format!(
            "{action}: {} was not found. Create the folder or reinstall the binary from Settings.",
            path.display()
        )),
        std::io::ErrorKind::PermissionDenied => AppError::message(format!(
            "{action}: permission denied for {}. Check the owner and mode (chmod 755) in the managed bin folder.",
            path.display()
        )),
        _ => AppError::message(format!("{action}: {}: {err}", path.display())),
    }
}

