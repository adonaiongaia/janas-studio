//! Janas adapter.
//!
//! The original `EngineManager` still owns Janas's process, health promotion,
//! log parsing and SSE proxy. This runner gives that manager the same
//! `EngineRunner` surface as llama.cpp so both can live behind one supervisor.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::{
    engine::EngineManager,
    error::{AppError, AppResult},
    settings::{self, AppSettings},
    util::now_ms,
};

use super::{
    advertises, combine_warnings, managed_binary_path, pid_alive, process_counters, run_cli_probe,
    write_pidfile, ActiveEngineInfo, EngineKind, EngineLaunch, EngineProbe, EngineRunner,
    EngineSupervisor, EngineTelemetry, ProbeOutput,
};

pub struct JanasRunner {
    engine: Arc<EngineManager>,
}

impl JanasRunner {
    pub fn new(engine: Arc<EngineManager>) -> Self {
        Self { engine }
    }

    fn binary_candidate<R: Runtime>(app: &AppHandle<R>, settings: &AppSettings) -> AppResult<PathBuf> {
        let managed = managed_binary_path(app, EngineKind::Janas)?;
        if managed.is_file() {
            return Ok(managed);
        }
        settings::resolve_binary(&settings.engine_path).ok_or_else(|| AppError::BinaryNotFound {
            name: settings.engine_path.clone(),
        })
    }

    async fn probe_full<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        settings: &AppSettings,
    ) -> AppResult<(ProbeOutput, JanasLayout, EngineProbe)> {
        let path = Self::binary_candidate(app, settings)?;
        let managed_bin = super::managed_bin_dir(app)?;
        let output = run_cli_probe(&path, Some(&managed_bin)).await?;
        let (layout, mut warnings) = janas_layout(&output, settings);
        if output.version_line.is_none() {
            warnings.push(
                "Janas did not report a version; the launcher will use the native flag layout and keep unknown options out of the command."
                    .into(),
            );
        }
        let probe = EngineProbe {
            version: output.version_line.clone(),
            build: output.build,
            adapter: layout.adapter.clone(),
            warning: combine_warnings(None, warnings),
        };
        Ok((output, layout, probe))
    }

    fn effective_settings<R: Runtime>(
        app: &AppHandle<R>,
        settings: &AppSettings,
        layout: &JanasLayout,
    ) -> AppSettings {
        let mut effective = settings.clone();
        if let Ok(managed) = managed_binary_path(app, EngineKind::Janas) {
            if managed.is_file() {
                effective.engine_path = managed.to_string_lossy().to_string();
            }
        }

        let Ok(mut extra) = shell_words::split(&settings.extra_args) else {
            // Let EngineManager return its normal, more precise parse error.
            return effective;
        };

        if layout.ctx_flag == "--ctx-size"
            && !extra.iter().any(|argument| argument == "--ctx-size")
        {
            extra.push("--ctx-size".into());
            extra.push(settings.ctx_size.clamp(512, 1_048_576).to_string());
        }
        if layout.ssd_streaming
            && settings.ssd_streaming
            && !extra.iter().any(|argument| argument == "--ssd-streaming")
        {
            extra.push("--ssd-streaming".into());
        }
        if layout.ssd_cache {
            let cache = settings.ssd_cache_dir.trim();
            if !cache.is_empty()
                && !extra
                    .iter()
                    .any(|argument| argument == "--ssd-cache-dir")
            {
                extra.push("--ssd-cache-dir".into());
                extra.push(
                    settings::expand_home(cache)
                        .to_string_lossy()
                        .to_string(),
                );
            }
        }
        effective.extra_args = shell_words::join(&extra);
        effective
    }
}

#[derive(Debug, Clone)]
struct JanasLayout {
    adapter: String,
    ctx_flag: &'static str,
    ssd_streaming: bool,
    ssd_cache: bool,
}

fn janas_layout(output: &ProbeOutput, settings: &AppSettings) -> (JanasLayout, Vec<String>) {
    let help = output.help_text.as_str();
    let mut warnings = Vec::new();
    let ctx_flag = if advertises(help, "--ctx-size") {
        "--ctx-size"
    } else {
        "--ctx"
    };
    let ssd_streaming = advertises(help, "--ssd-streaming");
    let ssd_cache = advertises(help, "--ssd-cache-dir");

    if settings.ssd_streaming && !ssd_streaming {
        warnings.push(
            "This Janas build does not advertise `--ssd-streaming`; the SSD streaming toggle will be ignored."
                .into(),
        );
    }
    if !settings.ssd_cache_dir.trim().is_empty() && !ssd_cache {
        warnings.push(
            "This Janas build does not advertise `--ssd-cache-dir`; the cache directory will be ignored."
                .into(),
        );
    }
    if !help.trim().is_empty() && !advertises(help, "--cache") {
        warnings.push(
            "This Janas build did not advertise `--cache`; the expert-cache size setting may be ignored."
                .into(),
        );
    }

    (
        JanasLayout {
            adapter: "janas-server/native".into(),
            ctx_flag,
            ssd_streaming,
            ssd_cache,
        },
        warnings,
    )
}

#[async_trait]
impl<R: Runtime> EngineRunner<R> for JanasRunner {
    async fn probe(
        &self,
        app: &AppHandle<R>,
        settings: &AppSettings,
    ) -> AppResult<EngineProbe> {
        Ok(self.probe_full(app, settings).await?.2)
    }

    async fn spawn_process(
        &self,
        app: &AppHandle<R>,
        settings: &AppSettings,
    ) -> AppResult<EngineLaunch> {
        let (_output, layout, probe) = self.probe_full(app, settings).await?;
        let effective = Self::effective_settings(app, settings, &layout);
        let context_window = effective.ctx_size.clamp(512, 1_048_576);
        let engine = self.engine.clone();
        let app_for_start = app.clone();
        let snapshot = tokio::task::spawn_blocking(move || engine.start(&app_for_start, &effective))
            .await
            .map_err(|err| {
                AppError::message(format!("The Janas start task could not complete: {err}"))
            })??;

        let pid = snapshot.pid.ok_or_else(|| {
            AppError::message(
                "janas-server reported a start but did not expose a process id. Check the engine log.",
            )
        })?;

        let launch = EngineLaunch {
            kind: EngineKind::Janas,
            pid,
            host: settings.connect_host().to_string(),
            port: snapshot.port,
            model: settings.model_path.trim().to_string(),
            context_window,
            started_at_ms: snapshot.started_at_ms.unwrap_or_else(now_ms),
            probe,
            metrics_enabled: false,
            exit_rx: None,
        };
        write_pidfile(app, &launch);
        Ok(launch)
    }

    async fn kill_process(&self, app: &AppHandle<R>, _launch: &EngineLaunch) -> AppResult<()> {
        let engine = self.engine.clone();
        let app = app.clone();
        tokio::task::spawn_blocking(move || engine.stop(&app))
            .await
            .map_err(|err| {
                AppError::message(format!("The Janas stop task could not complete: {err}"))
            })?
    }

    async fn monitor_telemetry(
        &self,
        app: &AppHandle<R>,
        launch: EngineLaunch,
    ) -> AppResult<()> {
        let mut previous: Option<(Instant, u64, u64)> = None;
        loop {
            if !pid_alive(launch.pid) {
                if let Some(supervisor) = supervisor_handle(app) {
                    let _ = supervisor.clear_if_pid(app, launch.pid, None).await;
                }
                return Ok(());
            }

            let now = Instant::now();
            let (read_bytes, read_ops, rss_bytes) = process_counters(launch.pid);
            let (read_bytes_per_sec, read_ops_per_sec) =
                if let Some((at, last_read, last_ops)) = previous {
                    let seconds = now.duration_since(at).as_secs_f64().max(0.001);
                    (
                        read_bytes.saturating_sub(last_read) as f64 / seconds,
                        read_ops.saturating_sub(last_ops) as f64 / seconds,
                    )
                } else {
                    (0.0, 0.0)
                };
            previous = Some((now, read_bytes, read_ops));

            let _ = app.emit(
                "engine://telemetry",
                EngineTelemetry {
                    engine: EngineKind::Janas,
                    pid: Some(launch.pid),
                    tokens_per_second: 0.0,
                    prompt_tokens_total: 0,
                    completion_tokens_total: 0,
                    kv_cache_usage: 0.0,
                    read_bytes_per_sec: read_bytes_per_sec.round() as u64,
                    read_ops_per_sec: read_ops_per_sec.round() as u64,
                    rss_bytes,
                    sampled_at_ms: now_ms(),
                },
            );

            tokio::time::sleep(Duration::from_millis(1_500)).await;
        }
    }
}

fn supervisor_handle<R: Runtime>(app: &AppHandle<R>) -> Option<Arc<EngineSupervisor<R>>> {
    app.try_state::<Arc<EngineSupervisor<R>>>()
        .map(|state| state.inner().clone())
}

// Keep the public type reachable from this module's docs and tests.
#[allow(dead_code)]
fn active_from_probe(launch: &EngineLaunch) -> ActiveEngineInfo {
    ActiveEngineInfo::from(launch)
}
