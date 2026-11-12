//! llama.cpp runner and version-adaptive CLI adapter.
//!
//! The release archive is installed into the managed bin directory. Some Linux
//! builds link against `libllama.so` / `libggml*.so`; those companions are
//! installed beside the binary and `LD_LIBRARY_PATH` is scoped to the child.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::process::Command;

use crate::{
    error::{AppError, AppResult},
    settings::{self, AppSettings},
    util::now_ms,
};

use super::{
    advertises, apply_library_path, combine_warnings, managed_bin_dir, managed_binary_path,
    pid_alive, probe_spawn_error, process_counters, run_cli_probe, signal_group,
    spawn_log_reader, wait_for_exit, write_pidfile, EngineKind, EngineLaunch,
    EngineProbe, EngineRunner, EngineSupervisor, EngineTelemetry, ProbeOutput,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashSyntax {
    /// This build has no `--flash-attn` flag.
    None,
    /// Old builds accept a bare `--flash-attn` switch.
    Bare,
    /// Current builds accept `--flash-attn on|off|auto`.
    Choice,
}

#[derive(Debug, Clone)]
pub struct LlamaLayout {
    pub adapter: String,
    pub model_flag: &'static str,
    pub ctx_flag: Option<&'static str>,
    pub ngl_flag: Option<&'static str>,
    pub flash: FlashSyntax,
    pub kv_cache: bool,
    pub metrics: bool,
    pub jinja: bool,
    pub top_k: bool,
    pub alias: bool,
    /// The flag this build uses for a multimodal projector, if it advertises
    /// vision support.
    pub mmproj_flag: Option<&'static str>,
    /// This build advertises the PR #30966 MoE cache pipeline: host experts,
    /// a device-side cache, and CPU execution for the cache misses.
    pub moe_cache_cpu_share: bool,
}

pub struct LlamaRunner;

impl LlamaRunner {
    pub fn new() -> Self {
        Self
    }

    fn binary_path<R: Runtime>(app: &AppHandle<R>) -> AppResult<PathBuf> {
        let path = managed_binary_path(app, EngineKind::Llama)?;
        if !path.is_file() {
            return Err(AppError::message(format!(
                "llama-server is not installed at {}. Open Settings → llama.cpp sidecar, paste a llama.cpp release archive URL, and install it. Binaries live only in the managed bin folder."
            , path.display())));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .map_err(|err| super::io_error("Could not inspect llama-server", &path, err))?
                .permissions()
                .mode();
            if mode & 0o111 == 0 {
                return Err(AppError::message(format!(
                    "{} is not executable. Reinstall it from Settings or run `chmod 755 {}`.",
                    path.display(),
                    path.display()
                )));
            }
        }
        Ok(path)
    }

    async fn probe_full<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        settings: &AppSettings,
    ) -> AppResult<(ProbeOutput, LlamaLayout, EngineProbe)> {
        let path = Self::binary_path(app)?;
        let bin = managed_bin_dir(app)?;
        let output = run_cli_probe(&path, Some(&bin)).await?;
        let (layout, mut warnings) = layout_for(&output, settings);
        if output.version_line.is_none() {
            warnings.push(
                "The binary did not report a version line; the launch flags were inferred from its help text."
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

    fn validate_port(settings: &AppSettings) -> AppResult<()> {
        let host = bind_host(settings);
        let port = settings.llama_port;
        if port == 0 {
            return Err(AppError::message(
                "The llama.cpp port must be between 1 and 65535.",
            ));
        }
        let listener = std::net::TcpListener::bind((host.as_str(), port)).map_err(|err| {
            AppError::message(format!(
                "Port {port} on {host} is not available ({err}). Stop the process using it or choose another llama.cpp port."
            ))
        })?;
        drop(listener);
        Ok(())
    }
}

#[async_trait]
impl<R: Runtime> EngineRunner<R> for LlamaRunner {
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
        Self::validate_port(settings)?;
        let path = Self::binary_path(app)?;
        let bin = managed_bin_dir(app)?;
        let (_output, layout, mut probe) = self.probe_full(app, settings).await?;
        let (args, build_warnings) = build_args(settings, &layout)?;
        let context_window = if layout.ctx_flag.is_some() {
            settings.llama_ctx_size.clamp(512, 1_048_576)
        } else {
            // Without a context flag the binary keeps its own default, which
            // is the only safe value to report to the request budget.
            16_384
        };
        probe.warning = combine_warnings(probe.warning.take(), build_warnings);
        let bind_host = bind_host(settings);
        let connect_host = settings.connect_host().to_string();

        let mut command = Command::new(&path);
        command
            .args(&args)
            .current_dir(&bin)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);
        apply_library_path(&mut command, Some(&bin));
        #[cfg(unix)]
        command.process_group(0);

        let mut child = command
            .spawn()
            .map_err(|err| probe_spawn_error(&path, Some(&bin), err))?;
        let pid = child.id().ok_or_else(|| {
            AppError::message("llama-server started but did not expose a process id.")
        })?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (exit_tx, exit_rx) = tokio::sync::watch::channel(None);

        let launch = EngineLaunch {
            kind: EngineKind::Llama,
            pid,
            host: connect_host.clone(),
            port: settings.llama_port,
            model: settings::expand_home(settings.llama_model_path.trim())
                .to_string_lossy()
                .to_string(),
            context_window,
            started_at_ms: now_ms(),
            probe,
            metrics_enabled: layout.metrics,
            exit_rx: Some(exit_rx),
        };
        write_pidfile(app, &launch);

        if let Some(stdout) = stdout {
            spawn_log_reader(app.clone(), stdout, "stdout");
        }
        if let Some(stderr) = stderr {
            spawn_log_reader(app.clone(), stderr, "stderr");
        }

        let app_for_reaper = app.clone();
        let pid_for_reaper = pid;
        tauri::async_runtime::spawn(async move {
            let status = child.wait().await;
            let code = status.ok().and_then(|status| status.code());
            let _ = exit_tx.send(Some(code.unwrap_or(-1)));
            super::remove_pidfile(&app_for_reaper);
            if let Some(supervisor) = supervisor_handle(&app_for_reaper) {
                let _ = supervisor
                    .clear_if_pid(&app_for_reaper, pid_for_reaper, Some(code.unwrap_or(-1)))
                    .await;
            }
        });

        spawn_health_probe(
            app.clone(),
            pid,
            settings.connect_host().to_string(),
            settings.llama_port,
        );

        let _ = bind_host;
        Ok(launch)
    }

    async fn kill_process(&self, app: &AppHandle<R>, launch: &EngineLaunch) -> AppResult<()> {
        if !pid_alive(launch.pid) {
            super::remove_pidfile(app);
            return Ok(());
        }

        signal_group(launch.pid, libc::SIGTERM);
        let stopped = match &launch.exit_rx {
            Some(rx) => wait_for_exit(rx, Duration::from_secs(6)).await,
            None => false,
        };
        if !stopped {
            signal_group(launch.pid, libc::SIGKILL);
            if let Some(rx) = &launch.exit_rx {
                let _ = wait_for_exit(rx, Duration::from_secs(2)).await;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }

        if pid_alive(launch.pid) {
            return Err(AppError::message(format!(
                "llama-server (pid {}) did not exit after SIGTERM and SIGKILL. Check for a stuck child process before starting another engine.",
                launch.pid
            )));
        }
        super::remove_pidfile(app);
        Ok(())
    }

    async fn monitor_telemetry(
        &self,
        app: &AppHandle<R>,
        launch: EngineLaunch,
    ) -> AppResult<()> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let metrics_url = format!("http://{}:{}/metrics", launch.host, launch.port);
        let mut previous: Option<(Instant, u64, u64, u64)> = None;

        loop {
            if !pid_alive(launch.pid) {
                return Ok(());
            }
            let now = Instant::now();
            let (completion, prompt, kv) = if launch.metrics_enabled {
                fetch_metrics(&client, &metrics_url).await
            } else {
                (0, 0, 0.0)
            };
            let (read_bytes, read_ops, rss_bytes) = process_counters(launch.pid);
            let (tokens_per_second, read_bytes_per_sec, read_ops_per_sec) =
                if let Some((at, last_completion, last_read, last_ops)) = previous {
                    let seconds = now.duration_since(at).as_secs_f64().max(0.001);
                    (
                        completion.saturating_sub(last_completion) as f64 / seconds,
                        read_bytes.saturating_sub(last_read) as f64 / seconds,
                        read_ops.saturating_sub(last_ops) as f64 / seconds,
                    )
                } else {
                    (0.0, 0.0, 0.0)
                };
            previous = Some((now, completion, read_bytes, read_ops));

            let payload = EngineTelemetry {
                engine: EngineKind::Llama,
                pid: Some(launch.pid),
                tokens_per_second: round2(tokens_per_second),
                prompt_tokens_total: prompt,
                completion_tokens_total: completion,
                kv_cache_usage: round4(kv),
                read_bytes_per_sec: read_bytes_per_sec.round() as u64,
                read_ops_per_sec: read_ops_per_sec.round() as u64,
                rss_bytes,
                sampled_at_ms: now_ms(),
            };
            let _ = app.emit("engine://telemetry", payload);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

fn bind_host(settings: &AppSettings) -> String {
    let host = settings.host.trim();
    if host.is_empty() {
        "127.0.0.1".into()
    } else {
        host.to_string()
    }
}

fn layout_for(output: &ProbeOutput, settings: &AppSettings) -> (LlamaLayout, Vec<String>) {
    let help = output.help_text.as_str();
    let help_missing = help.trim().is_empty();
    let build = output.build;
    let legacy = build.map(|value| value < 3_000).unwrap_or(false);
    let mut warnings = Vec::new();

    if help_missing {
        warnings.push(
            "`--help` returned no text; the adapter fell back to the standard llama-server flag layout."
                .into(),
        );
    }
    match build {
        None => warnings.push(
            "The llama-server build number could not be parsed; optional flags are only used when `--help` advertises them."
                .into(),
        ),
        Some(value) if !(3_000..=20_000).contains(&value) => warnings.push(format!(
            "llama-server build b{value} is outside the tested range (b3000–b20000). The adapter is using the closest known layout."
        )),
        _ => {}
    }

    let mmproj_flag = if advertises(help, "--mmproj") {
        Some("--mmproj")
    } else if advertises(help, "-mm") {
        Some("-mm")
    } else if help_missing {
        Some("--mmproj")
    } else {
        None
    };
    if mmproj_flag.is_none() && !settings.llama_mmproj_path.trim().is_empty() {
        warnings.push(
            "This llama-server build does not advertise `--mmproj`; the selected vision projector will not be loaded, so image input will stay unavailable. Update llama.cpp or remove the projector path."
                .into(),
        );
    }

    if legacy {
        warnings.push(
            "This is a legacy llama-server build. Context and GPU flags use the short forms, and KV-cache/telemetry flags are disabled."
                .into(),
        );
        return (
            LlamaLayout {
                adapter: format!("llama-server/classic{}", build_label(build)),
                model_flag: "--model",
                ctx_flag: Some("-c"),
                ngl_flag: Some("-ngl"),
                flash: FlashSyntax::None,
                kv_cache: false,
                metrics: false,
                jinja: false,
                top_k: false,
                alias: false,
                mmproj_flag,
                moe_cache_cpu_share: false,
            },
            warnings,
        );
    }

    let model_flag = if advertises(help, "--model") || help_missing {
        "--model"
    } else if advertises(help, "-m") {
        "-m"
    } else {
        warnings.push(
            "Neither `--model` nor `-m` was found in the help text; using `--model` and falling back to the server's error if the build differs."
                .into(),
        );
        "--model"
    };
    let ctx_flag = if advertises(help, "--ctx-size") {
        Some("--ctx-size")
    } else if advertises(help, "-c") {
        Some("-c")
    } else if help_missing {
        Some("--ctx-size")
    } else {
        warnings.push(
            "No context-size flag was advertised; the context value is left to the binary's default."
                .into(),
        );
        None
    };
    let ngl_flag = if advertises(help, "--n-gpu-layers") {
        Some("--n-gpu-layers")
    } else if advertises(help, "--gpu-layers") {
        Some("--gpu-layers")
    } else if advertises(help, "-ngl") {
        Some("-ngl")
    } else if help_missing {
        Some("--n-gpu-layers")
    } else {
        warnings.push(
            "No GPU-layer flag was advertised; GPU offload is disabled for this build."
                .into(),
        );
        None
    };
    let flash = if advertises(help, "--flash-attn") || advertises(help, "-fa") {
        let tail = help
            .find("--flash-attn")
            .map(|index| &help[index..])
            .unwrap_or_default();
        let choices = tail.contains("on|off|auto")
            || tail.contains("on\\|off\\|auto")
            || tail.contains("[on")
            || (build.map(|value| value >= 6_000).unwrap_or(false) && help_missing);
        if choices {
            FlashSyntax::Choice
        } else {
            FlashSyntax::Bare
        }
    } else if help_missing && build.map(|value| value >= 6_000).unwrap_or(false) {
        FlashSyntax::Choice
    } else {
        FlashSyntax::None
    };
    let kv_cache =
        advertises(help, "--cache-type-k") && advertises(help, "--cache-type-v");
    let metrics = advertises(help, "--metrics");
    let jinja = advertises(help, "--jinja");
    let top_k = advertises(help, "--top-k");
    let alias = advertises(help, "--alias");
    let moe_cache_cpu_share = advertises(help, "--moe-cache-mib")
        && advertises(help, "--moe-cache-cpu-share");

    if !kv_cache
        && (settings::normalize_kv_cache(&settings.llama_kv_cache_k) != "f16"
            || settings::normalize_kv_cache(&settings.llama_kv_cache_v) != "f16")
    {
        warnings.push(
            "This build does not advertise KV-cache type flags; the requested cache quantization will be ignored."
                .into(),
        );
    }
    if flash == FlashSyntax::None && !settings.llama_flash_attn.eq_ignore_ascii_case("auto") {
        warnings.push(
            "This build does not advertise Flash Attention; the selected value will be ignored."
                .into(),
        );
    }
    if !metrics {
        warnings.push(
            "This build does not advertise `--metrics`; token telemetry will use process counters only."
                .into(),
        );
    }
    if !jinja {
        warnings.push(
            "This build does not advertise `--jinja`; OpenAI tool calling in the studio may be unavailable."
                .into(),
        );
    }
    if !moe_cache_cpu_share && settings.llama_direct_cpu_moe {
        warnings.push(
            "This llama-server build does not advertise `--moe-cache-mib` and `--moe-cache-cpu-share` (llama.cpp b12191 or newer). Direct CPU MoE Compute was ignored."
                .into(),
        );
    }

    let tier = if build.map(|value| value < 6_000).unwrap_or(false) {
        "transitional"
    } else if build.is_some() {
        "modern"
    } else {
        "inferred"
    };
    (
        LlamaLayout {
            adapter: format!("llama-server/{tier}{}", build_label(build)),
            model_flag,
            ctx_flag,
            ngl_flag,
            flash,
            kv_cache,
            metrics,
            jinja,
            top_k,
            alias,
            mmproj_flag,
            moe_cache_cpu_share,
        },
        warnings,
    )
}

fn build_label(build: Option<u64>) -> String {
    build.map(|value| format!(" (b{value})")).unwrap_or_default()
}

fn build_args(
    settings: &AppSettings,
    layout: &LlamaLayout,
) -> AppResult<(Vec<String>, Vec<String>)> {
    let raw_model = settings.llama_model_path.trim();
    if raw_model.is_empty() {
        return Err(AppError::message(
            "No llama.cpp model is selected. Choose a .gguf file in Engine Settings or Settings → llama.cpp.",
        ));
    }
    let model = settings::expand_home(raw_model);
    if !model.is_file() {
        return Err(AppError::message(format!(
            "The llama.cpp model does not exist: {}",
            model.display()
        )));
    }
    if !model
        .extension()
        .map(|extension| extension.eq_ignore_ascii_case("gguf"))
        .unwrap_or(false)
    {
        return Err(AppError::message(
            "llama-server needs a .gguf model. Convert or download a GGUF file first.",
        ));
    }

    let raw_mmproj = settings.llama_mmproj_path.trim();
    let mmproj = if raw_mmproj.is_empty() {
        None
    } else {
        let path = settings::expand_home(raw_mmproj);
        if !path.is_file() {
            return Err(AppError::message(format!(
                "The llama.cpp vision projector does not exist: {}. Choose the matching mmproj .gguf in Settings → llama.cpp.",
                path.display()
            )));
        }
        if !path
            .extension()
            .map(|extension| extension.eq_ignore_ascii_case("gguf"))
            .unwrap_or(false)
        {
            return Err(AppError::message(
                "The llama.cpp vision projector must be a .gguf file. Choose the mmproj file that matches the selected vision model.",
            ));
        }
        Some(path)
    };

    let mut warnings = Vec::new();
    let model_text = model.to_string_lossy().to_string();
    let host = bind_host(settings);
    let mut args = vec![
        layout.model_flag.to_string(),
        model_text.clone(),
        "--host".into(),
        host,
        "--port".into(),
        settings.llama_port.to_string(),
    ];

    if let Some(path) = mmproj.as_ref() {
        if let Some(flag) = layout.mmproj_flag {
            args.push(flag.into());
            args.push(path.to_string_lossy().to_string());
        }
    }

    let extra_args = if settings.llama_extra_args.trim().is_empty() {
        Vec::new()
    } else {
        shell_words::split(&settings.llama_extra_args).map_err(|err| {
            AppError::message(format!(
                "Invalid llama.cpp extra arguments: {err}. Use shell-style quoting."
            ))
        })?
    };
    let extra_has = |flag: &str| {
        extra_args
            .iter()
            .any(|argument| argument == flag || argument.starts_with(&format!("{flag}=")))
    };

    if let Some(flag) = layout.ctx_flag {
        let ctx = settings.llama_ctx_size.clamp(512, 1_048_576);
        args.push(flag.into());
        args.push(ctx.to_string());
    }
    if let Some(flag) = layout.ngl_flag {
        let layers = settings.llama_gpu_layers.clamp(0, 999);
        args.push(flag.into());
        args.push(layers.to_string());
    }

    let flash = settings::normalize_flash_attn(&settings.llama_flash_attn);
    let cache_k = settings::normalize_kv_cache(&settings.llama_kv_cache_k);
    let cache_v = settings::normalize_kv_cache(&settings.llama_kv_cache_v);
    let quantized_v = cache_v != "f16";

    if quantized_v && flash == "off" {
        return Err(AppError::message(
            "A quantized V cache needs Flash Attention. Set Flash Attention to `on` or `auto`, or leave the V cache at f16.",
        ));
    }
    if quantized_v && layout.flash == FlashSyntax::None {
        return Err(AppError::message(
            "This llama-server build cannot enable Flash Attention, so a quantized V cache cannot be used. Set the V cache type to f16.",
        ));
    }

    match layout.flash {
        FlashSyntax::Choice => {
            args.push("--flash-attn".into());
            args.push(flash.clone());
        }
        FlashSyntax::Bare => {
            if flash == "on" || quantized_v {
                args.push("--flash-attn".into());
            }
        }
        FlashSyntax::None => {
            if flash != "auto" {
                warnings.push(
                    "Flash Attention was requested but is not supported by this build; it was ignored."
                        .into(),
                );
            }
        }
    }

    if layout.kv_cache {
        if cache_k != "f16" {
            args.push("--cache-type-k".into());
            args.push(cache_k);
        }
        if cache_v != "f16" {
            args.push("--cache-type-v".into());
            args.push(cache_v);
        }
    }

    if settings.llama_direct_cpu_moe && layout.moe_cache_cpu_share {
        let host_experts = [
            "--cpu-moe",
            "-cmoe",
            "--n-cpu-moe",
            "-ncmoe",
            "--override-tensor",
            "-ot",
        ]
        .iter()
        .any(|flag| extra_has(flag));
        if !host_experts {
            args.push("--cpu-moe".into());
        }
        if !extra_has("--moe-cache-mib") {
            args.push("--moe-cache-mib".into());
            args.push("2048".into());
        }
        if !extra_has("--moe-cache-cpu-share") {
            args.push("--moe-cache-cpu-share".into());
            args.push("1.0".into());
        }
        let threads = settings.llama_cpu_threads.min(256);
        if threads > 0 && !extra_has("--threads") && !extra_has("-t") {
            args.push("--threads".into());
            args.push(threads.to_string());
        }
    }

    if layout.top_k {
        args.push("--top-k".into());
        args.push(settings.top_k.clamp(1, 100).to_string());
    }

    if layout.jinja {
        args.push("--jinja".into());
    }
    if layout.metrics {
        args.push("--metrics".into());
    }
    if layout.alias {
        if let Some(stem) = Path::new(&model_text).file_stem() {
            args.push("--alias".into());
            args.push(stem.to_string_lossy().to_string());
        }
    }

    args.extend(extra_args);

    Ok((args, warnings))
}

fn spawn_health_probe<R: Runtime>(app: AppHandle<R>, pid: u32, host: String, port: u16) {
    tauri::async_runtime::spawn(async move {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let url = format!("http://{host}:{port}/health");
        let deadline = Instant::now() + Duration::from_secs(180);

        while Instant::now() < deadline {
            if !pid_alive(pid) {
                return;
            }
            if let Ok(response) = client
                .get(&url)
                .timeout(Duration::from_secs(2))
                .send()
                .await
            {
                if response.status().is_success() {
                    if let Some(supervisor) = supervisor_handle(&app) {
                        supervisor.mark_llama_ready(&app, pid).await;
                    }
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(700)).await;
        }

        if pid_alive(pid) {
            if let Some(supervisor) = supervisor_handle(&app) {
                supervisor
                    .mark_llama_failed(
                        &app,
                        pid,
                        "llama-server did not answer /health within 180 seconds. Check the engine log for a model or GPU error.",
                    )
                    .await;
            }
            signal_group(pid, libc::SIGTERM);
            tokio::time::sleep(Duration::from_millis(500)).await;
            signal_group(pid, libc::SIGKILL);
        }
    });
}

fn supervisor_handle<R: Runtime>(
    app: &AppHandle<R>,
) -> Option<std::sync::Arc<EngineSupervisor<R>>> {
    app.try_state::<std::sync::Arc<EngineSupervisor<R>>>()
        .map(|state| state.inner().clone())
}

async fn fetch_metrics(client: &reqwest::Client, url: &str) -> (u64, u64, f64) {
    let Ok(response) = client.get(url).timeout(Duration::from_secs(2)).send().await else {
        return (0, 0, 0.0);
    };
    if !response.status().is_success() {
        return (0, 0, 0.0);
    }
    let Ok(text) = response.text().await else {
        return (0, 0, 0.0);
    };
    parse_metrics(&text)
}

fn parse_metrics(text: &str) -> (u64, u64, f64) {
    let mut completion = 0u64;
    let mut prompt = 0u64;
    let mut kv_usage = 0.0f64;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        let Some(value) = parts.next().and_then(|value| value.parse::<f64>().ok()) else {
            continue;
        };
        if name.contains("tokens_predicted_total") && !name.contains("seconds") {
            completion = value.max(0.0) as u64;
        } else if name.contains("prompt_tokens_total") && !name.contains("seconds") {
            prompt = value.max(0.0) as u64;
        } else if name.contains("kv_cache_usage_ratio") {
            kv_usage = value.clamp(0.0, 1.0);
        }
    }
    (completion, prompt, kv_usage)
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_build_from_version_line() {
        assert_eq!(
            crate::engines::parse_build("version: 6640 (b4f4d4e4c)"),
            Some(6640)
        );
        assert_eq!(
            crate::engines::parse_build("llama-server b8123\nbuilt with clang"),
            Some(8123)
        );
    }

    #[test]
    fn detects_flag_tokens_without_substring_false_positives() {
        let help = "-c, --ctx-size N  context size\n-ngl, --n-gpu-layers N\n-fa, --flash-attn [on|off|auto]\n-ctk, --cache-type-k TYPE\n--metrics\n--jinja\n--top-k N\n--alias STR";
        assert!(advertises(help, "--ctx-size"));
        assert!(advertises(help, "-c"));
        assert!(!advertises("-cache", "-c"));
    }

    #[test]
    fn modern_layout_uses_choice_flash_and_kv_flags() {
        let output = ProbeOutput {
            version_line: Some("version: 6640 (hash)".into()),
            build: Some(6640),
            help_text: "--model\n--ctx-size\n--n-gpu-layers\n--flash-attn [on|off|auto]\n--cache-type-k\n--cache-type-v\n--metrics\n--jinja\n--top-k\n--alias\n--moe-cache-mib N\n--moe-cache-cpu-share F\n-mm, --mmproj FILE".into(),
        };
        let (layout, _) = layout_for(&output, &AppSettings::default());
        assert_eq!(layout.flash, FlashSyntax::Choice);
        assert!(layout.kv_cache);
        assert!(layout.metrics);
        assert!(layout.moe_cache_cpu_share);
        assert_eq!(layout.mmproj_flag, Some("--mmproj"));
    }

    #[test]
    fn quantized_v_requires_flash_attention() {
        let path = std::env::temp_dir().join(format!(
            "janas-llama-args-{}.gguf",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, b"GGUF-test").expect("write test model");
        let mut settings = AppSettings::default();
        settings.llama_model_path = path.to_string_lossy().to_string();
        settings.llama_kv_cache_v = "q8_0".into();
        settings.llama_flash_attn = "off".into();
        let layout = LlamaLayout {
            adapter: "test".into(),
            model_flag: "--model",
            ctx_flag: Some("--ctx-size"),
            ngl_flag: Some("--n-gpu-layers"),
            flash: FlashSyntax::Choice,
            kv_cache: true,
            metrics: true,
            jinja: true,
            top_k: true,
            alias: true,
            mmproj_flag: Some("--mmproj"),
            moe_cache_cpu_share: false,
        };
        let result = build_args(&settings, &layout);
        assert!(result.is_err());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn args_include_gpu_context_and_kv_flags() {
        let path = std::env::temp_dir().join(format!(
            "janas-llama-args-{}.gguf",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, b"GGUF-test").expect("write test model");
        let mut settings = AppSettings::default();
        settings.llama_model_path = path.to_string_lossy().to_string();
        settings.llama_gpu_layers = 24;
        settings.llama_ctx_size = 8192;
        settings.llama_flash_attn = "on".into();
        settings.llama_kv_cache_k = "q8_0".into();
        settings.llama_kv_cache_v = "q8_0".into();
        settings.top_k = 40;
        let layout = LlamaLayout {
            adapter: "test".into(),
            model_flag: "--model",
            ctx_flag: Some("--ctx-size"),
            ngl_flag: Some("--n-gpu-layers"),
            flash: FlashSyntax::Choice,
            kv_cache: true,
            metrics: true,
            jinja: true,
            top_k: true,
            alias: true,
            mmproj_flag: Some("--mmproj"),
            moe_cache_cpu_share: false,
        };
        let (args, warnings) = build_args(&settings, &layout).expect("args");
        let joined = args.join(" ");
        assert!(joined.contains("--n-gpu-layers 24"));
        assert!(joined.contains("--ctx-size 8192"));
        assert!(joined.contains("--flash-attn on"));
        assert!(joined.contains("--cache-type-k q8_0"));
        assert!(joined.contains("--cache-type-v q8_0"));
        assert!(joined.contains("--top-k 40"));
        assert!(warnings.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn args_include_vision_projector() {
        let dir = std::env::temp_dir().join(format!(
            "janas-llama-mmproj-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("create test directory");
        let model = dir.join("vision-model.gguf");
        let projector = dir.join("mmproj-model-f16.gguf");
        std::fs::write(&model, b"GGUF-test").expect("write test model");
        std::fs::write(&projector, b"GGUF-mmproj").expect("write test projector");
        let mut settings = AppSettings::default();
        settings.llama_model_path = model.to_string_lossy().to_string();
        settings.llama_mmproj_path = projector.to_string_lossy().to_string();
        let layout = LlamaLayout {
            adapter: "test".into(),
            model_flag: "--model",
            ctx_flag: Some("--ctx-size"),
            ngl_flag: Some("--n-gpu-layers"),
            flash: FlashSyntax::Choice,
            kv_cache: true,
            metrics: true,
            jinja: true,
            top_k: true,
            alias: true,
            mmproj_flag: Some("--mmproj"),
            moe_cache_cpu_share: false,
        };
        let (args, warnings) = build_args(&settings, &layout).expect("args");
        let joined = args.join(" ");
        assert!(joined.contains(&format!("--mmproj {}", projector.to_string_lossy())));
        assert!(warnings.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn direct_cpu_moe_adds_cache_pipeline_and_threads() {
        let path = std::env::temp_dir().join(format!(
            "janas-llama-direct-moe-{}.gguf",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, b"GGUF-test").expect("write test model");
        let mut settings = AppSettings::default();
        settings.llama_model_path = path.to_string_lossy().to_string();
        settings.llama_direct_cpu_moe = true;
        settings.llama_cpu_threads = 6;
        let layout = LlamaLayout {
            adapter: "test".into(),
            model_flag: "--model",
            ctx_flag: Some("--ctx-size"),
            ngl_flag: Some("--n-gpu-layers"),
            flash: FlashSyntax::Choice,
            kv_cache: true,
            metrics: true,
            jinja: true,
            top_k: true,
            alias: true,
            mmproj_flag: Some("--mmproj"),
            moe_cache_cpu_share: true,
        };
        let (args, warnings) = build_args(&settings, &layout).expect("args");
        let joined = args.join(" ");
        assert!(joined.contains("--cpu-moe"));
        assert!(joined.contains("--moe-cache-mib 2048"));
        assert!(joined.contains("--moe-cache-cpu-share 1.0"));
        assert!(joined.contains("--threads 6"));
        assert!(warnings.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn direct_cpu_moe_respects_extra_arg_overrides() {
        let path = std::env::temp_dir().join(format!(
            "janas-llama-direct-moe-overrides-{}.gguf",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, b"GGUF-test").expect("write test model");
        let mut settings = AppSettings::default();
        settings.llama_model_path = path.to_string_lossy().to_string();
        settings.llama_direct_cpu_moe = true;
        settings.llama_cpu_threads = 6;
        settings.llama_extra_args =
            "--n-cpu-moe 12 --moe-cache-mib 4096 --threads 3".into();
        let layout = LlamaLayout {
            adapter: "test".into(),
            model_flag: "--model",
            ctx_flag: Some("--ctx-size"),
            ngl_flag: Some("--n-gpu-layers"),
            flash: FlashSyntax::Choice,
            kv_cache: true,
            metrics: true,
            jinja: true,
            top_k: true,
            alias: true,
            mmproj_flag: Some("--mmproj"),
            moe_cache_cpu_share: true,
        };
        let (args, _) = build_args(&settings, &layout).expect("args");
        let count = |flag: &str| args.iter().filter(|argument| *argument == flag).count();
        assert_eq!(count("--cpu-moe"), 0);
        assert_eq!(count("--moe-cache-mib"), 1);
        assert_eq!(count("--moe-cache-cpu-share"), 1);
        assert_eq!(count("--threads"), 1);
        let joined = args.join(" ");
        assert!(joined.contains("--moe-cache-mib 4096"));
        assert!(joined.contains("--threads 3"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn direct_cpu_moe_is_ignored_when_the_build_lacks_the_pipeline() {
        let path = std::env::temp_dir().join(format!(
            "janas-llama-direct-moe-unsupported-{}.gguf",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, b"GGUF-test").expect("write test model");
        let mut settings = AppSettings::default();
        settings.llama_model_path = path.to_string_lossy().to_string();
        settings.llama_direct_cpu_moe = true;
        settings.llama_cpu_threads = 6;
        let layout = LlamaLayout {
            adapter: "test".into(),
            model_flag: "--model",
            ctx_flag: Some("--ctx-size"),
            ngl_flag: Some("--n-gpu-layers"),
            flash: FlashSyntax::Choice,
            kv_cache: true,
            metrics: true,
            jinja: true,
            top_k: true,
            alias: true,
            mmproj_flag: Some("--mmproj"),
            moe_cache_cpu_share: false,
        };
        let (args, _) = build_args(&settings, &layout).expect("args");
        let joined = args.join(" ");
        assert!(!joined.contains("--moe-cache"));
        assert!(!joined.contains("--cpu-moe"));
        assert!(!joined.contains("--threads"));
        let _ = std::fs::remove_file(path);
    }
}
