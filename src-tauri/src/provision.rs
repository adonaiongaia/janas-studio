//! Automatic engine provisioning.
//!
//! The app ships no llama.cpp binary and its read-only AppImage mount can never
//! be used for installs. This module runs once in the background at startup,
//! detects the host, resolves the matching release assets, and places the
//! engines in `app_local_data_dir()/bin`.
//!
//! The managed layout is deliberately the same as the one used by
//! [`crate::engines::binary`]:
//!
//! ```text
//! bin/
//!   llama-server
//!   janas-server
//!   janas-chat
//!   *.so / *.dylib / *.dll
//!   .staging/<uuid>/
//! ```
//!
//! `llama-server` goes through the existing verified installer, which performs
//! extraction guards, an executable probe and one-step rollback. Janas release
//! archives contain several executables, so the Janas path extracts once and
//! installs each requested tool atomically with the same path-safety checks.

use std::{
    env,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    util::{now_ms, truncate_line},
};

use crate::engines::{
    binary::{self, BinaryManager},
    managed_bin_dir, managed_binary_path, run_cli_probe, EngineKind, EngineSupervisor,
};

pub const SETUP_STATUS_EVENT: &str = "setup://status";
// The project moved from ggerganov/llama.cpp to its current canonical owner.
// Using the canonical path avoids an API redirect on every release check.
const LLAMA_REPO: &str = "ggml-org/llama.cpp";
const JANAS_REPO: &str = "prabanta-dev/janas";
const GITHUB_API: &str = "https://api.github.com";
const RELEASE_LIST_LIMIT: usize = 30;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupStatus {
    #[default]
    Idle,
    Running,
    Ready,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentStatus {
    #[default]
    Pending,
    Present,
    Installed,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GpuInfo {
    pub vendor: String,
    pub backend: String,
    pub name: String,
    pub cuda_version: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlatformProfile {
    pub os: String,
    pub arch: String,
    pub cpu_avx2: bool,
    pub apple_silicon: bool,
    pub gpus: Vec<GpuInfo>,
    pub preferred_backend: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReleaseChoice {
    pub repo: String,
    pub requested_tag: String,
    pub tag: String,
    pub asset: String,
    pub download_url: String,
    pub fallback: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProvisionedBinary {
    pub name: String,
    pub status: ComponentStatus,
    pub path: Option<String>,
    pub version: Option<String>,
    pub source: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupReport {
    pub status: SetupStatus,
    pub message: String,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
    pub platform: Option<PlatformProfile>,
    pub cache_dir: Option<String>,
    pub llama_release: Option<ReleaseChoice>,
    pub janas_release: Option<ReleaseChoice>,
    pub binaries: Vec<ProvisionedBinary>,
    pub warnings: Vec<String>,
}

impl Default for SetupReport {
    fn default() -> Self {
        Self {
            status: SetupStatus::Idle,
            message: "Engine setup has not run yet.".into(),
            started_at_ms: None,
            finished_at_ms: None,
            platform: None,
            cache_dir: None,
            llama_release: None,
            janas_release: None,
            binaries: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

/// Shared startup state. The mutex serializes startup provisioning and a manual
/// retry so two downloads cannot race over the same managed files.
pub struct Provisioner {
    report: RwLock<SetupReport>,
    gate: Mutex<()>,
}

impl Provisioner {
    pub fn new() -> Self {
        Self {
            report: RwLock::new(SetupReport::default()),
            gate: Mutex::new(()),
        }
    }

    pub fn report(&self) -> SetupReport {
        self.report
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn publish(&self, app: &AppHandle, report: SetupReport) {
        if let Ok(mut guard) = self.report.write() {
            *guard = report.clone();
        }
        let _ = app.emit(SETUP_STATUS_EVENT, report);
    }

    /// Detect the host and install whichever managed tools are missing. Network
    /// failures are represented in the report; they never prevent the app from
    /// starting with its bundled Janas tools.
    pub async fn run(
        &self,
        app: &AppHandle,
        manager: Arc<BinaryManager>,
        supervisor: Arc<EngineSupervisor>,
        force: bool,
    ) -> SetupReport {
        let _gate = self.gate.lock().await;
        let started = now_ms();
        let mut report = SetupReport {
            status: SetupStatus::Running,
            message: "Detecting hardware and checking managed engine binaries…".into(),
            started_at_ms: Some(started),
            ..SetupReport::default()
        };
        self.publish(app, report.clone());

        let platform = match tokio::task::spawn_blocking(detect_platform).await {
            Ok(profile) => profile,
            Err(err) => {
                report
                    .warnings
                    .push(format!("Hardware detection task failed: {err}"));
                PlatformProfile {
                    os: normalize_os(env::consts::OS),
                    arch: normalize_arch(env::consts::ARCH),
                    preferred_backend: "cpu".into(),
                    ..PlatformProfile::default()
                }
            }
        };
        report.platform = Some(platform.clone());

        match tokio::task::spawn_blocking(ensure_janas_cache).await {
            Ok(Ok(path)) => report.cache_dir = Some(path.to_string_lossy().to_string()),
            Ok(Err(err)) => report.warnings.push(format!(
                "Could not prepare the Janas cache directory: {err}"
            )),
            Err(err) => report
                .warnings
                .push(format!("Janas cache task failed: {err}")),
        }

        let (llama, llama_release) =
            provision_llama(app, manager, supervisor.clone(), &platform, force, &mut report.warnings)
                .await;
        report.llama_release = llama_release;
        report.binaries.push(llama);

        let (janas_binaries, janas_release) =
            provision_janas(app, supervisor, &platform, force, &mut report.warnings).await;
        report.janas_release = janas_release;
        report.binaries.extend(janas_binaries);

        report.finished_at_ms = Some(now_ms());
        report.status = overall_status(&report.binaries);
        report.message = match report.status {
            SetupStatus::Ready => "Managed engines are ready.".into(),
            SetupStatus::Degraded => {
                "One managed engine could not be provisioned; existing and bundled tools remain usable.".into()
            }
            SetupStatus::Failed => {
                "Managed engine provisioning failed. Existing and bundled tools remain usable.".into()
            }
            SetupStatus::Idle | SetupStatus::Running => "Engine setup is still running.".into(),
        };
        self.publish(app, report.clone());
        report
    }
}

fn overall_status(binaries: &[ProvisionedBinary]) -> SetupStatus {
    let failed = binaries
        .iter()
        .filter(|binary| binary.status == ComponentStatus::Failed)
        .count();
    if failed == 0 {
        return SetupStatus::Ready;
    }
    if failed == binaries.len() {
        SetupStatus::Failed
    } else {
        SetupStatus::Degraded
    }
}

async fn provision_llama(
    app: &AppHandle,
    manager: Arc<BinaryManager>,
    supervisor: Arc<EngineSupervisor>,
    profile: &PlatformProfile,
    force: bool,
    warnings: &mut Vec<String>,
) -> (ProvisionedBinary, Option<ReleaseChoice>) {
    let path = match managed_binary_path(app, EngineKind::Llama) {
        Ok(path) => path,
        Err(err) => return (failed_llama(err.to_string()), None),
    };
    let bin = match managed_bin_dir(app) {
        Ok(bin) => bin,
        Err(err) => return (failed_llama(err.to_string()), None),
    };
    let existing = path.is_file();
    let existing_probe = if existing {
        run_cli_probe(&path, Some(&bin))
            .await
            .ok()
            .filter(probe_is_usable)
    } else {
        None
    };

    let resolution = resolve_llama_release(profile).await;
    if existing && !force {
        if let Some(probe) = existing_probe {
            let message = match &resolution {
                Ok(resolution) => format!(
                    "Already installed. Latest usable release is {} ({}); this binary was left unchanged.",
                    resolution.release.tag_name, resolution.candidates[0].asset.name
                ),
                Err(err) => format!(
                    "Already installed; the release check failed and the working binary was left unchanged: {err}"
                ),
            };
            if let Err(err) = &resolution {
                warnings.push(format!("llama.cpp update check failed: {err}"));
            }
            return (
                ProvisionedBinary {
                    name: "llama-server".into(),
                    status: ComponentStatus::Present,
                    path: Some(path.to_string_lossy().to_string()),
                    version: probe.version_line,
                    source: Some("managed".into()),
                    message,
                },
                resolution.ok().map(|value| value.choice()),
            );
        }
    }

    if let Err(err) = &resolution {
        if existing {
            if let Some(probe) = existing_probe.as_ref() {
                warnings.push(format!(
                    "llama.cpp release check failed; the installed binary was kept: {err}"
                ));
                return (
                    ProvisionedBinary {
                        name: "llama-server".into(),
                        status: ComponentStatus::Present,
                        path: Some(path.to_string_lossy().to_string()),
                        version: probe.version_line.clone(),
                        source: Some("managed".into()),
                        message: "Release resolution failed; the working managed binary was kept."
                            .into(),
                    },
                    None,
                );
            }
        }
    }
    let resolution = match resolution {
        Ok(value) => value,
        Err(err) => {
            let message = format!("Could not find a llama.cpp release asset: {err}");
            warnings.push(message.clone());
            return (failed_llama(message), None);
        }
    };
    let choice = resolution.choice();
    let candidates = resolution.candidates.clone();
    let mut last_error = String::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let result = binary::install_engine_from_url(
            app,
            &manager,
            &supervisor,
            EngineKind::Llama,
            candidate.asset.browser_download_url.clone(),
        )
        .await;
        match result {
            Ok(installed) => {
                let message = if existing {
                    format!(
                        "Updated from {} ({}). Previous binary kept as {}.",
                        choice.tag,
                        candidate.asset.name,
                        installed.backup_path.as_deref().unwrap_or("none")
                    )
                } else {
                    format!(
                        "Installed from {} ({}).",
                        choice.tag, candidate.asset.name
                    )
                };
                return (
                    ProvisionedBinary {
                        name: "llama-server".into(),
                        status: ComponentStatus::Installed,
                        path: Some(installed.installed_path),
                        version: installed.version,
                        source: Some("github".into()),
                        message,
                    },
                    Some(choice),
                );
            }
            Err(err) => {
                last_error = err.to_string();
                if index + 1 < candidates.len() {
                    warnings.push(format!(
                        "The {} llama.cpp build was rejected ({}); trying the CPU fallback.",
                        candidate.backend, last_error
                    ));
                }
            }
        }
    }

    // A failed forced update is rolled back by binary.rs. If the old binary is
    // still healthy, report it as present rather than taking the engine offline.
    if path.is_file() {
        if let Ok(probe) = run_cli_probe(&path, Some(&bin)).await {
            warnings.push(format!(
                "llama.cpp update failed; the working previous binary was kept: {last_error}"
            ));
            return (
                ProvisionedBinary {
                    name: "llama-server".into(),
                    status: ComponentStatus::Present,
                    path: Some(path.to_string_lossy().to_string()),
                    version: probe.version_line,
                    source: Some("managed".into()),
                    message: "Update failed; the previous working binary remains installed.".into(),
                },
                Some(choice),
            );
        }
    }

    let message = format!("Could not install llama-server: {last_error}");
    warnings.push(message.clone());
    (failed_llama(message), Some(choice))
}

fn failed_llama(message: String) -> ProvisionedBinary {
    ProvisionedBinary {
        name: "llama-server".into(),
        status: ComponentStatus::Failed,
        path: None,
        version: None,
        source: None,
        message,
    }
}

async fn provision_janas(
    app: &AppHandle,
    supervisor: Arc<EngineSupervisor>,
    profile: &PlatformProfile,
    force: bool,
    warnings: &mut Vec<String>,
) -> (Vec<ProvisionedBinary>, Option<ReleaseChoice>) {
    let bin = match managed_bin_dir(app) {
        Ok(bin) => bin,
        Err(err) => {
            let message = err.to_string();
            return (
                vec![
                    failed_janas("janas-server", &message),
                    failed_janas("janas-chat", &message),
                ],
                None,
            );
        }
    };
    let server_path = bin.join("janas-server");
    let chat_path = bin.join("janas-chat");
    let server_probe = if server_path.is_file() {
        run_cli_probe(&server_path, Some(&bin))
            .await
            .ok()
            .filter(probe_is_usable)
    } else {
        None
    };
    let needs_server = force || !server_path.is_file() || server_probe.is_none();
    let needs_chat = force || !chat_path.is_file();

    if !needs_server && !needs_chat {
        return (
            vec![
                present_janas("janas-server", &server_path, server_probe.and_then(|probe| probe.version_line)),
                present_janas("janas-chat", &chat_path, None),
            ],
            None,
        );
    }

    let (release, asset) = match resolve_janas_release(profile).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            let message = format!(
                "No pre-built Janas release archive exists for {} {}. The bundled tools remain available.",
                profile.os, profile.arch
            );
            return (
                vec![
                    skipped_janas("janas-server", &server_path, &message),
                    skipped_janas("janas-chat", &chat_path, &message),
                ],
                None,
            );
        }
        Err(err) => {
            let message = format!("Could not resolve the latest Janas release: {err}");
            warnings.push(message.clone());
            return (
                vec![
                    failed_janas("janas-server", &message),
                    failed_janas("janas-chat", &message),
                ],
                None,
            );
        }
    };

    if let Err(err) = supervisor.ensure_inactive(EngineKind::Janas).await {
        let message = err.to_string();
        return (
            vec![
                failed_janas("janas-server", &message),
                failed_janas("janas-chat", &message),
            ],
            None,
        );
    }

    let url = match binary::validate_url(&asset.browser_download_url) {
        Ok(url) => url,
        Err(err) => {
            let message = format!("Invalid Janas download URL: {err}");
            warnings.push(message.clone());
            return (
                vec![
                    failed_janas("janas-server", &message),
                    failed_janas("janas-chat", &message),
                ],
                None,
            );
        }
    };
    let staging_root = bin.join(".staging");
    if let Err(err) = tokio::fs::create_dir_all(&staging_root).await {
        let message = format!("Could not create the Janas staging folder: {err}");
        warnings.push(message.clone());
        return (
            vec![
                failed_janas("janas-server", &message),
                failed_janas("janas-chat", &message),
            ],
            None,
        );
    }
    let work = staging_root.join(Uuid::new_v4().to_string());
    if let Err(err) = tokio::fs::create_dir_all(&work).await {
        let message = format!("Could not create the Janas download workspace: {err}");
        warnings.push(message.clone());
        return (
            vec![
                failed_janas("janas-server", &message),
                failed_janas("janas-chat", &message),
            ],
            None,
        );
    }
    let archive = work.join("janas-release.tar.gz");
    if let Err(err) = binary::download_file(app, EngineKind::Janas, url, &archive).await {
        let message = format!("Could not download the Janas release: {err}");
        warnings.push(message.clone());
        let _ = tokio::fs::remove_dir_all(&work).await;
        return (
            vec![
                failed_janas("janas-server", &message),
                failed_janas("janas-chat", &message),
            ],
            None,
        );
    }

    let payload_dir = work.join("payload");
    if let Err(err) = tokio::fs::create_dir_all(&payload_dir).await {
        let message = format!("Could not create the Janas extraction folder: {err}");
        warnings.push(message.clone());
        let _ = tokio::fs::remove_dir_all(&work).await;
        return (
            vec![
                failed_janas("janas-server", &message),
                failed_janas("janas-chat", &message),
            ],
            None,
        );
    }

    let primary = if needs_server { "janas-server" } else { "janas-chat" };
    let archive_for_extract = archive.clone();
    let payload_for_extract = payload_dir.clone();
    let primary_owned = primary.to_string();
    let payload = match tokio::task::spawn_blocking(move || {
        binary::extract_archive(&archive_for_extract, &payload_for_extract, &primary_owned)
    })
    .await
    {
        Ok(Ok(payload)) => payload,
        Ok(Err(err)) => {
            let message = format!("Could not extract the Janas release: {err}");
            warnings.push(message.clone());
            let _ = tokio::fs::remove_dir_all(&work).await;
            return (
                vec![
                    failed_janas("janas-server", &message),
                    failed_janas("janas-chat", &message),
                ],
                None,
            );
        }
        Err(err) => {
            let message = format!("The Janas extraction task failed: {err}");
            warnings.push(message.clone());
            let _ = tokio::fs::remove_dir_all(&work).await;
            return (
                vec![
                    failed_janas("janas-server", &message),
                    failed_janas("janas-chat", &message),
                ],
                None,
            );
        }
    };

    let server_source = if needs_server {
        Some(payload.binary.clone())
    } else {
        None
    };
    let chat_source = if needs_chat {
        binary::find_binary(&payload_dir, "janas-chat")
    } else {
        None
    };
    let companions = payload.companions.clone();
    let choice = ReleaseChoice {
        repo: JANAS_REPO.into(),
        requested_tag: release.tag_name.clone(),
        tag: release.tag_name.clone(),
        asset: asset.name.clone(),
        download_url: asset.browser_download_url.clone(),
        fallback: false,
    };

    let mut binaries = Vec::new();
    if let Some(source) = server_source {
        match install_janas_file(app, &bin, "janas-server", &source, &companions, true).await {
            Ok(version) => binaries.push(ProvisionedBinary {
                name: "janas-server".into(),
                status: ComponentStatus::Installed,
                path: Some(server_path.to_string_lossy().to_string()),
                version,
                source: Some("github".into()),
                message: format!("Installed from {} ({}).", choice.tag, choice.asset),
            }),
            Err(err) => {
                let message = format!("Could not install janas-server: {err}");
                warnings.push(message.clone());
                binaries.push(failed_janas("janas-server", &message));
            }
        }
    } else {
        binaries.push(present_janas(
            "janas-server",
            &server_path,
            server_probe.and_then(|probe| probe.version_line),
        ));
    }

    if needs_chat {
        match chat_source {
            Some(source) => match install_janas_file(
                app,
                &bin,
                "janas-chat",
                &source,
                &companions,
                false,
            )
            .await
            {
                Ok(_) => binaries.push(ProvisionedBinary {
                    name: "janas-chat".into(),
                    status: ComponentStatus::Installed,
                    path: Some(chat_path.to_string_lossy().to_string()),
                    version: None,
                    source: Some("github".into()),
                    message: format!("Installed from {} ({}).", choice.tag, choice.asset),
                }),
                Err(err) => {
                    let message = format!("Could not install janas-chat: {err}");
                    warnings.push(message.clone());
                    binaries.push(failed_janas("janas-chat", &message));
                }
            },
            None => {
                let message = "The Janas release archive did not contain janas-chat.".to_string();
                warnings.push(message.clone());
                binaries.push(failed_janas("janas-chat", &message));
            }
        }
    } else {
        binaries.push(present_janas("janas-chat", &chat_path, None));
    }

    let _ = tokio::fs::remove_dir_all(&work).await;
    (binaries, Some(choice))
}

fn present_janas(name: &str, path: &Path, version: Option<String>) -> ProvisionedBinary {
    ProvisionedBinary {
        name: name.into(),
        status: ComponentStatus::Present,
        path: Some(path.to_string_lossy().to_string()),
        version,
        source: Some("managed".into()),
        message: "Already installed in the managed bin folder.".into(),
    }
}

fn skipped_janas(name: &str, path: &Path, message: &str) -> ProvisionedBinary {
    ProvisionedBinary {
        name: name.into(),
        status: ComponentStatus::Skipped,
        path: path.is_file().then(|| path.to_string_lossy().to_string()),
        version: None,
        source: path.is_file().then(|| "managed".to_string()),
        message: message.into(),
    }
}

fn failed_janas(name: &str, message: &str) -> ProvisionedBinary {
    ProvisionedBinary {
        name: name.into(),
        status: ComponentStatus::Failed,
        path: None,
        version: None,
        source: None,
        message: message.into(),
    }
}

async fn install_janas_file(
    app: &AppHandle,
    bin: &Path,
    name: &str,
    source: &Path,
    companions: &[PathBuf],
    verify: bool,
) -> AppResult<Option<String>> {
    let bin_owned = bin.to_path_buf();
    let source_owned = source.to_path_buf();
    let name_owned = name.to_string();
    let companions_owned = companions.to_vec();
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_janas_install(&bin_owned, &name_owned, &source_owned, &companions_owned)
    })
    .await
    .map_err(|err| AppError::message(format!("The {name} install task failed: {err}")))??;

    if !verify {
        return Ok(None);
    }

    let outcome = run_cli_probe(&prepared.target, Some(bin)).await;
    let version = match outcome {
        Ok(output) if output.version_line.is_some() || !output.help_text.trim().is_empty() => {
            output.version_line
        }
        Ok(_) => {
            let _ = restore_janas_install(&prepared);
            return Err(AppError::message(
                "janas-server started but produced no version or help output. It may be the wrong architecture or missing a runtime library.",
            ));
        }
        Err(err) => {
            let _ = restore_janas_install(&prepared);
            return Err(AppError::message(format!(
                "janas-server failed its startup probe: {err}"
            )));
        }
    };

    let _ = app.emit(
        "engine://binary-progress",
        serde_json::json!({
            "engine": "janas",
            "phase": "complete",
            "downloaded": 0,
            "total": null,
            "percent": 100.0,
            "message": format!("{name} installed and verified in the managed bin folder."),
        }),
    );
    Ok(version)
}

#[derive(Debug, Clone)]
struct PreparedJanasInstall {
    target: PathBuf,
    backup: Option<PathBuf>,
}

fn prepare_janas_install(
    bin: &Path,
    name: &str,
    source: &Path,
    companions: &[PathBuf],
) -> AppResult<PreparedJanasInstall> {
    fs::create_dir_all(bin)
        .map_err(|err| AppError::message(format!("Could not create {}: {err}", bin.display())))?;
    let target = bin.join(name);
    let backup = target.with_file_name(format!("{name}.bak"));
    let had_target = target.is_file();
    let temp = bin.join(format!(".{name}.{}.new", Uuid::new_v4()));

    let outcome = (|| -> AppResult<()> {
        if had_target {
            if backup.exists() {
                fs::remove_file(&backup).map_err(|err| {
                    AppError::message(format!("Could not remove old {} backup: {err}", backup.display()))
                })?;
            }
            fs::rename(&target, &backup).map_err(|err| {
                AppError::message(format!("Could not move {} aside: {err}", target.display()))
            })?;
        }

        for companion in companions {
            let Some(file_name) = companion.file_name() else {
                continue;
            };
            let destination = bin.join(file_name);
            fs::copy(companion, &destination).map_err(|err| {
                AppError::message(format!(
                    "Could not install {}: {err}",
                    destination.display()
                ))
            })?;
            binary::set_executable(&destination)?;
        }

        fs::copy(source, &temp).map_err(|err| {
            AppError::message(format!("Could not stage {}: {err}", target.display()))
        })?;
        binary::set_executable(&temp)?;
        fs::rename(&temp, &target).map_err(|err| {
            AppError::message(format!("Could not atomically install {}: {err}", target.display()))
        })?;
        Ok(())
    })();

    match outcome {
        Ok(()) => Ok(PreparedJanasInstall {
            target,
            backup: had_target.then_some(backup),
        }),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            let _ = restore_janas_target(&target, &backup, had_target);
            Err(err)
        }
    }
}

fn restore_janas_install(prepared: &PreparedJanasInstall) -> AppResult<()> {
    let backup = prepared
        .backup
        .clone()
        .unwrap_or_else(|| prepared.target.with_file_name(format!(
            "{}.bak",
            prepared
                .target
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "janas".into())
        )));
    restore_janas_target(
        &prepared.target,
        &backup,
        prepared.backup.is_some(),
    )
}

fn restore_janas_target(target: &Path, backup: &Path, had_target: bool) -> AppResult<()> {
    if target.is_file() {
        fs::remove_file(target).map_err(|err| {
            AppError::message(format!("Could not remove failed {}: {err}", target.display()))
        })?;
    }
    if had_target && backup.is_file() {
        fs::rename(backup, target).map_err(|err| {
            AppError::message(format!("Could not restore {}: {err}", target.display()))
        })?;
        binary::set_executable(target)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Hardware discovery
// ---------------------------------------------------------------------------

/// Detect OS, CPU capabilities and the GPU backends that have both a device and
/// a usable user-space stack. Detection is intentionally best-effort: a missing
/// tool means "not detected", not a fatal setup error.
pub fn detect_platform() -> PlatformProfile {
    let os = normalize_os(env::consts::OS);
    let arch = normalize_arch(env::consts::ARCH);
    let apple_silicon = os == "macos" && arch == "aarch64";
    let mut gpus = Vec::new();

    if apple_silicon {
        gpus.push(GpuInfo {
            vendor: "apple".into(),
            backend: "metal".into(),
            name: "Apple Silicon (Metal)".into(),
            cuda_version: None,
        });
    } else if os != "macos" {
        if let Some(gpu) = detect_nvidia() {
            gpus.push(gpu);
        }
        if let Some(gpu) = detect_rocm() {
            gpus.push(gpu);
        }
        if let Some(gpu) = detect_vulkan() {
            gpus.push(gpu);
        }
    }

    let preferred_backend = preferred_backend(&gpus, apple_silicon);
    PlatformProfile {
        os,
        arch,
        cpu_avx2: cpu_avx2(),
        apple_silicon,
        gpus,
        preferred_backend,
    }
}

fn probe_is_usable(probe: &crate::engines::ProbeOutput) -> bool {
    probe.version_line.is_some() || !probe.help_text.trim().is_empty()
}

fn normalize_os(value: &str) -> String {
    match value {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    }
    .into()
}

fn normalize_arch(value: &str) -> String {
    match value {
        "aarch64" | "arm64" => "aarch64",
        "x86_64" | "amd64" => "x86_64",
        other => other,
    }
    .into()
}

fn preferred_backend(gpus: &[GpuInfo], apple_silicon: bool) -> String {
    if apple_silicon {
        return "metal".into();
    }
    for backend in ["cuda", "rocm", "vulkan"] {
        if gpus.iter().any(|gpu| gpu.backend == backend) {
            return backend.into();
        }
    }
    "cpu".into()
}

fn cpu_avx2() -> bool {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return true;
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
            return cpuinfo
                .lines()
                .filter(|line| line.to_ascii_lowercase().starts_with("flags"))
                .any(|line| line.split_whitespace().any(|flag| flag.eq_ignore_ascii_case("avx2")));
        }
    }
    false
}

fn detect_nvidia() -> Option<GpuInfo> {
    let name = run_short(
        "nvidia-smi",
        &["--query-gpu=name", "--format=csv,noheader"],
        Duration::from_secs(3),
    )
    .and_then(|text| {
        text.lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string)
    });
    let full = run_short("nvidia-smi", &[], Duration::from_secs(3));
    let cuda_version = full.as_deref().and_then(parse_cuda_version);

    #[cfg(target_os = "linux")]
    let device_present = Path::new("/dev/nvidia0").exists();
    #[cfg(not(target_os = "linux"))]
    let device_present = false;

    if name.is_none() && cuda_version.is_none() && !device_present {
        return None;
    }
    Some(GpuInfo {
        vendor: "nvidia".into(),
        backend: "cuda".into(),
        name: name.unwrap_or_else(|| "NVIDIA CUDA GPU".into()),
        cuda_version,
    })
}

fn parse_cuda_version(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        let marker = "cuda version:";
        let start = lower.find(marker)? + marker.len();
        let version = line[start..]
            .trim()
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches(',');
        parse_version(version).map(|_| version.to_string())
    })
}

fn detect_rocm() -> Option<GpuInfo> {
    #[cfg(target_os = "linux")]
    let installed = Path::new("/opt/rocm").exists() || Path::new("/dev/kfd").exists();
    #[cfg(target_os = "windows")]
    let installed = env::var_os("ROCM_PATH").is_some()
        || Path::new("C:\\Program Files\\AMD\\ROCm").exists();
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let installed = false;

    if !installed {
        return None;
    }
    let name = run_short("rocminfo", &[], Duration::from_secs(3))
        .and_then(|text| {
            text.lines().find_map(|line| {
                let trimmed = line.trim();
                let value = trimmed.strip_prefix("Name:")?.trim();
                if value.is_empty() || value.to_ascii_lowercase().contains("cpu") {
                    None
                } else {
                    Some(value.to_string())
                }
            })
        })
        .unwrap_or_else(|| "AMD ROCm GPU".into());
    Some(GpuInfo {
        vendor: "amd".into(),
        backend: "rocm".into(),
        name,
        cuda_version: None,
    })
}

fn detect_vulkan() -> Option<GpuInfo> {
    let output = run_short("vulkaninfo", &["--summary"], Duration::from_secs(4));
    let name = output.as_deref().and_then(|text| {
        text.lines().find_map(|line| {
            let trimmed = line.trim();
            let value = trimmed
                .strip_prefix("deviceName")
                .or_else(|| trimmed.strip_prefix("GPU0:"))
                .map(str::trim)
                .unwrap_or("");
            if value.is_empty() {
                None
            } else {
                Some(value.trim_start_matches('=').trim().to_string())
            }
        })
    });

    let has_loader = {
        #[cfg(target_os = "linux")]
        {
            Path::new("/usr/lib/x86_64-linux-gnu/libvulkan.so.1").exists()
                || Path::new("/usr/lib/aarch64-linux-gnu/libvulkan.so.1").exists()
                || Path::new("/usr/lib/libvulkan.so.1").exists()
                || env::var_os("VK_ICD_FILENAMES").is_some()
                || Path::new("/usr/share/vulkan/icd.d").exists()
        }
        #[cfg(target_os = "windows")]
        {
            env::var_os("WINDIR")
                .map(PathBuf::from)
                .map(|dir| dir.join("System32").join("vulkan-1.dll").is_file())
                .unwrap_or(false)
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            false
        }
    };

    if output.is_none() && !has_loader {
        return None;
    }
    Some(GpuInfo {
        vendor: "unknown".into(),
        backend: "vulkan".into(),
        name: name.unwrap_or_else(|| "Vulkan-compatible GPU".into()),
        cuda_version: None,
    })
}

fn run_short(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child.wait_with_output().ok()?;
                let mut text = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.trim().is_empty() {
                    if !text.trim().is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&stderr);
                }
                return Some(text);
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                return None;
            }
            Err(_) => return None,
        }
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// Resolve the Janas engine cache without writing through a symlink. The
/// upstream engine uses `~/.cache/janas` on Linux; the platform equivalents are
/// used on macOS and Windows.
pub fn ensure_janas_cache() -> AppResult<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("AppData").join("Local")))
            .ok_or_else(|| AppError::message("Could not resolve LOCALAPPDATA for the Janas cache."))?
            .join("janas")
            .join("cache")
    } else if cfg!(target_os = "macos") {
        env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| AppError::message("Could not resolve HOME for the Janas cache."))?
            .join("Library")
            .join("Caches")
            .join("janas")
    } else {
        env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .ok_or_else(|| AppError::message("Could not resolve the user cache directory for Janas."))?
            .join("janas")
    };

    if let Ok(metadata) = fs::symlink_metadata(&base) {
        if metadata.file_type().is_symlink() {
            return Err(AppError::message(format!(
                "Refusing to use the Janas cache through a symlink: {}",
                base.display()
            )));
        }
        if !metadata.is_dir() {
            return Err(AppError::message(format!(
                "The Janas cache path exists but is not a directory: {}",
                base.display()
            )));
        }
    }
    fs::create_dir_all(&base).map_err(|err| {
        AppError::message(format!(
            "Could not create the Janas cache directory {}: {err}",
            base.display()
        ))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).map_err(|err| {
            AppError::message(format!(
                "Could not secure the Janas cache directory {}: {err}",
                base.display()
            ))
        })?;
    }
    Ok(base)
}

// ---------------------------------------------------------------------------
// GitHub release resolution
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Clone)]
struct AssetCandidate {
    backend: String,
    asset: GhAsset,
}

#[derive(Debug, Clone)]
struct LlamaResolution {
    release: GhRelease,
    candidates: Vec<AssetCandidate>,
    stable_tag: String,
    fallback: bool,
}

impl LlamaResolution {
    fn choice(&self) -> ReleaseChoice {
        let asset = &self.candidates[0].asset;
        ReleaseChoice {
            repo: LLAMA_REPO.into(),
            requested_tag: self.stable_tag.clone(),
            tag: self.release.tag_name.clone(),
            asset: asset.name.clone(),
            download_url: asset.browser_download_url.clone(),
            fallback: self.fallback,
        }
    }
}

async fn github_json<T: DeserializeOwned>(path: &str) -> AppResult<T> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("Janas-Studio/0.1 (+https://github.com/prabanta-dev/janas)")
        .build()
        .map_err(|err| AppError::message(format!("Could not build the GitHub client: {err}")))?;
    let response = client
        .get(github_url(path))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|err| {
            AppError::message(format!(
                "Could not reach the GitHub release API: {err}. Check the network connection and retry."
            ))
        })?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let hint = if status.as_u16() == 403 {
            " The unauthenticated GitHub API rate limit may be exhausted."
        } else {
            ""
        };
        return Err(AppError::message(format!(
            "GitHub returned HTTP {status}.{hint} {}",
            truncate_line(&body, 300)
        )));
    }
    serde_json::from_str(&body).map_err(|err| {
        AppError::message(format!("GitHub returned an unreadable release response: {err}"))
    })
}

fn github_url(path: &str) -> String {
    format!("{GITHUB_API}/{path}")
}

async fn fetch_latest_release(repo: &str) -> AppResult<GhRelease> {
    github_json(&format!("repos/{repo}/releases/latest")).await
}

async fn fetch_release_list(repo: &str) -> AppResult<Vec<GhRelease>> {
    github_json(&format!(
        "repos/{repo}/releases?per_page={RELEASE_LIST_LIMIT}"
    ))
    .await
}

async fn resolve_llama_release(profile: &PlatformProfile) -> AppResult<LlamaResolution> {
    let stable = fetch_latest_release(LLAMA_REPO).await?;
    let stable_tag = stable.tag_name.clone();
    if let Some(candidates) = select_llama_candidates(&stable, profile) {
        return Ok(LlamaResolution {
            release: stable,
            candidates,
            stable_tag,
            fallback: false,
        });
    }

    // The current llama.cpp stable release can contain only a nightly-tag.txt
    // pointer while the actual platform builds live under prerelease tags.
    // Prefer a stable asset whenever one exists, then use the newest release
    // that actually contains a matching build.
    let releases = fetch_release_list(LLAMA_REPO).await?;
    for release in releases {
        if release.draft || release.tag_name == stable_tag {
            continue;
        }
        if let Some(candidates) = select_llama_candidates(&release, profile) {
            return Ok(LlamaResolution {
                release,
                candidates,
                stable_tag,
                fallback: true,
            });
        }
    }

    Err(AppError::message(format!(
        "The latest stable tag is {stable_tag}, but it has no build for {} {}. No newer release has a matching asset either.",
        profile.os, profile.arch
    )))
}

async fn resolve_janas_release(
    profile: &PlatformProfile,
) -> AppResult<Option<(GhRelease, GhAsset)>> {
    let release = fetch_latest_release(JANAS_REPO).await?;
    let asset = release
        .assets
        .iter()
        .find(|asset| is_janas_asset(&asset.name, profile))
        .cloned();
    Ok(asset.map(|asset| (release, asset)))
}

fn select_llama_candidates(
    release: &GhRelease,
    profile: &PlatformProfile,
) -> Option<Vec<AssetCandidate>> {
    let backends = backend_preferences(profile);
    let mut seen = std::collections::HashSet::new();
    let mut candidates = Vec::new();
    for backend in backends {
        if let Some(asset) = best_llama_asset(&release.assets, profile, &backend) {
            if seen.insert(asset.name.clone()) {
                candidates.push(AssetCandidate {
                    backend: backend.clone(),
                    asset,
                });
            }
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn backend_preferences(profile: &PlatformProfile) -> Vec<String> {
    if profile.os == "macos" {
        return vec![if profile.apple_silicon { "metal".into() } else { "cpu".into() }];
    }
    let mut backends = Vec::new();
    if profile.gpus.iter().any(|gpu| gpu.backend == "cuda") {
        backends.push("cuda".into());
    }
    if profile.gpus.iter().any(|gpu| gpu.backend == "rocm") {
        backends.push("rocm".into());
    }
    if profile.gpus.iter().any(|gpu| gpu.backend == "vulkan") {
        backends.push("vulkan".into());
    }
    backends.push("cpu".into());
    backends
}

fn best_llama_asset(
    assets: &[GhAsset],
    profile: &PlatformProfile,
    backend: &str,
) -> Option<GhAsset> {
    let detected_cuda = profile
        .gpus
        .iter()
        .find(|gpu| gpu.backend == "cuda")
        .and_then(|gpu| gpu.cuda_version.as_deref())
        .and_then(parse_version);

    assets
        .iter()
        .filter(|asset| is_llama_main_asset(&asset.name, profile))
        .filter(|asset| match backend {
            "metal" | "cpu" => asset_backend(&asset.name) == "none" || asset_backend(&asset.name) == backend,
            _ => asset_backend(&asset.name) == backend,
        })
        .filter(|asset| backend != "cuda" || cuda_compatible(&asset.name, detected_cuda))
        .max_by_key(|asset| {
            if backend == "cuda" {
                let version = cuda_asset_version(&asset.name);
                match (detected_cuda, version) {
                    (Some(_driver), Some((major, minor))) => {
                        (0i32, major as i32, minor as i32, asset.size)
                    }
                    (None, Some((major, minor))) => {
                        (1i32, -(major as i32), -(minor as i32), asset.size)
                    }
                    _ => (2i32, 0, 0, asset.size),
                }
            } else {
                (0, 0, 0, asset.size)
            }
        })
        .cloned()
}

fn is_llama_main_asset(name: &str, profile: &PlatformProfile) -> bool {
    let lower = name.to_ascii_lowercase();
    if !lower.starts_with("llama-")
        || !lower.contains("-bin-")
        || lower.contains("cudart")
        || lower.contains("xcframework")
        || lower.contains("android")
        || lower.contains("snapdragon")
        || lower.contains("s390x")
        || lower.contains("-ui.")
    {
        return false;
    }
    if !(lower.ends_with(".zip") || lower.ends_with(".tar.gz")) {
        return false;
    }
    let platform_ok = match profile.os.as_str() {
        "linux" => lower.contains("ubuntu") || lower.contains("linux"),
        "windows" => lower.contains("win-") || lower.contains("-win-"),
        "macos" => lower.contains("macos"),
        _ => false,
    };
    if !platform_ok {
        return false;
    }
    let arch_ok = match profile.arch.as_str() {
        "x86_64" => lower.contains("-x64.") || lower.contains("-x64-") || lower.contains("x86_64"),
        "aarch64" => lower.contains("-arm64.") || lower.contains("-arm64-"),
        _ => false,
    };
    arch_ok
}

fn asset_backend(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.contains("cuda") {
        "cuda"
    } else if lower.contains("rocm") {
        "rocm"
    } else if lower.contains("vulkan") {
        "vulkan"
    } else if lower.contains("sycl") {
        "sycl"
    } else if lower.contains("openvino") {
        "openvino"
    } else if lower.contains("opencl") {
        "opencl"
    } else if lower.contains("-cpu-") || lower.ends_with("-cpu.zip") {
        "cpu"
    } else {
        "none"
    }
}

fn cuda_asset_version(name: &str) -> Option<(u32, u32)> {
    let lower = name.to_ascii_lowercase();
    let start = lower.find("cuda-")? + "cuda-".len();
    let version = lower[start..].split(|ch: char| ch == '-' || ch == '.').collect::<Vec<_>>();
    let major = version.first()?.parse::<u32>().ok()?;
    let minor = version.get(1)?.parse::<u32>().ok()?;
    Some((major, minor))
}

fn cuda_compatible(name: &str, detected: Option<(u32, u32)>) -> bool {
    match (cuda_asset_version(name), detected) {
        (Some(asset), Some(driver)) => asset <= driver,
        _ => true,
    }
}

fn parse_version(value: &str) -> Option<(u32, u32)> {
    let mut parts = value.trim().split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    Some((major, minor))
}

fn is_janas_asset(name: &str, profile: &PlatformProfile) -> bool {
    let lower = name.to_ascii_lowercase();
    if !lower.starts_with("janas-")
        || lower.ends_with(".sha256")
        || lower.contains("diag")
        || !(lower.ends_with(".zip") || lower.ends_with(".tar.gz"))
    {
        return false;
    }
    let platform_ok = match profile.os.as_str() {
        "linux" => lower.contains("linux"),
        "windows" => lower.contains("windows"),
        _ => false,
    };
    platform_ok
        && match profile.arch.as_str() {
            "x86_64" => lower.contains("x86_64"),
            "aarch64" => lower.contains("arm64") || lower.contains("aarch64"),
            _ => false,
        }
}

// ---------------------------------------------------------------------------
// IPC
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_engine_setup_status(setup: State<'_, Arc<Provisioner>>) -> SetupReport {
    setup.report()
}

#[tauri::command]
pub async fn rerun_engine_setup(
    app: AppHandle,
    setup: State<'_, Arc<Provisioner>>,
    manager: State<'_, Arc<BinaryManager>>,
    supervisor: State<'_, Arc<EngineSupervisor>>,
) -> AppResult<SetupReport> {
    let setup = setup.inner().clone();
    let manager = manager.inner().clone();
    let supervisor = supervisor.inner().clone();
    Ok(setup.run(&app, manager, supervisor, true).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, names: &[&str]) -> GhRelease {
        GhRelease {
            tag_name: tag.into(),
            draft: false,
            assets: names
                .iter()
                .map(|name| GhAsset {
                    name: (*name).into(),
                    browser_download_url: format!("https://example.test/{name}"),
                    size: 1,
                })
                .collect(),
        }
    }

    fn profile(os: &str, arch: &str, backend: Option<&str>) -> PlatformProfile {
        PlatformProfile {
            os: os.into(),
            arch: arch.into(),
            cpu_avx2: true,
            apple_silicon: os == "macos" && arch == "aarch64",
            gpus: backend
                .map(|backend| {
                    vec![GpuInfo {
                        vendor: "test".into(),
                        backend: backend.into(),
                        name: "test GPU".into(),
                        cuda_version: (backend == "cuda").then(|| "12.8".into()),
                    }]
                })
                .unwrap_or_default(),
            preferred_backend: backend.unwrap_or("cpu").into(),
        }
    }

    #[test]
    fn selects_cuda_for_compatible_linux_and_keeps_cpu_fallback() {
        let release = release(
            "b1",
            &[
                "cudart-llama-b1-bin-ubuntu-cuda-12.8-x64.tar.gz",
                "llama-b1-bin-ubuntu-cuda-12.8-x64.tar.gz",
                "llama-b1-bin-ubuntu-cuda-13.5-x64.tar.gz",
                "llama-b1-bin-ubuntu-vulkan-x64.tar.gz",
                "llama-b1-bin-ubuntu-x64.tar.gz",
                "llama-b1-bin-macos-arm64.tar.gz",
                "llama-b1-ui.tar.gz",
            ],
        );
        let candidates = select_llama_candidates(&release, &profile("linux", "x86_64", Some("cuda")))
            .expect("candidate list");
        assert_eq!(candidates[0].backend, "cuda");
        assert_eq!(candidates[0].asset.name, "llama-b1-bin-ubuntu-cuda-12.8-x64.tar.gz");
        assert_eq!(candidates.last().unwrap().asset.name, "llama-b1-bin-ubuntu-x64.tar.gz");
        assert!(!candidates.iter().any(|candidate| candidate.asset.name.contains("cudart")));
    }

    #[test]
    fn selects_arm64_windows_vulkan_and_cpu() {
        let release = release(
            "b2",
            &[
                "llama-b2-bin-win-vulkan-arm64.zip",
                "llama-b2-bin-win-cpu-arm64.zip",
                "llama-b2-bin-win-vulkan-x64.zip",
                "llama-b2-bin-win-cpu-x64.zip",
            ],
        );
        let candidates = select_llama_candidates(&release, &profile("windows", "aarch64", Some("vulkan")))
            .expect("candidate list");
        assert_eq!(candidates[0].asset.name, "llama-b2-bin-win-vulkan-arm64.zip");
        assert_eq!(candidates[1].asset.name, "llama-b2-bin-win-cpu-arm64.zip");
    }

    #[test]
    fn selects_macos_metal_archive_without_a_backend_token() {
        let release = release(
            "b3",
            &[
                "llama-b3-bin-macos-arm64.tar.gz",
                "llama-b3-bin-macos-x64.tar.gz",
            ],
        );
        let candidates = select_llama_candidates(&release, &profile("macos", "aarch64", None))
            .expect("candidate list");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].backend, "metal");
        assert_eq!(candidates[0].asset.name, "llama-b3-bin-macos-arm64.tar.gz");
    }

    #[test]
    fn no_asset_selection_when_cuda_driver_is_too_old() {
        let release = release(
            "b4",
            &[
                "llama-b4-bin-ubuntu-cuda-13.5-x64.tar.gz",
                "llama-b4-bin-ubuntu-x64.tar.gz",
            ],
        );
        let mut machine = profile("linux", "x86_64", Some("cuda"));
        machine.gpus[0].cuda_version = Some("12.8".into());
        let candidates = select_llama_candidates(&release, &machine).expect("candidate list");
        assert_eq!(candidates[0].backend, "cpu");
        assert_eq!(candidates[0].asset.name, "llama-b4-bin-ubuntu-x64.tar.gz");
    }

    #[test]
    fn janas_asset_is_platform_specific_and_excludes_diagnostics() {
        let release = release(
            "v1",
            &[
                "janas-diag-linux-x86_64.tar.gz",
                "janas-linux-x86_64.tar.gz",
                "janas-windows-x86_64.zip",
            ],
        );
        assert_eq!(
            release
                .assets
                .iter()
                .find(|asset| is_janas_asset(&asset.name, &profile("linux", "x86_64", None)))
                .unwrap()
                .name,
            "janas-linux-x86_64.tar.gz"
        );
        assert!(release
            .assets
            .iter()
            .find(|asset| is_janas_asset(&asset.name, &profile("macos", "aarch64", None)))
            .is_none());
    }

    #[test]
    fn parses_cuda_version_from_nvidia_smi_output() {
        assert_eq!(
            parse_cuda_version("| NVIDIA-SMI 550.54.14 Driver Version: 550.54.14 CUDA Version: 12.4 |"),
            Some("12.4".into())
        );
        assert_eq!(parse_cuda_version("no gpu here"), None);
    }

    #[test]
    fn github_url_contains_the_api_path_exactly_once() {
        assert_eq!(
            github_url("repos/ggml-org/llama.cpp/releases/latest"),
            "https://api.github.com/repos/ggml-org/llama.cpp/releases/latest"
        );
        assert!(!github_url("repos/prabanta-dev/janas/releases/latest").contains("/repos/repos/"));
    }

    #[cfg(unix)]
    #[test]
    fn janas_install_backs_up_and_restores_the_previous_binary() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("janas-provision-test-{}", Uuid::new_v4()));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("create managed bin");
        let target = bin.join("janas-chat");
        fs::write(&target, "#!/bin/sh\nprintf 'old\\n'\n").expect("write old binary");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).expect("chmod old");
        let source = root.join("incoming-janas-chat");
        fs::write(&source, "#!/bin/sh\nprintf 'new\\n'\n").expect("write new binary");

        let prepared = prepare_janas_install(&bin, "janas-chat", &source, &[])
            .expect("prepare Janas install");
        assert_eq!(fs::read_to_string(&target).unwrap(), "#!/bin/sh\nprintf 'new\\n'\n");
        assert!(prepared.backup.as_ref().expect("backup").exists());

        restore_janas_install(&prepared).expect("restore Janas binary");
        assert_eq!(fs::read_to_string(&target).unwrap(), "#!/bin/sh\nprintf 'old\\n'\n");
        assert_ne!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o111,
            0,
            "restored binary must stay executable"
        );

        let _ = fs::remove_dir_all(root);
    }
}
