#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod converter;
mod capabilities;
mod engine;
mod engines;
#[cfg(test)]
mod dual_engine_harness;
mod error;
mod gguf;
mod huggingface;
mod mcp;
mod memory;
mod provision;
mod settings;
mod system;
mod util;

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, State};

use converter::{ConversionStarted, ConvertRequest, ConverterManager};
use memory::{MemoryFact, MemoryStore};
use provision::Provisioner;
use engine::{ChatRequest, EngineManager, EngineSnapshot, EngineStatus};
use engines::binary::BinaryManager;
use engines::{ActiveEngineInfo, EngineKind, EngineSupervisor};
use error::{AppError, AppResult};
use huggingface::{
    DownloadManager, DownloadRequest, DownloadStarted, HfModel, HfQuant, SearchRequest,
};
use settings::{AppSettings, BinaryProbe, LocalModel, SettingsStore};

#[tauri::command]
fn get_settings(store: State<'_, Arc<SettingsStore>>) -> AppSettings {
    store.get()
}

#[tauri::command]
fn save_settings(
    store: State<'_, Arc<SettingsStore>>,
    next: AppSettings,
) -> AppResult<AppSettings> {
    store.save(next)
}

#[tauri::command(rename_all = "snake_case")]
fn probe_binary(path: String) -> BinaryProbe {
    settings::probe_binary(&path)
}

#[tauri::command]
fn scan_local_models(store: State<'_, Arc<SettingsStore>>) -> Vec<LocalModel> {
    let current = store.get();
    settings::scan_models(&current.models_dir, Some(&current.model_path), 4)
}

#[tauri::command]
fn inspect_gguf_model(path: String) -> AppResult<gguf::GgufModelInfo> {
    gguf::inspect(&path)
}

#[tauri::command]
async fn get_engine_status(
    supervisor: State<'_, Arc<EngineSupervisor>>,
) -> AppResult<EngineSnapshot> {
    Ok(supervisor.inner().clone().snapshot().await)
}

/// Start the engine the user last selected. The supervisor owns the choice, so
/// the sidebar's single Start/Stop button works for both engines.
#[tauri::command]
async fn start_engine(
    app: AppHandle,
    supervisor: State<'_, Arc<EngineSupervisor>>,
    store: State<'_, Arc<SettingsStore>>,
) -> AppResult<EngineSnapshot> {
    let supervisor = supervisor.inner().clone();
    let settings = store.get();
    let target = supervisor.preferred_kind();
    supervisor.switch(&app, &settings, Some(target)).await
}

#[tauri::command]
async fn stop_engine(
    app: AppHandle,
    supervisor: State<'_, Arc<EngineSupervisor>>,
    store: State<'_, Arc<SettingsStore>>,
) -> AppResult<()> {
    let supervisor = supervisor.inner().clone();
    let settings = store.get();
    supervisor
        .switch(&app, &settings, None)
        .await
        .map(|_| ())
}

#[tauri::command]
async fn switch_engine(
    app: AppHandle,
    supervisor: State<'_, Arc<EngineSupervisor>>,
    store: State<'_, Arc<SettingsStore>>,
    engine: String,
) -> AppResult<EngineSnapshot> {
    let target = match engine.trim().to_ascii_lowercase().as_str() {
        "janas" | "janas-server" => Some(EngineKind::Janas),
        "llama" | "llama.cpp" | "llamacpp" => Some(EngineKind::Llama),
        "none" | "stop" => None,
        other => {
            return Err(AppError::message(format!(
                "Unknown engine \"{other}\". Use \"janas\", \"llama\", or \"none\"."
            )))
        }
    };
    let supervisor = supervisor.inner().clone();
    let settings = store.get();
    supervisor.switch(&app, &settings, target).await
}

#[tauri::command]
async fn get_active_engine(
    supervisor: State<'_, Arc<EngineSupervisor>>,
) -> AppResult<Option<ActiveEngineInfo>> {
    Ok(supervisor.inner().clone().active_info().await)
}

#[tauri::command]
async fn stream_chat(
    app: AppHandle,
    engine: State<'_, Arc<EngineManager>>,
    supervisor: State<'_, Arc<EngineSupervisor>>,
    store: State<'_, Arc<SettingsStore>>,
    memory: State<'_, Arc<MemoryStore>>,
    request: ChatRequest,
) -> AppResult<()> {
    let engine = engine.inner().clone();
    let memory = memory.inner().clone();
    let active = supervisor
        .active_info()
        .await
        .ok_or(AppError::EngineNotReady)?;
    let status = supervisor.snapshot().await;
    if status.status != EngineStatus::Ready {
        return Err(AppError::EngineNotReady);
    }

    // The existing SSE proxy is provider-neutral. Point it at whichever engine
    // the supervisor has active instead of always using Janas's settings port.
    let mut config = store.get();
    config.host = active.host;
    config.port = active.port;
    // The proxy also uses this value to decide whether MCP tool schemas fit.
    // Give it the running child's window, not a stale saved preference.
    config.ctx_size = active.context_window;
    config.llama_ctx_size = active.context_window;
    config.model_path = active.model;
    engine.stream_chat(&app, &config, &memory, request).await
}

fn publish_memory(app: &AppHandle, memory: &MemoryStore) {
    let _ = app.emit("memory://changed", memory.list());
}

#[tauri::command]
fn memory_list(memory: State<'_, Arc<MemoryStore>>) -> Vec<MemoryFact> {
    memory.list()
}

/// The backend handler for the `save_to_memory_bank` chat tool. The streaming
/// loop calls the same `MemoryStore::save` method directly for model-initiated
/// saves; this command covers the UI and future callers.
#[tauri::command]
fn save_to_memory_bank(
    app: AppHandle,
    memory: State<'_, Arc<MemoryStore>>,
    fact: String,
) -> AppResult<MemoryFact> {
    let outcome = memory.save(&fact, "user")?;
    if outcome.created {
        publish_memory(&app, &memory);
    }
    Ok(outcome.fact)
}

#[tauri::command]
fn memory_update(
    app: AppHandle,
    memory: State<'_, Arc<MemoryStore>>,
    id: String,
    text: String,
) -> AppResult<MemoryFact> {
    let fact = memory.update(&id, &text)?;
    publish_memory(&app, &memory);
    Ok(fact)
}

#[tauri::command]
fn memory_delete(
    app: AppHandle,
    memory: State<'_, Arc<MemoryStore>>,
    id: String,
) -> AppResult<bool> {
    let removed = memory.delete(&id)?;
    if removed {
        publish_memory(&app, &memory);
    }
    Ok(removed)
}

#[tauri::command]
fn memory_clear(app: AppHandle, memory: State<'_, Arc<MemoryStore>>) -> AppResult<()> {
    memory.clear()?;
    publish_memory(&app, &memory);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
fn cancel_chat(engine: State<'_, Arc<EngineManager>>, request_id: String) {
    engine.cancel_chat(&request_id);
}

#[tauri::command]
fn start_conversion(
    app: AppHandle,
    manager: State<'_, Arc<ConverterManager>>,
    store: State<'_, Arc<SettingsStore>>,
    request: ConvertRequest,
) -> AppResult<ConversionStarted> {
    manager.start(&app, &store.get(), request)
}

#[tauri::command]
fn cancel_conversion(
    app: AppHandle,
    manager: State<'_, Arc<ConverterManager>>,
) -> AppResult<()> {
    manager.cancel(&app)
}

#[tauri::command]
async fn search_models(request: SearchRequest) -> AppResult<Vec<HfModel>> {
    huggingface::search(request).await
}

#[tauri::command]
async fn list_model_quants(repo: String) -> AppResult<Vec<HfQuant>> {
    huggingface::list_quants(repo).await
}

#[tauri::command]
fn start_download(
    app: AppHandle,
    manager: State<'_, Arc<DownloadManager>>,
    store: State<'_, Arc<SettingsStore>>,
    request: DownloadRequest,
) -> AppResult<DownloadStarted> {
    let manager = manager.inner().clone();
    manager.start(app, &store.get(), request)
}

#[tauri::command(rename_all = "snake_case")]
fn cancel_download(manager: State<'_, Arc<DownloadManager>>, id: String) -> bool {
    manager.cancel(&id)
}

#[tauri::command(rename_all = "snake_case")]
fn pause_download(manager: State<'_, Arc<DownloadManager>>, id: String) -> bool {
    manager.pause(&id)
}

#[tauri::command(rename_all = "snake_case")]
fn resume_download(manager: State<'_, Arc<DownloadManager>>, id: String) -> bool {
    manager.resume(&id)
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            if let Ok(resources) = app.path().resource_dir() {
                settings::set_bundled_dir(resources);
            }

            let store = Arc::new(SettingsStore::load(config_dir, data_dir.clone())?);
            let port = store.get().port;
            let memory = MemoryStore::load(data_dir.join("memory.json"));

            engines::cleanup_stale_processes(app.handle())?;
            let engine = Arc::new(EngineManager::new(port));
            let supervisor = Arc::new(EngineSupervisor::new(engine.clone()));
            let binaries = Arc::new(BinaryManager::new());
            let provisioner = Arc::new(Provisioner::new());

            app.manage(store);
            app.manage(Arc::new(memory));
            app.manage(engine);
            app.manage(supervisor.clone());
            app.manage(binaries.clone());
            app.manage(provisioner.clone());
            app.manage(Arc::new(ConverterManager::new()));
            app.manage(Arc::new(DownloadManager::new()));

            // Provisioning is intentionally silent and off the UI thread. The
            // report is available immediately via get_engine_setup_status and
            // is replayed over setup://status as the task advances.
            let app_for_setup = app.handle().clone();
            let provisioner_for_setup = provisioner.clone();
            let manager_for_setup = binaries.clone();
            let supervisor_for_setup = supervisor.clone();
            tauri::async_runtime::spawn(async move {
                provisioner_for_setup
                    .run(
                        &app_for_setup,
                        manager_for_setup,
                        supervisor_for_setup,
                        false,
                    )
                    .await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            probe_binary,
            scan_local_models,
            inspect_gguf_model,
            get_engine_status,
            start_engine,
            stop_engine,
            switch_engine,
            get_active_engine,
            capabilities::get_backend_capabilities,
            stream_chat,
            cancel_chat,
            memory_list,
            save_to_memory_bank,
            memory_update,
            memory_delete,
            memory_clear,
            start_conversion,
            cancel_conversion,
            search_models,
            list_model_quants,
            start_download,
            cancel_download,
            pause_download,
            resume_download,
            system::get_telemetry,
            system::benchmark_storage,
            system::ingest_project,
            system::web_search,
            system::open_external,
            system::transcribe_audio,
            system::library_models,
            mcp::mcp_probe,
            engines::binary::download_and_extract_binary,
            engines::binary::rollback_binary,
            engines::binary::probe_managed_binary,
            provision::get_engine_setup_status,
            provision::rerun_engine_setup,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Janas Studio");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(supervisor) = app_handle.try_state::<Arc<EngineSupervisor>>() {
                supervisor.inner().shutdown_blocking(app_handle);
            }
        }
    });
}
