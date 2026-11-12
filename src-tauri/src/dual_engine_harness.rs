//! End-to-end harness for the dual-engine supervisor.
//!
//! This test deliberately uses Tauri's mock runtime and exercises the real
//! supervisor, llama runner, process-group shutdown, log forwarding, telemetry
//! polling and managed paths. A tiny dependency-free server is compiled into a
//! disposable `bin/llama-server` so no real model, GPU or network service is
//! required.

use std::{
    fs,
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use tauri::{
    test::{mock_app, MockRuntime},
    Listener, Manager,
};

use crate::{
    engine::{EngineManager, EngineSnapshot, EngineStatus},
    engines::{self, set_test_bin_dir, EngineKind, EngineSupervisor},
    settings::AppSettings,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mock_runtime_switches_engine_and_streams_logs_and_telemetry() {
    let root = std::env::temp_dir().join(format!(
        "janas-dual-engine-harness-{}",
        uuid::Uuid::new_v4()
    ));
    let bin = root.join("bin");
    fs::create_dir_all(&bin).expect("create harness bin directory");

    let cleanup = Cleanup {
        root: root.clone(),
        pid: Arc::new(AtomicU32::new(0)),
    };

    let fake_server = build_fake_server(&bin);
    let janas_server = bin.join("janas-server");
    fs::copy(&fake_server, &janas_server).expect("install fake janas-server");
    set_executable(&janas_server);
    set_test_bin_dir(bin.clone());

    let llama_model = root.join("harness-model.gguf");
    fs::write(&llama_model, b"GGUF-harness").expect("write fake llama model");
    let janas_model = root.join("harness-model.jns");
    fs::write(&janas_model, b"JNS-harness").expect("write fake Janas model");

    let mut settings = AppSettings::default();
    settings.host = "127.0.0.1".into();
    settings.port = free_port();
    settings.model_path = janas_model.to_string_lossy().to_string();
    settings.llama_model_path = llama_model.to_string_lossy().to_string();
    settings.llama_port = free_port();
    while settings.llama_port == settings.port {
        settings.llama_port = free_port();
    }
    settings.llama_gpu_layers = 0;
    settings.llama_ctx_size = 2_048;
    settings.llama_flash_attn = "auto".into();
    settings.llama_kv_cache_k = "f16".into();
    settings.llama_kv_cache_v = "f16".into();
    settings.llama_extra_args.clear();

    let app = mock_app();
    let handle = app.handle().clone();

    let logs = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let logs_for_listener = logs.clone();
    let _log_listener = app.listen("engine://log", move |event| {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(event.payload()) {
            let stream = value
                .get("stream")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown");
            let line = value
                .get("line")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            logs_for_listener
                .lock()
                .expect("log capture lock")
                .push((stream.to_string(), line.to_string()));
        }
    });

    let telemetry = Arc::new(Mutex::new(Vec::<(u64, u64, f64)>::new()));
    let telemetry_for_listener = telemetry.clone();
    let _telemetry_listener = app.listen("engine://telemetry", move |event| {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(event.payload()) {
            telemetry_for_listener
                .lock()
                .expect("telemetry capture lock")
                .push((
                    value
                        .get("completion_tokens_total")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0),
                    value
                        .get("prompt_tokens_total")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0),
                    value
                        .get("kv_cache_usage")
                        .and_then(|value| value.as_f64())
                        .unwrap_or(0.0),
                ));
        }
    });

    let active = Arc::new(Mutex::new(Vec::<Option<u64>>::new()));
    let active_for_listener = active.clone();
    let _active_listener = app.listen("engine://active", move |event| {
        let payload = serde_json::from_str::<Option<serde_json::Value>>(event.payload())
            .ok()
            .flatten();
        let pid = payload
            .as_ref()
            .and_then(|value| value.get("pid"))
            .and_then(|value| value.as_u64());
        active_for_listener
            .lock()
            .expect("active capture lock")
            .push(pid);
    });

    let manager = Arc::new(EngineManager::new(settings.port));
    app.manage(manager.clone());
    let supervisor = Arc::new(EngineSupervisor::<MockRuntime>::new_mock(manager));
    app.manage(supervisor.clone());

    let first = supervisor
        .switch(&handle, &settings, Some(EngineKind::Llama))
        .await
        .expect("start fake llama-server");
    let first_pid = first.pid.expect("first engine pid");
    cleanup.pid.store(first_pid, Ordering::SeqCst);
    assert!(matches!(
        first.status,
        EngineStatus::LoadingModel | EngineStatus::Ready
    ));

    let ready = wait_until_ready(&supervisor).await;
    assert_eq!(ready.status, EngineStatus::Ready);
    assert_eq!(ready.pid, Some(first_pid));
    wait_for_log(&logs, "HARNESS_FAKE_SERVER_READY").await;
    wait_for_telemetry(&telemetry).await;

    // Switch the same supervisor to Janas. This must stop llama.cpp first, then
    // probe and launch the managed janas-server, with readiness owned by the
    // existing EngineManager health watcher.
    let janas = supervisor
        .switch(&handle, &settings, Some(EngineKind::Janas))
        .await
        .expect("switch to fake janas-server");
    let janas_pid = janas.pid.expect("Janas engine pid");
    cleanup.pid.store(janas_pid, Ordering::SeqCst);
    assert!(
        !engines::pid_alive(first_pid),
        "llama.cpp should be stopped before Janas starts"
    );
    let janas_ready = wait_until_ready(&supervisor).await;
    assert_eq!(janas_ready.status, EngineStatus::Ready);
    let janas_active = supervisor.active_info().await.expect("Janas active info");
    assert_eq!(janas_active.kind, EngineKind::Janas);
    assert_eq!(janas_active.pid, janas_pid);

    let second = supervisor
        .switch(&handle, &settings, Some(EngineKind::Llama))
        .await
        .expect("switch back to fake llama-server");
    let second_pid = second.pid.expect("second llama engine pid");
    cleanup.pid.store(second_pid, Ordering::SeqCst);
    assert_ne!(first_pid, second_pid, "restart should create a new process");
    assert_ne!(janas_pid, second_pid, "the Janas process must not be reused");
    wait_until_ready(&supervisor).await;

    wait_for_log(&logs, "HARNESS_FAKE_SERVER_STARTED").await;

    let active_snapshot = active.lock().expect("active capture lock").clone();
    assert!(
        active_snapshot.contains(&Some(first_pid as u64)),
        "active event should report the llama PID: {active_snapshot:?}"
    );
    assert!(
        active_snapshot.contains(&Some(janas_pid as u64)),
        "active event should report the Janas PID: {active_snapshot:?}"
    );
    assert!(
        active_snapshot.contains(&Some(second_pid as u64)),
        "active event should report the second llama PID: {active_snapshot:?}"
    );

    supervisor
        .switch(&handle, &settings, None)
        .await
        .expect("final stop");
    cleanup.pid.store(0, Ordering::SeqCst);
    assert!(!engines::pid_alive(second_pid));
}

fn set_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("make fake server executable");
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    listener.local_addr().expect("ephemeral address").port()
}

fn build_fake_server(bin: &Path) -> PathBuf {
    let output = bin.join("llama-server");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("harness")
        .join("fake_llama_server.rs");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let status = Command::new(rustc)
        .arg("--edition=2021")
        .arg("-O")
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .status()
        .expect("run rustc for fake llama-server");
    assert!(status.success(), "fake llama-server did not compile");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&output, fs::Permissions::from_mode(0o755))
            .expect("make fake llama-server executable");
    }
    output
}

async fn wait_until_ready(
    supervisor: &EngineSupervisor<MockRuntime>,
) -> EngineSnapshot {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let snapshot = supervisor.snapshot().await;
        if snapshot.status == EngineStatus::Ready {
            return snapshot;
        }
        if snapshot.status == EngineStatus::Disconnected {
            panic!("engine failed before becoming ready: {}", snapshot.detail);
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for engine readiness: {snapshot:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_log(logs: &Arc<Mutex<Vec<(String, String)>>>, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let found = logs
            .lock()
            .expect("log capture lock")
            .iter()
            .any(|(_, line)| line.contains(needle));
        if found {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "timed out waiting for log containing {needle:?}; captured: {:?}",
                logs.lock().expect("log capture lock")
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_telemetry(telemetry: &Arc<Mutex<Vec<(u64, u64, f64)>>>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let found = telemetry
            .lock()
            .expect("telemetry capture lock")
            .iter()
            .any(|(completion, prompt, kv)| {
                *completion == 42 && *prompt == 7 && (*kv - 0.25).abs() < 0.001
            });
        if found {
            return;
        }
        if Instant::now() >= deadline {
            panic!(
                "timed out waiting for telemetry; captured: {:?}",
                telemetry.lock().expect("telemetry capture lock")
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

struct Cleanup {
    root: PathBuf,
    pid: Arc<AtomicU32>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let pid = self.pid.load(Ordering::SeqCst);
        if pid > 1 {
            engines::signal_group(pid, libc::SIGTERM);
            for _ in 0..20 {
                if !engines::pid_alive(pid) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if engines::pid_alive(pid) {
                engines::signal_group(pid, libc::SIGKILL);
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
