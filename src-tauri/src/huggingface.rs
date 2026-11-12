use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

use crate::{
    error::{AppError, AppResult},
    settings::AppSettings,
};

const USER_AGENT: &str = "Janas-Studio/0.1 (+https://github.com/prabanta-dev/janas)";
const HF_API: &str = "https://huggingface.co/api/models";
const HF_RESOLVE: &str = "https://huggingface.co";

#[derive(Debug, Clone, Serialize)]
pub struct HfFile {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HfQuant {
    pub quant: String,
    pub files: Vec<HfFile>,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HfModel {
    pub id: String,
    pub author: String,
    pub name: String,
    pub downloads: u64,
    pub likes: u64,
    pub pipeline_tag: Option<String>,
    pub tags: Vec<String>,
    pub parameters: Option<u64>,
    pub last_modified: Option<String>,
    pub quants: Vec<HfQuant>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchRequest {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub gguf_only: bool,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    24
}

#[derive(Debug, Deserialize)]
struct ApiModel {
    id: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    downloads: Option<u64>,
    #[serde(default)]
    likes: Option<u64>,
    #[serde(default)]
    pipeline_tag: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    last_modified: Option<String>,
    #[serde(default)]
    safetensors: Option<SafetensorsInfo>,
}

#[derive(Debug, Deserialize)]
struct SafetensorsInfo {
    #[serde(default)]
    total: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct TreeEntry {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    path: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    lfs: Option<LfsInfo>,
}

#[derive(Debug, Deserialize)]
struct LfsInfo {
    #[serde(default)]
    size: Option<u64>,
}

fn metadata_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

pub async fn search(request: SearchRequest) -> AppResult<Vec<HfModel>> {
    let limit = request.limit.clamp(1, 40);
    let mut url = format!("{HF_API}?limit={limit}&sort=downloads&direction=-1&full=true");
    let query = request.query.trim();
    if !query.is_empty() {
        url.push_str("&search=");
        url.push_str(&urlencoding::encode(query));
    }
    if request.gguf_only {
        url.push_str("&filter=gguf");
    }

    let client = metadata_client();
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|err| AppError::message(format!("Hugging Face search failed: {err}")))?;

    if !response.status().is_success() {
        return Err(AppError::message(format!(
            "Hugging Face returned HTTP {} for this search.",
            response.status()
        )));
    }

    let models: Vec<ApiModel> = response
        .json()
        .await
        .map_err(|err| AppError::message(format!("Could not read the Hugging Face response: {err}")))?;

    let mut enriched = futures_util::stream::iter(models.into_iter().map(|model| {
        let client = client.clone();
        async move {
            let quants = fetch_quants(&client, &model.id).await.unwrap_or_default();
            let author = model
                .author
                .clone()
                .or_else(|| model.id.split('/').next().map(str::to_string))
                .unwrap_or_else(|| "unknown".into());
            let name = model
                .id
                .split('/')
                .nth(1)
                .map(str::to_string)
                .unwrap_or_else(|| model.id.clone());
            HfModel {
                id: model.id,
                author,
                name,
                downloads: model.downloads.unwrap_or(0),
                likes: model.likes.unwrap_or(0),
                pipeline_tag: model.pipeline_tag,
                tags: model.tags,
                parameters: model.safetensors.and_then(|info| info.total),
                last_modified: model.last_modified,
                quants,
            }
        }
    }))
    .buffer_unordered(8)
    .collect::<Vec<HfModel>>()
    .await;

    enriched.sort_by(|a, b| b.downloads.cmp(&a.downloads));
    Ok(enriched)
}

pub async fn list_quants(repo: String) -> AppResult<Vec<HfQuant>> {
    validate_repo(&repo)?;
    let client = metadata_client();
    fetch_quants(&client, &repo)
        .await
        .map_err(|err| AppError::message(format!("Could not list this model's files: {err}")))
}

async fn fetch_quants(
    client: &reqwest::Client,
    repo: &str,
) -> Result<Vec<HfQuant>, reqwest::Error> {
    let url = format!("{HF_API}/{repo}/tree/main?recursive=true&expand=true");
    let entries: Vec<TreeEntry> = client
        .get(url)
        .timeout(Duration::from_secs(8))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(build_quants(entries))
}

fn build_quants(entries: Vec<TreeEntry>) -> Vec<HfQuant> {
    let mut groups: BTreeMap<String, HfQuant> = BTreeMap::new();

    for entry in entries {
        if entry
            .kind
            .as_deref()
            .map(|kind| kind != "file")
            .unwrap_or(false)
        {
            continue;
        }
        let lower = entry.path.to_ascii_lowercase();
        if !lower.ends_with(".gguf") {
            continue;
        }
        let file_name = lower.rsplit('/').next().unwrap_or(&lower);
        // Vision encoders, draft/MTP blocks and importance matrices are not the
        // chat model itself; keep them out of the download picker.
        if file_name.starts_with("mmproj")
            || file_name.starts_with("mtp-")
            || lower.contains("imatrix")
            || lower.contains("vision")
        {
            continue;
        }
        let quant = detect_quant(&lower).unwrap_or_else(|| "GGUF".into());
        let size = entry
            .lfs
            .and_then(|lfs| lfs.size)
            .or(entry.size)
            .unwrap_or(0);
        let group = groups.entry(quant.clone()).or_insert_with(|| HfQuant {
            quant,
            files: Vec::new(),
            size_bytes: 0,
        });
        group.size_bytes = group.size_bytes.saturating_add(size);
        group.files.push(HfFile {
            path: entry.path,
            size,
        });
    }

    let mut quants: Vec<HfQuant> = groups.into_values().collect();
    for quant in &mut quants {
        quant
            .files
            .sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));
    }
    quants.sort_by(|a, b| {
        quant_rank(&a.quant)
            .cmp(&quant_rank(&b.quant))
            .then(a.size_bytes.cmp(&b.size_bytes))
    });
    quants
}

const QUANT_TOKENS: &[&str] = &[
    "UD-Q6_K_XL",
    "UD-Q5_K_XL",
    "UD-Q4_K_XL",
    "UD-Q3_K_XL",
    "Q4_K_M",
    "Q4_K_S",
    "Q5_K_M",
    "Q5_K_S",
    "Q6_K",
    "Q8_0",
    "Q4_1",
    "Q4_0",
    "Q5_1",
    "Q3_K_M",
    "Q3_K_S",
    "Q3_K_L",
    "Q2_K",
    "IQ4_XS",
    "IQ4_NL",
    "IQ3_S",
    "MXFP4_MOE",
    "BF16",
    "F16",
];

fn detect_quant(file_name: &str) -> Option<String> {
    let upper = file_name.to_ascii_uppercase();
    QUANT_TOKENS
        .iter()
        .find(|token| upper.contains(**token))
        .map(|token| token.to_string())
}

fn quant_rank(quant: &str) -> usize {
    QUANT_TOKENS
        .iter()
        .position(|token| token.eq_ignore_ascii_case(quant))
        .unwrap_or(99)
}

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadFile {
    pub path: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DownloadRequest {
    pub repo: String,
    pub files: Vec<DownloadFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadStarted {
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub id: String,
    pub repo: String,
    pub file: String,
    pub downloaded: u64,
    pub total: u64,
    pub percent: Option<f64>,
    pub files_done: usize,
    pub files_total: usize,
    pub status: String,
    pub message: String,
    pub saved_path: Option<String>,
}

#[derive(Default)]
pub struct DownloadManager {
    tasks: Mutex<HashMap<String, Arc<AtomicBool>>>,
    paused: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl DownloadManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(
        self: &Arc<Self>,
        app: AppHandle,
        settings: &AppSettings,
        request: DownloadRequest,
    ) -> AppResult<DownloadStarted> {
        validate_repo(&request.repo)?;
        if request.files.is_empty() {
            return Err(AppError::message("Choose at least one GGUF file to download."));
        }
        if settings.models_dir.trim().is_empty() {
            return Err(AppError::message("Set a models folder in Settings first."));
        }
        for file in &request.files {
            safe_relative(&file.path)?;
        }

        let id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        self.tasks
            .lock()
            .map_err(|_| AppError::message("Download state is locked."))?
            .insert(id.clone(), cancel.clone());
        self.paused
            .lock()
            .map_err(|_| AppError::message("Download state is locked."))?
            .insert(id.clone(), pause.clone());

        let manager = Arc::clone(self);
        let repo = request.repo.clone();
        let files = request.files.clone();
        let destination = PathBuf::from(settings.models_dir.trim());
        let task_id = id.clone();
        let app_handle = app.clone();

        let manifest_root = destination.clone();
        crate::system::record_download(
            &manifest_root,
            crate::system::DownloadRecord {
                repo: repo.clone(),
                quant: quant_from_files(&files),
                files: files.iter().map(|file| file.path.clone()).collect(),
                saved_at: crate::util::now_ms().to_string(),
                size_bytes: files.iter().map(|file| file.size).sum(),
            },
        );

        tauri::async_runtime::spawn(async move {
            run_download(
                app_handle,
                task_id.clone(),
                cancel,
                pause,
                repo,
                files,
                destination,
            )
            .await;
            manager.finish(&task_id);
        });

        Ok(DownloadStarted { id })
    }

    pub fn cancel(&self, id: &str) -> bool {
        let Ok(guard) = self.tasks.lock() else {
            return false;
        };
        if let Some(flag) = guard.get(id) {
            flag.store(true, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Pausing keeps the HTTP stream open but stops consuming it, so a resume
    /// continues from the exact byte the disk already holds.
    pub fn pause(&self, id: &str) -> bool {
        self.paused
            .lock()
            .ok()
            .and_then(|guard| guard.get(id).map(|flag| (flag.store(true, Ordering::Relaxed), true).1))
            .unwrap_or(false)
    }

    pub fn resume(&self, id: &str) -> bool {
        self.paused
            .lock()
            .ok()
            .and_then(|guard| {
                guard
                    .get(id)
                    .map(|flag| (flag.store(false, Ordering::Relaxed), true).1)
            })
            .unwrap_or(false)
    }

    fn finish(&self, id: &str) {
        if let Ok(mut guard) = self.tasks.lock() {
            guard.remove(id);
        }
        if let Ok(mut guard) = self.paused.lock() {
            guard.remove(id);
        }
    }
}

async fn run_download(
    app: AppHandle,
    id: String,
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    repo: String,
    files: Vec<DownloadFile>,
    models_dir: PathBuf,
) {
    let files_total = files.len();
    let known_total: Option<u64> = if files.iter().all(|file| file.size > 0) {
        Some(files.iter().map(|file| file.size).sum())
    } else {
        None
    };
    let root = models_dir.join(repo.replace('/', std::path::MAIN_SEPARATOR_STR));
    let client = match reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(20))
        .build()
    {
        Ok(client) => client,
        Err(_) => reqwest::Client::new(),
    };

    emit_progress(
        &app,
        DownloadProgress {
            id: id.clone(),
            repo: repo.clone(),
            file: String::new(),
            downloaded: 0,
            total: known_total.unwrap_or(0),
            percent: Some(0.0),
            files_done: 0,
            files_total,
            status: "preparing".into(),
            message: format!("Preparing {files_total} file(s)…"),
            saved_path: Some(root.to_string_lossy().to_string()),
        },
    );

    let mut downloaded_total = 0u64;

    for (index, file) in files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            emit_progress(
                &app,
                cancelled_payload(&id, &repo, files_total, downloaded_total, known_total, &root),
            );
            return;
        }

        let relative = match safe_relative(&file.path) {
            Ok(path) => path,
            Err(err) => {
                emit_error(
                    &app,
                    &id,
                    &repo,
                    file,
                    files_total,
                    format!("Unsafe file path: {err}"),
                );
                return;
            }
        };
        let destination = root.join(&relative);
        if let Some(parent) = destination.parent() {
            if let Err(err) = tokio::fs::create_dir_all(parent).await {
                emit_error(
                    &app,
                    &id,
                    &repo,
                    file,
                    files_total,
                    format!("Could not create {}: {err}", parent.display()),
                );
                return;
            }
        }

        let url = format!(
            "{HF_RESOLVE}/{}/resolve/main/{}?download=true",
            repo.split('/')
                .map(|segment| urlencoding::encode(segment).to_string())
                .collect::<Vec<_>>()
                .join("/"),
            file.path
                .split('/')
                .map(|segment| urlencoding::encode(segment).to_string())
                .collect::<Vec<_>>()
                .join("/")
        );

        let response = match client.get(url).send().await {
            Ok(response) => response,
            Err(err) => {
                emit_error(
                    &app,
                    &id,
                    &repo,
                    file,
                    files_total,
                    format!("Download failed: {err}"),
                );
                return;
            }
        };
        if !response.status().is_success() {
            emit_error(
                &app,
                &id,
                &repo,
                file,
                files_total,
                format!("Hugging Face returned HTTP {}.", response.status()),
            );
            return;
        }

        let file_total = response
            .content_length()
            .filter(|length| *length > 0)
            .unwrap_or(file.size);
        let total_for_display = known_total.unwrap_or(downloaded_total + file_total);
        let part = destination.with_extension(format!(
            "{}.part",
            destination
                .extension()
                .map(|ext| ext.to_string_lossy().to_string())
                .unwrap_or_default()
        ));

        let mut handle = match tokio::fs::File::create(&part).await {
            Ok(handle) => handle,
            Err(err) => {
                emit_error(
                    &app,
                    &id,
                    &repo,
                    file,
                    files_total,
                    format!("Could not write {}: {err}", part.display()),
                );
                return;
            }
        };

        let mut stream = response.bytes_stream();
        let mut file_downloaded = 0u64;
        let mut last_emit = Instant::now();
        let mut failed = None;

        while let Some(chunk) = stream.next().await {
            if pause.load(Ordering::Relaxed) {
                if let Err(err) = handle.flush().await {
                    failed = Some(format!("Could not flush the file: {err}"));
                    break;
                }
                emit_progress(
                    &app,
                    DownloadProgress {
                        id: id.clone(),
                        repo: repo.clone(),
                        file: file.path.clone(),
                        downloaded: downloaded_total,
                        total: total_for_display,
                        percent: Some(if total_for_display > 0 {
                            (downloaded_total as f64 / total_for_display as f64 * 100.0)
                                .clamp(0.0, 100.0)
                        } else {
                            0.0
                        }),
                        files_done: index,
                        files_total,
                        status: "paused".into(),
                        message: format!("Paused at {} - resume any time", file.path),
                        saved_path: Some(root.to_string_lossy().to_string()),
                    },
                );
                while pause.load(Ordering::Relaxed) {
                    if cancel.load(Ordering::Relaxed) {
                        drop(handle);
                        let _ = tokio::fs::remove_file(&part).await;
                        emit_progress(
                            &app,
                            cancelled_payload(
                                &id,
                                &repo,
                                files_total,
                                downloaded_total,
                                known_total,
                                &root,
                            ),
                        );
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
                emit_progress(
                    &app,
                    DownloadProgress {
                        id: id.clone(),
                        repo: repo.clone(),
                        file: file.path.clone(),
                        downloaded: downloaded_total,
                        total: total_for_display,
                        percent: Some(if total_for_display > 0 {
                            (downloaded_total as f64 / total_for_display as f64 * 100.0)
                                .clamp(0.0, 100.0)
                        } else {
                            0.0
                        }),
                        files_done: index,
                        files_total,
                        status: "downloading".into(),
                        message: format!("Resumed {}", file.path),
                        saved_path: Some(root.to_string_lossy().to_string()),
                    },
                );
            }
            if cancel.load(Ordering::Relaxed) {
                drop(handle);
                let _ = tokio::fs::remove_file(&part).await;
                emit_progress(
                    &app,
                    cancelled_payload(
                        &id,
                        &repo,
                        files_total,
                        downloaded_total,
                        known_total,
                        &root,
                    ),
                );
                return;
            }
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(err) => {
                    failed = Some(format!("Transfer interrupted: {err}"));
                    break;
                }
            };
            if let Err(err) = handle.write_all(&chunk).await {
                failed = Some(format!("Could not write to disk: {err}"));
                break;
            }
            let length = chunk.len() as u64;
            file_downloaded = file_downloaded.saturating_add(length);
            downloaded_total = downloaded_total.saturating_add(length);

            if last_emit.elapsed() >= Duration::from_millis(140) {
                last_emit = Instant::now();
                emit_progress(
                    &app,
                    DownloadProgress {
                        id: id.clone(),
                        repo: repo.clone(),
                        file: file.path.clone(),
                        downloaded: downloaded_total,
                        total: total_for_display,
                        percent: Some(if total_for_display > 0 {
                            (downloaded_total as f64 / total_for_display as f64 * 100.0)
                                .clamp(0.0, 100.0)
                        } else {
                            0.0
                        }),
                        files_done: index,
                        files_total,
                        status: "downloading".into(),
                        message: format!("Downloading {}", file.path),
                        saved_path: Some(root.to_string_lossy().to_string()),
                    },
                );
            }
        }

        if let Some(message) = failed {
            drop(handle);
            let _ = tokio::fs::remove_file(&part).await;
            emit_error(&app, &id, &repo, file, files_total, message);
            return;
        }

        if let Err(err) = handle.flush().await {
            drop(handle);
            let _ = tokio::fs::remove_file(&part).await;
            emit_error(
                &app,
                &id,
                &repo,
                file,
                files_total,
                format!("Could not flush the file: {err}"),
            );
            return;
        }
        drop(handle);

        if let Err(err) = tokio::fs::rename(&part, &destination).await {
            let _ = tokio::fs::remove_file(&part).await;
            emit_error(
                &app,
                &id,
                &repo,
                file,
                files_total,
                format!("Could not finalize {}: {err}", destination.display()),
            );
            return;
        }

        emit_progress(
            &app,
            DownloadProgress {
                id: id.clone(),
                repo: repo.clone(),
                file: file.path.clone(),
                downloaded: downloaded_total,
                total: known_total.unwrap_or(downloaded_total),
                percent: Some(if let Some(total) = known_total {
                    (downloaded_total as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
                } else {
                    (index + 1) as f64 / files_total as f64 * 100.0
                }),
                files_done: index + 1,
                files_total,
                status: "downloading".into(),
                message: format!("Saved {}", file.path),
                saved_path: Some(root.to_string_lossy().to_string()),
            },
        );
    }

    emit_progress(
        &app,
        DownloadProgress {
            id,
            repo,
            file: String::new(),
            downloaded: downloaded_total,
            total: known_total.unwrap_or(downloaded_total),
            percent: Some(100.0),
            files_done: files_total,
            files_total,
            status: "complete".into(),
            message: format!("Downloaded {files_total} file(s). Convert them to .jns in the Compiler tab."),
            saved_path: Some(root.to_string_lossy().to_string()),
        },
    );
}

fn quant_from_files(files: &[DownloadFile]) -> String {
    let haystack = files
        .iter()
        .map(|file| file.path.to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join(" ");
    QUANT_TOKENS
        .iter()
        .find(|token| haystack.contains(**token))
        .map(|token| token.to_string())
        .unwrap_or_else(|| "GGUF".into())
}

fn cancelled_payload(
    id: &str,
    repo: &str,
    files_total: usize,
    downloaded: u64,
    total: Option<u64>,
    root: &Path,
) -> DownloadProgress {
    DownloadProgress {
        id: id.to_string(),
        repo: repo.to_string(),
        file: String::new(),
        downloaded,
        total: total.unwrap_or(downloaded),
        percent: None,
        files_done: 0,
        files_total,
        status: "cancelled".into(),
        message: "Download cancelled.".into(),
        saved_path: Some(root.to_string_lossy().to_string()),
    }
}

fn emit_progress(app: &AppHandle, payload: DownloadProgress) {
    let _ = app.emit("download://progress", payload);
}

fn emit_error(
    app: &AppHandle,
    id: &str,
    repo: &str,
    file: &DownloadFile,
    files_total: usize,
    message: String,
) {
    emit_progress(
        app,
        DownloadProgress {
            id: id.to_string(),
            repo: repo.to_string(),
            file: file.path.clone(),
            downloaded: 0,
            total: file.size,
            percent: None,
            files_done: 0,
            files_total,
            status: "error".into(),
            message,
            saved_path: None,
        },
    );
}

fn validate_repo(repo: &str) -> AppResult<()> {
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || !part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        })
    {
        return Err(AppError::message(
            "A Hugging Face model id must look like \"owner/model\".",
        ));
    }
    Ok(())
}

fn safe_relative(path: &str) -> AppResult<PathBuf> {
    let mut out = PathBuf::new();
    for segment in path.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.contains('\\')
            || segment.chars().any(|ch| ch == '\0')
        {
            return Err(AppError::message(
                "The file path contains an unsafe segment.",
            ));
        }
        out.push(segment);
    }
    if out.as_os_str().is_empty() {
        return Err(AppError::message("The file path is empty."));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::now_ms;

    fn entry(path: &str, size: u64) -> TreeEntry {
        TreeEntry {
            kind: Some("file".into()),
            path: path.into(),
            size: Some(size),
            lfs: None,
        }
    }

    #[test]
    fn detects_common_quants_without_matching_ud_prefix() {
        assert_eq!(
            detect_quant("Qwen_Qwen3-4B-Q4_K_M.gguf"),
            Some("Q4_K_M".into())
        );
        assert_eq!(
            detect_quant("qwen3-30b-a3b-ud-q4_k_xl.gguf"),
            Some("UD-Q4_K_XL".into())
        );
        assert_eq!(detect_quant("model.gguf"), None);
    }

    #[test]
    fn groups_split_shards_and_skips_sidecars() {
        let quants = build_quants(vec![
            entry("Q4_K_M/model-00001-of-00002.gguf", 100),
            entry("Q4_K_M/model-00002-of-00002.gguf", 100),
            entry("mmproj-model-f16.gguf", 50),
            entry("Q8_0/model.gguf", 400),
            entry("README.md", 2),
        ]);
        assert_eq!(quants.len(), 2);
        assert_eq!(quants[0].quant, "Q4_K_M");
        assert_eq!(quants[0].files.len(), 2);
        assert_eq!(quants[0].size_bytes, 200);
        assert_eq!(quants[1].quant, "Q8_0");
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(safe_relative("../../etc/passwd").is_err());
        assert!(safe_relative("sub/file.gguf").is_ok());
        assert_eq!(
            safe_relative("sub/file.gguf").unwrap(),
            Path::new("sub").join("file.gguf")
        );
    }

    #[test]
    fn validates_repo_ids() {
        assert!(validate_repo("Qwen/Qwen3-4B-GGUF").is_ok());
        assert!(validate_repo("not-a-repo").is_err());
        assert!(validate_repo("../evil").is_err());
    }

    #[test]
    fn timestamps_are_available() {
        assert!(now_ms() > 0);
    }
}
