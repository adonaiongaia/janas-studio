//! Managed binary downloads, safe extraction, atomic install and rollback.
//!
//! Everything lives under `app_local_data_dir()/bin`:
//!
//! ```text
//! bin/
//!   llama-server          current binary
//!   llama-server.bak      exactly one previous binary
//!   libllama.so...        shared libraries from the same release
//!   .rollback/llama/      libraries that belong with llama-server.bak
//!   .staging/<uuid>/      in-flight download and extraction
//! ```
//!
//! The AppImage itself is mounted read-only, so no code path here writes next
//! to the executable or into the resource directory.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::{io::AsyncWriteExt, sync::Mutex};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    util::truncate_line,
};

use super::{managed_bin_dir, managed_binary_path, run_cli_probe, EngineKind, EngineSupervisor};

const MAX_DOWNLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 20_000;

#[derive(Debug, Clone, Serialize)]
pub struct BinaryProgress {
    pub engine: EngineKind,
    pub phase: String,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub percent: Option<f64>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BinaryInstallResult {
    pub engine: EngineKind,
    pub installed_path: String,
    pub backup_path: Option<String>,
    pub version: Option<String>,
    pub message: String,
    pub rolled_back: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ManagedBinaryProbe {
    pub engine: EngineKind,
    pub installed: bool,
    pub path: String,
    pub version: Option<String>,
    pub build: Option<u64>,
    pub message: String,
}

pub struct BinaryManager {
    /// Serializes downloads, installs and rollbacks so two archives can never
    /// race over the same `.bak` file.
    gate: Mutex<()>,
}

impl BinaryManager {
    pub fn new() -> Self {
        Self {
            gate: Mutex::new(()),
        }
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn download_and_extract_binary(
    app: AppHandle,
    manager: State<'_, Arc<BinaryManager>>,
    supervisor: State<'_, Arc<EngineSupervisor>>,
    engine_type: String,
    download_url: String,
) -> AppResult<BinaryInstallResult> {
    let manager = manager.inner().clone();
    let supervisor = supervisor.inner().clone();
    download_and_install(app, manager, supervisor, engine_type, download_url).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rollback_binary(
    app: AppHandle,
    manager: State<'_, Arc<BinaryManager>>,
    supervisor: State<'_, Arc<EngineSupervisor>>,
    engine_type: String,
) -> AppResult<BinaryInstallResult> {
    let manager = manager.inner().clone();
    let supervisor = supervisor.inner().clone();
    rollback(app, manager, supervisor, engine_type).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn probe_managed_binary(
    app: AppHandle,
    engine_type: String,
) -> AppResult<ManagedBinaryProbe> {
    let kind = parse_engine_type(&engine_type)?;
    let path = managed_binary_path(&app, kind)?;
    if !path.is_file() {
        return Ok(ManagedBinaryProbe {
            engine: kind,
            installed: false,
            path: path.to_string_lossy().to_string(),
            version: None,
            build: None,
            message: format!(
                "{} is not installed in the managed bin folder yet.",
                kind.binary_name()
            ),
        });
    }
    let bin = managed_bin_dir(&app)?;
    let output = run_cli_probe(&path, Some(&bin)).await?;
    Ok(ManagedBinaryProbe {
        engine: kind,
        installed: true,
        path: path.to_string_lossy().to_string(),
        version: output.version_line,
        build: output.build,
        message: "Binary found and executable. Shared libraries are resolved from the managed bin folder.".into(),
    })
}

pub(crate) async fn install_engine_from_url(
    app: &AppHandle,
    manager: &Arc<BinaryManager>,
    supervisor: &Arc<EngineSupervisor>,
    kind: EngineKind,
    download_url: String,
) -> AppResult<BinaryInstallResult> {
    download_and_install(
        app.clone(),
        manager.clone(),
        supervisor.clone(),
        kind.storage_name().to_string(),
        download_url,
    )
    .await
}

async fn download_and_install(
    app: AppHandle,
    manager: Arc<BinaryManager>,
    supervisor: Arc<EngineSupervisor>,
    engine_type: String,
    download_url: String,
) -> AppResult<BinaryInstallResult> {
    let kind = parse_engine_type(&engine_type)?;
    let url = validate_url(&download_url)?;
    let _gate = manager.gate.lock().await;
    supervisor.ensure_inactive(kind).await?;

    let bin = managed_bin_dir(&app)?;
    let staging_root = bin.join(".staging");
    tokio::fs::create_dir_all(&staging_root)
        .await
        .map_err(|err| super::io_error("Could not create the staging folder", &staging_root, err))?;
    let work = staging_root.join(Uuid::new_v4().to_string());
    tokio::fs::create_dir_all(&work)
        .await
        .map_err(|err| super::io_error("Could not create a download workspace", &work, err))?;

    emit_progress(
        &app,
        kind,
        "preparing",
        0,
        None,
        format!("Preparing {} download…", kind.label()),
    );

    let result = install_inner(&app, &supervisor, kind, url, &bin, &work).await;
    if let Err(err) = &result {
        emit_progress(
            &app,
            kind,
            "error",
            0,
            None,
            truncate_line(&err.to_string(), 400),
        );
    }
    let _ = tokio::fs::remove_dir_all(&work).await;
    result
}

async fn install_inner(
    app: &AppHandle,
    supervisor: &Arc<EngineSupervisor>,
    kind: EngineKind,
    url: reqwest::Url,
    bin: &Path,
    work: &Path,
) -> AppResult<BinaryInstallResult> {
    let archive = work.join("download.bin");
    emit_progress(
        app,
        kind,
        "downloading",
        0,
        None,
        format!("Downloading {}…", kind.label()),
    );
    download_file(app, kind, url, &archive).await?;

    emit_progress(
        app,
        kind,
        "extracting",
        0,
        None,
        "Verifying the archive and extracting the binary…".into(),
    );
    let payload_dir = work.join("payload");
    tokio::fs::create_dir_all(&payload_dir)
        .await
        .map_err(|err| super::io_error("Could not create the extraction folder", &payload_dir, err))?;

    let binary_name = kind.binary_name();
    let archive_for_extract = archive.clone();
    let payload_for_extract = payload_dir.clone();
    let name_for_extract = binary_name.clone();
    let staged = tokio::task::spawn_blocking(move || {
        extract_archive(&archive_for_extract, &payload_for_extract, &name_for_extract)
    })
    .await
    .map_err(|err| AppError::message(format!("The extraction task failed: {err}")))??;

    // The user may have started the engine while the archive was downloading.
    supervisor.ensure_inactive(kind).await?;

    emit_progress(
        app,
        kind,
        "installing",
        0,
        None,
        "Backing up the current binary and installing the new one…".into(),
    );
    let bin_for_install = bin.to_path_buf();
    let staged_binary = staged.binary.clone();
    let companions = staged.companions.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_install(&bin_for_install, kind, &staged_binary, &companions)
    })
    .await
    .map_err(|err| AppError::message(format!("The install task failed: {err}")))??;

    emit_progress(
        app,
        kind,
        "verifying",
        0,
        None,
        "Checking that the new binary starts and can load its shared libraries…".into(),
    );
    let verification = verify_installed(&prepared.target, bin).await;
    match verification {
        Ok(version) => {
            emit_progress(
                app,
                kind,
                "complete",
                0,
                None,
                format!("{} installed successfully.", kind.label()),
            );
            let backup = prepared
                .backup
                .as_ref()
                .map(|path| path.to_string_lossy().to_string());
            Ok(BinaryInstallResult {
                engine: kind,
                installed_path: prepared.target.to_string_lossy().to_string(),
                backup_path: backup,
                version: version.clone(),
                message: match version {
                    Some(_version) => format!(
                        "{} installed and verified. Previous binary kept as {}.",
                        kind.label(),
                        prepared
                            .backup
                            .as_ref()
                            .map(|path| path.to_string_lossy().to_string())
                            .unwrap_or_else(|| "no backup (first install)".into())
                    ),
                    None => format!("{} installed and verified.", kind.label()),
                },
                rolled_back: false,
            })
        }
        Err(err) => {
            let bin_for_restore = bin.to_path_buf();
            let prepared_for_restore = prepared.clone();
            let restore = tokio::task::spawn_blocking(move || {
                restore_after_failed_verify(&bin_for_restore, &prepared_for_restore)
            })
            .await;
            let restored = match restore {
                Ok(Ok(())) => "The previous binary was restored.".to_string(),
                Ok(Err(restore_err)) => format!(
                    "The automatic restore also failed: {restore_err}. Reinstall the previous release manually."
                ),
                Err(join_err) => format!("The automatic restore task failed: {join_err}."),
            };
            Err(AppError::message(format!(
                "{} failed its startup check, so the update was rolled back. {restored} Original error: {err}",
                kind.label()
            )))
        }
    }
}

async fn rollback(
    app: AppHandle,
    manager: Arc<BinaryManager>,
    supervisor: Arc<EngineSupervisor>,
    engine_type: String,
) -> AppResult<BinaryInstallResult> {
    let kind = parse_engine_type(&engine_type)?;
    let _gate = manager.gate.lock().await;
    supervisor.ensure_inactive(kind).await?;
    let bin = managed_bin_dir(&app)?;
    emit_progress(
        &app,
        kind,
        "rolling_back",
        0,
        None,
        format!("Rolling {} back to its .bak binary…", kind.label()),
    );

    let prepared = {
        let bin = bin.clone();
        tokio::task::spawn_blocking(move || prepare_rollback(&bin, kind))
            .await
            .map_err(|err| AppError::message(format!("The rollback task failed: {err}")))?
    }?;

    let verification = verify_installed(&prepared.target, &bin).await;
    match verification {
        Ok(version) => {
            let bin_for_cleanup = bin.clone();
            let prepared_for_cleanup = prepared.clone();
            let _ = tokio::task::spawn_blocking(move || {
                finish_rollback(&bin_for_cleanup, &prepared_for_cleanup)
            })
            .await;
            emit_progress(
                &app,
                kind,
                "complete",
                0,
                None,
                format!("{} rolled back successfully.", kind.label()),
            );
            Ok(BinaryInstallResult {
                engine: kind,
                installed_path: prepared.target.to_string_lossy().to_string(),
                backup_path: None,
                version: version.clone(),
                message: match version {
                    Some(version) => format!("Rollback verified ({version})."),
                    None => "Rollback verified.".into(),
                },
                rolled_back: true,
            })
        }
        Err(err) => {
            let bin_for_undo = bin.clone();
            let prepared_for_undo = prepared.clone();
            let undo = tokio::task::spawn_blocking(move || {
                undo_rollback(&bin_for_undo, &prepared_for_undo)
            })
            .await;
            let note = match undo {
                Ok(Ok(())) => "The newer binary was restored.".to_string(),
                Ok(Err(undo_err)) => format!("Restoring the newer binary also failed: {undo_err}."),
                Err(join_err) => format!("The restore task failed: {join_err}."),
            };
            Err(AppError::message(format!(
                "The .bak binary failed its startup check. {note} Original error: {err}"
            )))
        }
    }
}

fn parse_engine_type(raw: &str) -> AppResult<EngineKind> {
    EngineKind::parse_public(raw).ok_or_else(|| {
        AppError::message(format!(
            "Unknown engine type \"{raw}\". Use \"llama\" or \"janas\"."
        ))
    })
}

pub(crate) fn validate_url(raw: &str) -> AppResult<reqwest::Url> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|err| {
        AppError::message(format!(
            "The download URL is not valid: {err}. Paste the full https:// asset URL."
        ))
    })?;
    let local = matches!(
        url.host_str(),
        Some("localhost") | Some("127.0.0.1") | Some("::1")
    );
    match url.scheme() {
        "https" => Ok(url),
        "http" if local => Ok(url),
        scheme => Err(AppError::message(format!(
            "Refusing a {scheme} download. Use an https:// URL (HTTP is allowed only for localhost test servers)."
        ))),
    }
}

pub(crate) async fn download_file(
    app: &AppHandle,
    kind: EngineKind,
    url: reqwest::Url,
    destination: &Path,
) -> AppResult<()> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(30 * 60))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent("Janas-Studio/0.1")
        .build()
        .map_err(|err| AppError::message(format!("Could not build the HTTP client: {err}")))?;

    let response = client.get(url).send().await.map_err(|err| {
        AppError::message(format!(
            "Could not start the download: {err}. Check the URL, your network, and any proxy settings."
        ))
    })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::message(format!(
            "The download server returned HTTP {status}: {}",
            truncate_line(&body, 300)
        )));
    }

    let total = response.content_length();
    if let Some(total) = total {
        if total > MAX_DOWNLOAD_BYTES {
            return Err(AppError::message(format!(
                "The archive is {total} bytes, which is larger than the {} byte safety limit.",
                MAX_DOWNLOAD_BYTES
            )));
        }
    }

    let mut file = tokio::fs::File::create(destination)
        .await
        .map_err(|err| super::io_error("Could not create the download file", destination, err))?;
    let mut stream = response.bytes_stream();
    let mut downloaded = 0u64;
    let mut last_emit = Instant::now();
    emit_progress(
        app,
        kind,
        "downloading",
        0,
        total,
        match total {
            Some(total) => format!("Downloading {} ({})…", kind.label(), format_bytes(total)),
            None => format!("Downloading {}…", kind.label()),
        },
    );

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| {
            AppError::message(format!("The download was interrupted after {downloaded} bytes: {err}"))
        })?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > MAX_DOWNLOAD_BYTES {
            return Err(AppError::message(format!(
                "The download exceeded the {} byte safety limit.",
                MAX_DOWNLOAD_BYTES
            )));
        }
        file.write_all(&chunk)
            .await
            .map_err(|err| super::io_error("Could not write the download file", destination, err))?;

        if last_emit.elapsed() >= Duration::from_millis(250) {
            emit_progress(
                app,
                kind,
                "downloading",
                downloaded,
                total,
                format!(
                    "Downloading {} … {}{}",
                    kind.label(),
                    format_bytes(downloaded),
                    total
                        .map(|total| format!(" / {}", format_bytes(total)))
                        .unwrap_or_default()
                ),
            );
            last_emit = Instant::now();
        }
    }

    file.flush()
        .await
        .map_err(|err| super::io_error("Could not flush the download file", destination, err))?;
    drop(file);

    if let Some(total) = total {
        if total != downloaded {
            return Err(AppError::message(format!(
                "The download ended early: received {downloaded} of {total} bytes. Retry the download from Settings."
            )));
        }
    }

    emit_progress(
        app,
        kind,
        "downloading",
        downloaded,
        total.or(Some(downloaded)),
        format!("Downloaded {}.", format_bytes(downloaded)),
    );
    Ok(())
}

fn emit_progress(
    app: &AppHandle,
    kind: EngineKind,
    phase: &str,
    downloaded: u64,
    total: Option<u64>,
    message: String,
) {
    let percent = total
        .filter(|total| *total > 0)
        .map(|total| (downloaded as f64 / total as f64 * 100.0).clamp(0.0, 100.0));
    let _ = app.emit(
        "engine://binary-progress",
        BinaryProgress {
            engine: kind,
            phase: phase.into(),
            downloaded,
            total,
            percent,
            message,
        },
    );
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct StagedPayload {
    pub(crate) binary: PathBuf,
    pub(crate) companions: Vec<PathBuf>,
}

pub(crate) fn extract_archive(
    archive: &Path,
    destination: &Path,
    binary_name: &str,
) -> AppResult<StagedPayload> {
    fs::create_dir_all(destination)
        .map_err(|err| super::io_error("Could not create the extraction folder", destination, err))?;
    let magic = read_magic(archive)?;

    if magic[0] == b'P' && magic[1] == b'K' {
        extract_zip(archive, destination)?;
    } else if magic[0] == 0x1f && magic[1] == 0x8b {
        extract_tar_gz(archive, destination)?;
    } else if is_executable_magic(&magic) {
        let target = destination.join(binary_name);
        fs::copy(archive, &target)
            .map_err(|err| super::io_error("Could not stage the downloaded executable", &target, err))?;
        set_executable(&target)?;
    } else {
        return Err(AppError::message(
            "The download is not a ZIP, a gzip-compressed tar archive, or a recognized executable. Download a release archive for this platform, not an HTML page or source tarball.",
        ));
    }

    // Release archives use symlinks for the SONAME aliases the executable asks
    // for (for example libllama.so.0 -> libllama.so.0.9.0). Archive extraction
    // deliberately skips symlinks, so materialize those aliases as regular
    // files before the runtime libraries are collected and installed.
    materialize_library_aliases(destination)?;

    let binary = find_binary(destination, binary_name).ok_or_else(|| {
        AppError::message(format!(
            "The archive did not contain {binary_name}. Check that it is the release build for this CPU and operating system."
        ))
    })?;
    let companions = collect_companions(destination, &binary);
    Ok(StagedPayload { binary, companions })
}

fn materialize_library_aliases(root: &Path) -> AppResult<()> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 8 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                walk(&path, depth + 1, out);
            } else if file_type.is_file() {
                out.push(path);
            }
        }
    }

    let mut libraries = Vec::new();
    walk(root, 0, &mut libraries);
    for source in libraries {
        let Some(name) = source.file_name().map(|value| value.to_string_lossy()) else {
            continue;
        };
        if !is_runtime_library(&name) {
            continue;
        }
        for alias in library_aliases(&name) {
            let destination = source.with_file_name(&alias);
            if destination.exists() {
                continue;
            }
            fs::copy(&source, &destination).map_err(|err| {
                super::io_error("Could not create a shared library alias", &destination, err)
            })?;
            set_executable(&destination)?;
        }
    }
    Ok(())
}

/// Derive the SONAME aliases that release archives normally express as
/// relative symlinks. We only derive numeric version suffixes; an arbitrary
/// archive filename can never name a path outside its own directory.
fn library_aliases(name: &str) -> Vec<String> {
    let lower = name.to_ascii_lowercase();
    if !lower.starts_with("lib") {
        return Vec::new();
    }

    let mut aliases = Vec::new();
    if let Some(index) = lower.find(".so.") {
        let version = &name[index + 4..];
        let major = version.split('.').next().unwrap_or_default();
        if major.is_empty() || !major.chars().all(|ch| ch.is_ascii_digit()) {
            return aliases;
        }
        aliases.push(format!("{}.so.{major}", &name[..index]));
        aliases.push(format!("{}.so", &name[..index]));
    } else if let Some(index) = lower.find(".dylib") {
        let base = &name[..index];
        let Some((stem, version)) = base.rsplit_once('.') else {
            return aliases;
        };
        if version.is_empty() || !version.chars().all(|ch| ch.is_ascii_digit()) {
            return aliases;
        }
        aliases.push(format!("{stem}.dylib"));
        if let Some((root, major)) = stem.rsplit_once('.') {
            if !major.is_empty() && major.chars().all(|ch| ch.is_ascii_digit()) {
                aliases.push(format!("{root}.dylib"));
            }
        }
    }
    aliases
}

fn read_magic(path: &Path) -> AppResult<[u8; 4]> {
    let mut file = File::open(path)
        .map_err(|err| super::io_error("Could not open the downloaded archive", path, err))?;
    let mut buffer = [0u8; 4];
    let read = file
        .read(&mut buffer)
        .map_err(|err| super::io_error("Could not read the downloaded archive", path, err))?;
    if read < 2 {
        return Err(AppError::message(
            "The downloaded file is empty or too small to be a release archive.",
        ));
    }
    Ok(buffer)
}

fn is_executable_magic(magic: &[u8; 4]) -> bool {
    magic[0] == 0x7f && magic[1] == b'E' && magic[2] == b'L' && magic[3] == b'F'
        || (magic[0] == b'M' && magic[1] == b'Z')
        || u32::from_be_bytes(*magic) == 0xfeed_face
        || u32::from_be_bytes(*magic) == 0xfeed_facf
        || u32::from_be_bytes(*magic) == 0xcefa_edfe
        || u32::from_be_bytes(*magic) == 0xcffa_edfe
}

fn extract_zip(archive: &Path, destination: &Path) -> AppResult<()> {
    let file = File::open(archive)
        .map_err(|err| super::io_error("Could not open the ZIP archive", archive, err))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|err| AppError::message(format!("Invalid ZIP archive: {err}")))?;
    if zip.len() > MAX_ARCHIVE_ENTRIES {
        return Err(AppError::message(
            "The ZIP archive contains too many entries to extract safely.",
        ));
    }

    let mut total = 0u64;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|err| AppError::message(format!("Could not read ZIP entry {index}: {err}")))?;
        total = total.saturating_add(entry.size());
        if total > MAX_EXTRACTED_BYTES {
            return Err(AppError::message(format!(
                "The extracted archive would exceed the {} byte safety limit.",
                MAX_EXTRACTED_BYTES
            )));
        }
        if entry
            .unix_mode()
            .map(|mode| mode & 0o170000 == 0o120000)
            .unwrap_or(false)
        {
            continue;
        }
        let Some(relative) = entry.enclosed_name().map(|path| path.to_path_buf()) else {
            return Err(AppError::message(
                "The ZIP archive contains a path that escapes the extraction folder.",
            ));
        };
        if !safe_relative(&relative) {
            return Err(AppError::message(
                "The ZIP archive contains an absolute or parent-directory path.",
            ));
        }
        let output = destination.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&output)
                .map_err(|err| super::io_error("Could not create an extracted folder", &output, err))?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| super::io_error("Could not create an extracted folder", parent, err))?;
        }
        let mut writer = File::create(&output)
            .map_err(|err| super::io_error("Could not create an extracted file", &output, err))?;
        io::copy(&mut entry, &mut writer)
            .map_err(|err| super::io_error("Could not extract a ZIP entry", &output, err))?;
    }
    Ok(())
}

fn extract_tar_gz(archive: &Path, destination: &Path) -> AppResult<()> {
    let file = File::open(archive)
        .map_err(|err| super::io_error("Could not open the tar archive", archive, err))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    let mut total = 0u64;
    let mut entries = 0usize;

    for entry in tar
        .entries()
        .map_err(|err| AppError::message(format!("Invalid tar archive: {err}")))?
    {
        let mut entry = entry
            .map_err(|err| AppError::message(format!("Could not read tar entry: {err}")))?;
        entries += 1;
        if entries > MAX_ARCHIVE_ENTRIES {
            return Err(AppError::message(
                "The tar archive contains too many entries to extract safely.",
            ));
        }
        if !entry.header().entry_type().is_file() {
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > MAX_EXTRACTED_BYTES {
            return Err(AppError::message(format!(
                "The extracted archive would exceed the {} byte safety limit.",
                MAX_EXTRACTED_BYTES
            )));
        }
        let relative = entry
            .path()
            .map_err(|err| AppError::message(format!("Invalid tar entry path: {err}")))?
            .to_path_buf();
        if !safe_relative(&relative) {
            return Err(AppError::message(
                "The tar archive contains an absolute or parent-directory path.",
            ));
        }
        let output = destination.join(&relative);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| super::io_error("Could not create an extracted folder", parent, err))?;
        }
        let unpacked = entry
            .unpack_in(destination)
            .map_err(|err| AppError::message(format!("Could not extract a tar entry: {err}")))?;
        if !unpacked {
            return Err(AppError::message(
                "A tar entry resolved outside the extraction folder and was rejected.",
            ));
        }
    }
    Ok(())
}

fn safe_relative(path: &Path) -> bool {
    path.components().all(|component| matches!(component, Component::Normal(_)))
}

pub(crate) fn find_binary(root: &Path, name: &str) -> Option<PathBuf> {
    fn walk(
        dir: &Path,
        name: &str,
        depth: usize,
        exact: &mut Option<PathBuf>,
        folded: &mut Option<PathBuf>,
    ) {
        if depth > 8 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                walk(&path, name, depth + 1, exact, folded);
                continue;
            }
            let Some(file_name) = path.file_name().map(|value| value.to_string_lossy().to_string())
            else {
                continue;
            };
            if file_name == name {
                *exact = Some(path);
                return;
            }
            if folded.is_none() && file_name.eq_ignore_ascii_case(name) {
                *folded = Some(path);
            }
        }
    }

    let mut exact = None;
    let mut folded = None;
    walk(root, name, 0, &mut exact, &mut folded);
    exact.or(folded)
}

fn collect_companions(root: &Path, binary: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, binary: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 8 || out.len() >= 64 {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                walk(&path, binary, depth + 1, out);
                continue;
            }
            if path == binary {
                continue;
            }
            let Some(name) = path.file_name().map(|value| value.to_string_lossy().to_string())
            else {
                continue;
            };
            if is_runtime_library(&name) {
                out.push(path);
            }
        }
    }

    let mut companions = Vec::new();
    walk(root, binary, 0, &mut companions);
    companions
}

pub(crate) fn is_runtime_library(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.starts_with("lib") && (lower.contains(".so") || lower.ends_with(".dylib")))
        || lower.ends_with(".dll")
}

pub(crate) fn set_executable(path: &Path) -> AppResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .map_err(|err| super::io_error("Could not set executable permissions", path, err))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct PreparedInstall {
    target: PathBuf,
    backup: Option<PathBuf>,
    lib_backup: PathBuf,
}

fn prepare_install(
    bin: &Path,
    kind: EngineKind,
    staged_binary: &Path,
    companions: &[PathBuf],
) -> AppResult<PreparedInstall> {
    let target = bin.join(kind.binary_name());
    let backup = super::backup_path(&target);
    let lib_backup = bin.join(".rollback").join(kind.storage_name());
    let existing = target.is_file();
    let temp = bin.join(format!(".{}.{}.new", kind.binary_stem(), Uuid::new_v4()));

    let outcome = (|| -> AppResult<()> {
        if existing {
            if backup.exists() {
                fs::remove_file(&backup)
                    .map_err(|err| super::io_error("Could not remove the old .bak", &backup, err))?;
            }
            fs::rename(&target, &backup).map_err(|err| {
                super::io_error(
                    "Could not move the current binary to .bak",
                    &target,
                    err,
                )
            })?;
        }

        if lib_backup.exists() {
            fs::remove_dir_all(&lib_backup).map_err(|err| {
                super::io_error("Could not clear the old rollback libraries", &lib_backup, err)
            })?;
        }
        fs::create_dir_all(&lib_backup).map_err(|err| {
            super::io_error("Could not create the rollback library folder", &lib_backup, err)
        })?;
        copy_runtime_libraries(bin, &lib_backup)?;

        for source in companions {
            let Some(name) = source.file_name() else {
                continue;
            };
            let destination = bin.join(name);
            fs::copy(source, &destination).map_err(|err| {
                super::io_error("Could not install a shared library", &destination, err)
            })?;
            set_executable(&destination)?;
        }

        fs::copy(staged_binary, &temp).map_err(|err| {
            super::io_error("Could not copy the new binary into place", &temp, err)
        })?;
        set_executable(&temp)?;
        fs::rename(&temp, &target).map_err(|err| {
            super::io_error("Could not atomically replace the engine binary", &target, err)
        })?;
        Ok(())
    })();

    match outcome {
        Ok(()) => Ok(PreparedInstall {
            target,
            backup: existing.then_some(backup),
            lib_backup,
        }),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            let _ = restore_previous(&target, &backup, &lib_backup, existing);
            Err(err)
        }
    }
}

fn copy_runtime_libraries(bin: &Path, destination: &Path) -> AppResult<()> {
    let entries = fs::read_dir(bin)
        .map_err(|err| super::io_error("Could not read the managed bin folder", bin, err))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().map(|value| value.to_string_lossy().to_string()) else {
            continue;
        };
        if !is_runtime_library(&name) {
            continue;
        }
        let target = destination.join(&name);
        fs::copy(&path, &target)
            .map_err(|err| super::io_error("Could not back up a shared library", &target, err))?;
    }
    Ok(())
}

fn restore_runtime_libraries(bin: &Path, backup: &Path) -> AppResult<()> {
    if !backup.is_dir() {
        return Ok(());
    }
    let entries = fs::read_dir(backup)
        .map_err(|err| super::io_error("Could not read the rollback libraries", backup, err))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        let destination = bin.join(name);
        fs::copy(&path, &destination).map_err(|err| {
            super::io_error("Could not restore a shared library", &destination, err)
        })?;
        set_executable(&destination)?;
    }
    Ok(())
}

fn restore_previous(
    target: &Path,
    backup: &Path,
    lib_backup: &Path,
    existing: bool,
) -> AppResult<()> {
    let bin = target.parent().ok_or_else(|| {
        AppError::message("The managed binary path has no parent directory.")
    })?;
    restore_runtime_libraries(bin, lib_backup)?;

    if backup.is_file() {
        if target.exists() {
            fs::remove_file(target).map_err(|err| {
                super::io_error("Could not remove the failed binary", target, err)
            })?;
        }
        fs::rename(backup, target).map_err(|err| {
            super::io_error("Could not restore the previous binary", target, err)
        })?;
        set_executable(target)?;
    } else if !existing && target.exists() {
        fs::remove_file(target).map_err(|err| {
            super::io_error("Could not remove the failed first install", target, err)
        })?;
    }
    Ok(())
}

async fn verify_installed(target: &Path, bin: &Path) -> AppResult<Option<String>> {
    if !target.is_file() {
        return Err(AppError::message(format!(
            "{} is missing after installation.",
            target.display()
        )));
    }
    let output = run_cli_probe(target, Some(bin)).await?;
    if output.version_line.is_none() && output.help_text.trim().is_empty() {
        return Err(AppError::message(
            "The binary started but produced no version or help output. It may be the wrong architecture or missing a loader dependency.",
        ));
    }
    Ok(output.version_line)
}

fn restore_after_failed_verify(bin: &Path, prepared: &PreparedInstall) -> AppResult<()> {
    if prepared.target.exists() {
        fs::remove_file(&prepared.target).map_err(|err| {
            super::io_error("Could not remove the failed new binary", &prepared.target, err)
        })?;
    }
    if let Some(backup) = &prepared.backup {
        if !backup.is_file() {
            return Err(AppError::message(format!(
                "The rollback backup {} is missing.",
                backup.display()
            )));
        }
        fs::rename(backup, &prepared.target).map_err(|err| {
            super::io_error("Could not restore the previous binary", &prepared.target, err)
        })?;
        set_executable(&prepared.target)?;
    }
    restore_runtime_libraries(bin, &prepared.lib_backup)?;
    Ok(())
}

#[derive(Debug, Clone)]
struct PreparedRollback {
    target: PathBuf,
    failed: Option<PathBuf>,
    current_libs: PathBuf,
}

fn prepare_rollback(bin: &Path, kind: EngineKind) -> AppResult<PreparedRollback> {
    let target = bin.join(kind.binary_name());
    let backup = super::backup_path(&target);
    if !backup.is_file() {
        return Err(AppError::message(format!(
            "No rollback backup exists for {}. Install an update first; the previous binary is kept as {}.",
            kind.label(),
            backup.display()
        )));
    }

    let file_name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| kind.binary_name());
    let failed = bin.join(format!("{file_name}.rollback-failed"));
    let rollback_root = bin.join(".rollback");
    let current_libs = rollback_root.join(format!("{}-current", kind.storage_name()));
    let previous_libs = rollback_root.join(kind.storage_name());

    if failed.exists() {
        fs::remove_file(&failed)
            .map_err(|err| super::io_error("Could not clear an old rollback file", &failed, err))?;
    }
    if current_libs.exists() {
        fs::remove_dir_all(&current_libs).map_err(|err| {
            super::io_error("Could not clear old rollback libraries", &current_libs, err)
        })?;
    }
    fs::create_dir_all(&current_libs).map_err(|err| {
        super::io_error("Could not save the current libraries before rollback", &current_libs, err)
    })?;
    copy_runtime_libraries(bin, &current_libs)?;

    let had_target = target.is_file();
    if had_target {
        fs::rename(&target, &failed).map_err(|err| {
            super::io_error("Could not move the current binary aside for rollback", &target, err)
        })?;
    }
    if let Err(err) = fs::rename(&backup, &target) {
        if had_target {
            let _ = fs::rename(&failed, &target);
        }
        return Err(super::io_error("Could not restore the .bak binary", &target, err));
    }
    if let Err(err) = restore_runtime_libraries(bin, &previous_libs) {
        // Put the newer binary and its libraries back before reporting failure.
        let _ = fs::copy(&target, &backup);
        let _ = fs::remove_file(&target);
        if had_target {
            let _ = fs::rename(&failed, &target);
        }
        let _ = restore_runtime_libraries(bin, &current_libs);
        return Err(err);
    }
    if let Err(err) = set_executable(&target) {
        return Err(err);
    }

    Ok(PreparedRollback {
        target,
        failed: had_target.then_some(failed),
        current_libs,
    })
}

fn finish_rollback(bin: &Path, prepared: &PreparedRollback) -> AppResult<()> {
    if let Some(failed) = &prepared.failed {
        if failed.exists() {
            fs::remove_file(failed).map_err(|err| {
                super::io_error("Could not remove the newer binary after rollback", failed, err)
            })?;
        }
    }
    let rollback_root = bin.join(".rollback");
    if prepared.current_libs.starts_with(&rollback_root) && prepared.current_libs.exists() {
        let _ = fs::remove_dir_all(&prepared.current_libs);
    }
    Ok(())
}

fn undo_rollback(bin: &Path, prepared: &PreparedRollback) -> AppResult<()> {
    if prepared.target.exists() {
        fs::remove_file(&prepared.target).map_err(|err| {
            super::io_error("Could not remove the failed rollback binary", &prepared.target, err)
        })?;
    }
    if let Some(failed) = &prepared.failed {
        if failed.is_file() {
            fs::rename(failed, &prepared.target).map_err(|err| {
                super::io_error("Could not restore the newer binary", &prepared.target, err)
            })?;
            set_executable(&prepared.target)?;
        }
    }
    restore_runtime_libraries(bin, &prepared.current_libs)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_shared_libraries_and_ignores_ordinary_files() {
        assert!(is_runtime_library("libllama.so"));
        assert!(is_runtime_library("libggml-cpu.so.0"));
        assert!(is_runtime_library("llama.dll"));
        assert!(!is_runtime_library("llama-server"));
        assert!(!is_runtime_library("README.md"));
    }

    #[cfg(unix)]
    #[test]
    fn materializes_versioned_library_soname_aliases() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "janas-library-alias-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("create alias test directory");
        let versioned = root.join("libllama.so.0.9.0");
        fs::write(&versioned, b"test library").expect("write versioned library");
        fs::set_permissions(&versioned, fs::Permissions::from_mode(0o644))
            .expect("set versioned library permissions");
        let unversioned = root.join("libggml-cpu-zen4.so");
        fs::write(&unversioned, b"test backend").expect("write unversioned library");

        materialize_library_aliases(&root).expect("materialize aliases");

        assert_eq!(
            fs::read(root.join("libllama.so.0")).expect("read SONAME alias"),
            b"test library"
        );
        assert_eq!(
            fs::read(root.join("libllama.so")).expect("read linker-name alias"),
            b"test library"
        );
        assert!(!root.join("libggml-cpu-zen4.so.0").exists());
        assert_eq!(
            fs::metadata(root.join("libllama.so.0"))
                .expect("alias metadata")
                .permissions()
                .mode()
                & 0o111,
            0o111,
            "library aliases must remain executable"
        );
        assert_eq!(
            library_aliases("libmtmd.0.9.dylib"),
            vec!["libmtmd.0.dylib", "libmtmd.dylib"]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_absolute_and_parent_paths() {
        assert!(safe_relative(Path::new("lib/libllama.so")));
        assert!(!safe_relative(Path::new("../libllama.so")));
        assert!(!safe_relative(Path::new("/tmp/libllama.so")));
    }

    #[test]
    fn backup_name_is_kept_next_to_the_binary() {
        let path = Path::new("/tmp/bin/llama-server");
        assert_eq!(super::super::backup_path(path), Path::new("/tmp/bin/llama-server.bak"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_update_verification_restores_previous_binary() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "janas-failed-update-{}",
            uuid::Uuid::new_v4()
        ));
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("create managed bin directory");

        let target = bin.join(EngineKind::Llama.binary_name());
        let old_binary = "#!/bin/sh\nprintf 'old harness binary\\n'\n";
        fs::write(&target, old_binary).expect("write old binary");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))
            .expect("make old binary executable");

        let staged = root.join("staged-llama-server");
        let broken_binary = "#!/bin/sh\nexit 0\n";
        fs::write(&staged, broken_binary).expect("write broken update");
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
            .expect("make broken update executable");

        let prepared = prepare_install(&bin, EngineKind::Llama, &staged, &[])
            .expect("prepare failed update");
        let backup = prepared.backup.clone().expect("previous binary backup");
        assert_eq!(fs::read_to_string(&prepared.target).unwrap(), broken_binary);
        assert_eq!(fs::read_to_string(&backup).unwrap(), old_binary);

        // Installed binaries are verified by running their probe commands. A
        // process that exits without output must fail that check.
        let verification = verify_installed(&prepared.target, &bin).await;
        assert!(verification.is_err(), "silent binary should not verify");

        restore_after_failed_verify(&bin, &prepared).expect("restore previous binary");
        assert_eq!(fs::read_to_string(&prepared.target).unwrap(), old_binary);
        assert!(!backup.exists(), "restore should consume the .bak file");
        let mode = fs::metadata(&prepared.target)
            .expect("restored binary metadata")
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0, "restored binary should stay executable");

        let _ = fs::remove_dir_all(root);
    }
}
