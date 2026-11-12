use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read},
    net::TcpListener,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::{
    error::{AppError, AppResult},
    mcp::{McpActivityEvent, McpRuntime, McpServerSpec},
    memory::{tool_result_json, MemoryStore},
    settings::{self, AppSettings},
    util::{now_ms, truncate_line},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineStatus {
    Disconnected,
    Indexing,
    LoadingModel,
    Ready,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineSnapshot {
    pub status: EngineStatus,
    pub detail: String,
    pub pid: Option<u32>,
    pub model: Option<String>,
    pub port: u16,
    pub started_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineLog {
    pub stream: String,
    pub line: String,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatDeltaEvent {
    pub request_id: String,
    pub delta: String,
    pub reasoning: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatDoneEvent {
    pub request_id: String,
    pub finish_reason: String,
    pub usage: Option<serde_json::Value>,
    pub elapsed_ms: u64,
    pub first_token_ms: Option<u64>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatErrorEvent {
    pub request_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatNoticeEvent {
    pub request_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatRequest {
    pub request_id: String,
    pub messages: Vec<WireMessage>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    pub temperature: f32,
    pub top_p: f32,
    #[serde(default)]
    pub top_k: Option<u32>,
    #[serde(default)]
    pub min_p: Option<f32>,
    #[serde(default)]
    pub repeat_penalty: Option<f32>,
    #[serde(default)]
    pub structured: bool,
    #[serde(default)]
    pub thinking: Option<bool>,
    #[serde(default)]
    pub images: Vec<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerSpec>,
}

pub struct EngineManager {
    child: Mutex<Option<Child>>,
    snapshot: Mutex<EngineSnapshot>,
    cancelled: Mutex<HashMap<String, Arc<AtomicBool>>>,
    http: reqwest::Client,
}

impl EngineManager {
    pub fn new(port: u16) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            child: Mutex::new(None),
            snapshot: Mutex::new(EngineSnapshot {
                status: EngineStatus::Disconnected,
                detail: "Engine has not been started".into(),
                pid: None,
                model: None,
                port,
                started_at_ms: None,
            }),
            cancelled: Mutex::new(HashMap::new()),
            http,
        }
    }

    pub fn snapshot(&self) -> EngineSnapshot {
        self.snapshot
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or(EngineSnapshot {
                status: EngineStatus::Disconnected,
                detail: "Engine state unavailable".into(),
                pid: None,
                model: None,
                port: 8080,
                started_at_ms: None,
            })
    }

    pub fn is_running(&self) -> bool {
        let Ok(mut guard) = self.child.lock() else {
            return false;
        };
        let Some(child) = guard.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(Some(_)) => {
                *guard = None;
                false
            }
            Ok(None) => true,
            Err(_) => false,
        }
    }

    fn publish<R: Runtime>(&self, app: &AppHandle<R>) {
        let _ = app.emit("engine://status", self.snapshot());
    }

    pub fn set_state<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        status: EngineStatus,
        detail: impl Into<String>,
    ) {
        let Ok(mut guard) = self.snapshot.lock() else {
            return;
        };
        guard.status = status;
        guard.detail = detail.into();
        if status == EngineStatus::Disconnected {
            guard.pid = None;
            guard.started_at_ms = None;
        }
        drop(guard);
        self.publish(app);
    }

    /// Promote the state only forward (Indexing → Loading → Ready) so a noisy
    /// server log line cannot overwrite a later stage.
    fn note_status<R: Runtime>(&self, app: &AppHandle<R>, status: EngineStatus, detail: &str) {
        let current = self.snapshot().status;
        if rank(status) > rank(current) && status != EngineStatus::Ready {
            self.set_state(app, status, detail);
        }
    }

    fn mark_started<R: Runtime>(&self, app: &AppHandle<R>, pid: u32, model: &str, port: u16) {
        if let Ok(mut guard) = self.snapshot.lock() {
            guard.pid = Some(pid);
            guard.model = Some(model.to_string());
            guard.port = port;
            guard.started_at_ms = Some(now_ms());
        }
        self.publish(app);
    }

    pub fn start<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        settings: &AppSettings,
    ) -> AppResult<EngineSnapshot> {
        if self.is_running() {
            return Ok(self.snapshot());
        }

        let binary = settings::resolve_binary(&settings.engine_path)
            .ok_or_else(|| AppError::BinaryNotFound {
                name: settings.engine_path.clone(),
            })?;

        let model_path = settings.model_path.trim();
        if model_path.is_empty() {
            return Err(AppError::message(
                "No model selected. Choose a .jns model in Settings (or convert a GGUF first).",
            ));
        }
        let model = Path::new(model_path);
        if !model.is_file() {
            return Err(AppError::message(format!(
                "The model file does not exist: {model_path}"
            )));
        }
        if !model
            .extension()
            .map(|ext| ext.eq_ignore_ascii_case("jns"))
            .unwrap_or(false)
        {
            return Err(AppError::message(
                "Janas needs a converted .jns model, not a .gguf file. Convert it in the Compiler tab first.",
            ));
        }

        let port = settings.port.clamp(1, 65535);
        if TcpListener::bind((settings.connect_host(), port)).is_err() {
            return Err(AppError::message(format!(
                "Port {port} is already in use. Stop the other process or pick another port in Settings."
            )));
        }

        let mut command = Command::new(&binary);
        command
            .arg(model)
            .arg("--host")
            .arg(&settings.host)
            .arg("--port")
            .arg(port.to_string())
            .arg("--name")
            .arg(
                model
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
                    .unwrap_or_else(|| "janas".into()),
            );

        if !settings.api_key.trim().is_empty() {
            command.arg("--api-key").arg(settings.api_key.trim());
        }
        apply_engine_settings(&mut command, settings);

        if !settings.extra_args.trim().is_empty() {
            let extra = shell_words::split(&settings.extra_args)
                .map_err(|err| AppError::message(format!("Invalid extra arguments: {err}")))?;
            command.args(extra);
        }

        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }

        let mut child = command.spawn().map_err(|err| {
            AppError::message(format!(
                "Could not start {}: {err}. Check the binary path in Settings.",
                binary.display()
            ))
        })?;

        let pid = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        *self
            .child
            .lock()
            .map_err(|_| AppError::message("Engine state is locked."))?
            = Some(child);

        self.mark_started(app, pid, model_path, port);
        self.set_state(
            app,
            EngineStatus::Indexing,
            format!("Starting {}. Reading the model index…", binary.display()),
        );

        if let Some(stdout) = stdout {
            spawn_log_reader(app.clone(), self_arc(app), stdout, "stdout");
        }
        if let Some(stderr) = stderr {
            spawn_log_reader(app.clone(), self_arc(app), stderr, "stderr");
        }
        spawn_health_probe(app.clone(), settings.connect_host().to_string(), port);
        spawn_exit_watcher(app.clone(), self_arc(app));

        Ok(self.snapshot())
    }

    pub fn stop<R: Runtime>(&self, app: &AppHandle<R>) -> AppResult<()> {
        {
            let Ok(mut guard) = self.child.lock() else {
                return Err(AppError::message("Engine state is locked."));
            };
            if let Some(child) = guard.as_mut() {
                #[cfg(unix)]
                unsafe {
                    libc::kill(child.id() as i32, libc::SIGTERM);
                }
                #[cfg(not(unix))]
                {
                    let _ = child.kill();
                }

                let deadline = Instant::now() + Duration::from_secs(6);
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        Ok(None) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                        Err(_) => break,
                    }
                }
            }
            *guard = None;
        }

        self.set_state(app, EngineStatus::Disconnected, "Engine stopped");
        Ok(())
    }

    pub fn cancel_chat(&self, request_id: &str) {
        if let Ok(mut guard) = self.cancelled.lock() {
            if guard.len() > 64 {
                guard.clear();
            }
            guard
                .entry(request_id.to_string())
                .or_insert_with(|| Arc::new(AtomicBool::new(false)))
                .store(true, Ordering::Relaxed);
        }
    }

    /// Register the shared flag before opening the stream, so a cancel that
    /// arrives during a long prompt read is still observed promptly.
    fn register_chat(&self, request_id: &str) -> Arc<AtomicBool> {
        if let Ok(mut guard) = self.cancelled.lock() {
            if let Some(flag) = guard.get(request_id) {
                return flag.clone();
            }
            let flag = Arc::new(AtomicBool::new(false));
            guard.insert(request_id.to_string(), flag.clone());
            flag
        } else {
            Arc::new(AtomicBool::new(false))
        }
    }

    fn clear_cancelled(&self, request_id: &str) {
        if let Ok(mut guard) = self.cancelled.lock() {
            guard.remove(request_id);
        }
    }

    pub async fn stream_chat(
        &self,
        app: &AppHandle,
        settings: &AppSettings,
        memory: &MemoryStore,
        request: ChatRequest,
    ) -> AppResult<()> {
        let mut messages = Vec::with_capacity(request.messages.len() + 1);
        if let Some(system) = request
            .system_prompt
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            messages.push(serde_json::json!({ "role": "system", "content": system }));
        }
        for message in &request.messages {
            messages.push(serde_json::json!({
                "role": message.role,
                "content": message.content,
            }));
        }
        // Vision input rides on the final user turn as OpenAI-style content
        // parts. Engines without a vision projector reject it, which surfaces
        // as a normal chat error instead of silently dropping the images.
        if !request.images.is_empty() {
            if let Some(last) = messages
                .iter_mut()
                .rev()
                .find(|message| message["role"] == "user")
            {
                let text = last["content"].as_str().unwrap_or_default().to_string();
                let mut parts = vec![serde_json::json!({ "type": "text", "text": text })];
                for image in &request.images {
                    parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": { "url": image }
                    }));
                }
                last["content"] = serde_json::Value::Array(parts);
            }
        }

        let url = format!("{}/v1/chat/completions", settings.base_url());
        let cancel = self.register_chat(&request.request_id);
        let started = Instant::now();
        let mut first_token_ms: Option<u64> = None;
        let mut usage: Option<serde_json::Value> = None;
        let mut finish_reason = String::new();
        let mut cancelled = false;
        let mut saw_output = false;
        let mut malformed_total = 0usize;
        let mut last_problem: Option<String> = None;
        let mut tool_rounds = 0usize;
        let mut context_retry_done = false;

        // MCP servers are connected once for the turn. Their tools are added
        // to the model's tool list, and the same live stdio sessions execute
        // any calls the model makes while this response is streaming.
        let mut mcp_runtime = if request.mcp_servers.is_empty() {
            None
        } else {
            let specs = request.mcp_servers.clone();
            match tauri::async_runtime::spawn_blocking(move || McpRuntime::connect(&specs)).await {
                Ok(runtime) => Some(runtime),
                Err(err) => {
                    let _ = app.emit(
                        "chat://notice",
                        ChatNoticeEvent {
                            request_id: request.request_id.clone(),
                            message: format!("MCP tools could not be prepared: {err}"),
                        },
                    );
                    None
                }
            }
        };
        if let Some(runtime) = mcp_runtime.as_ref() {
            if !runtime.warnings().is_empty() {
                let _ = app.emit(
                    "chat://notice",
                    ChatNoticeEvent {
                        request_id: request.request_id.clone(),
                        message: format!(
                            "MCP server warning: {}",
                            truncate_line(&runtime.warnings().join(" "), 900)
                        ),
                    },
                );
            }
        }
        let mut mcp_tools = mcp_runtime
            .as_ref()
            .map(McpRuntime::tool_definitions)
            .unwrap_or_default();

        // Tool definitions are part of the prompt even though they are not
        // messages. A large MCP server can advertise more schema than the
        // whole context window, so drop its tools before the first request
        // rather than waiting for the engine to reject the turn.
        let context_window = settings.ctx_size.max(512) as usize;
        let prompt_budget = request_prompt_budget(context_window, request.max_tokens);
        let message_cost: usize = messages.iter().map(retry_message_cost).sum();
        let tools_cost = tool_schema_tokens(&mcp_tools);
        if !mcp_tools.is_empty() && message_cost + tools_cost > prompt_budget {
            let omitted = mcp_tools.len();
            mcp_tools.clear();
            emit_chat_notice(
                app,
                &request.request_id,
                format!(
                    "The enabled MCP server advertises {omitted} tools whose definitions need about {tools_cost} tokens. They do not fit in this {context_window}-token context, so this turn is running without MCP tools. Disable the server in MCP Hub or use a smaller MCP server."
                ),
            );
        }

        use futures_util::StreamExt;
        let mut ticker = tokio::time::interval(Duration::from_millis(150));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        // Each pass through this loop is one completion request. When the model
        // asks for the memory tool, the pass appends the assistant tool call and
        // its result to the history, then runs another pass so the model can
        // finish its reply. The loop is bounded so a confused model cannot spin
        // forever.
        loop {
            let mut body = serde_json::json!({
                "model": settings.model_display_name().unwrap_or_else(|| "janas".into()),
                "messages": &messages,
                "temperature": request.temperature.clamp(0.0, 2.0),
                "top_p": request.top_p.clamp(0.001, 1.0),
                "stream": true,
                "stream_options": { "include_usage": true },
                "tools": tools_schema(&mcp_tools),
                "tool_choice": "auto",
                "parallel_tool_calls": false,
            });
            if let Some(top_k) = request.top_k {
                body["top_k"] = serde_json::json!(top_k.clamp(1, 100));
            }
            if let Some(max_tokens) = request.max_tokens.filter(|value| *value > 0) {
                body["max_tokens"] = serde_json::json!(max_tokens);
            }
            if let Some(min_p) = request.min_p.filter(|value| *value > 0.0) {
                body["min_p"] = serde_json::json!(min_p.clamp(0.0, 1.0));
            }
            if let Some(penalty) = request.repeat_penalty.filter(|value| *value > 0.0) {
                body["repeat_penalty"] = serde_json::json!(penalty.clamp(0.1, 4.0));
            }
            if request.structured {
                body["response_format"] = serde_json::json!({ "type": "json_object" });
            }
            if let Some(thinking) = request.thinking {
                body["chat_template_kwargs"] = serde_json::json!({ "enable_thinking": thinking });
            }

            let mut builder = self.http.post(&url).json(&body);
            if !settings.api_key.trim().is_empty() {
                builder = builder.bearer_auth(settings.api_key.trim());
            }

            let response = match builder.send().await {
                Ok(response) => response,
                Err(err) => {
                    emit_chat_error(
                        app,
                        &request.request_id,
                        format!("Could not reach the local inference server: {err}"),
                    );
                    self.clear_cancelled(&request.request_id);
                    return Ok(());
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let text = truncate_line(&body, 800);
                if status == reqwest::StatusCode::BAD_REQUEST && !context_retry_done {
                    if let Some(limit) = context_limit_from_error(&body) {
                        let mut changed = false;
                        if let Some(trimmed) =
                            trim_messages_to_context(&messages, limit, request.max_tokens)
                        {
                            messages = trimmed;
                            changed = true;
                        }
                        let budget = request_prompt_budget(limit as usize, request.max_tokens);
                        let message_cost: usize =
                            messages.iter().map(retry_message_cost).sum();
                        let tools_cost = tool_schema_tokens(&mcp_tools);
                        if !mcp_tools.is_empty() && message_cost + tools_cost > budget {
                            let omitted = mcp_tools.len();
                            mcp_tools.clear();
                            changed = true;
                            emit_chat_notice(
                                app,
                                &request.request_id,
                                format!(
                                    "The engine rejected the request because the {omitted} MCP tool definitions need about {tools_cost} tokens. MCP tools are omitted for this retry so the conversation can continue."
                                ),
                            );
                        }
                        if changed {
                            context_retry_done = true;
                            continue;
                        }
                    }
                }
                emit_chat_error(
                    app,
                    &request.request_id,
                    format!("The inference engine returned HTTP {status}: {text}"),
                );
                self.clear_cancelled(&request.request_id);
                return Ok(());
            }

            let mut stream = response.bytes_stream();
            let mut decoder = SseDecoder::default();
            let mut round = StreamRound::default();
            let mut stream_done = false;

            while !stream_done {
                let chunk = tokio::select! {
                    _ = ticker.tick() => {
                        if cancel.load(Ordering::Relaxed) {
                            cancelled = true;
                            break;
                        }
                        continue;
                    }
                    next = stream.next() => match next {
                        Some(chunk) => chunk,
                        None => break,
                    },
                };
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(err) => {
                        emit_chat_error(
                            app,
                            &request.request_id,
                            format!("The inference stream stopped: {err}"),
                        );
                        self.clear_cancelled(&request.request_id);
                        return Ok(());
                    }
                };

                let batch = drain_payloads(decoder.feed(&chunk), &mut round);
                emit_batch(
                    app,
                    &request.request_id,
                    &batch,
                    &mut saw_output,
                    &mut first_token_ms,
                    started,
                );
                if let Some(message) = batch.error {
                    emit_chat_error(app, &request.request_id, message);
                    self.clear_cancelled(&request.request_id);
                    return Ok(());
                }
                if batch.done {
                    stream_done = true;
                }
            }

            // A server that closes without a blank SSE line still has a final
            // chunk in the decoder. Flush it instead of dropping the last words.
            if !stream_done && !cancelled {
                let batch = drain_payloads(decoder.finish(), &mut round);
                emit_batch(
                    app,
                    &request.request_id,
                    &batch,
                    &mut saw_output,
                    &mut first_token_ms,
                    started,
                );
                if let Some(message) = batch.error {
                    emit_chat_error(app, &request.request_id, message);
                    self.clear_cancelled(&request.request_id);
                    return Ok(());
                }
            }

            if cancelled {
                break;
            }

            usage = round.usage.clone().or(usage);
            if !round.finish_reason.is_empty() {
                finish_reason = round.finish_reason.clone();
            }
            malformed_total += round.malformed;
            if round.last_problem.is_some() {
                last_problem = round.last_problem.clone();
            }

            let calls: Vec<ToolCallDraft> = round
                .tool_calls
                .iter()
                .filter(|call| !call.name.trim().is_empty())
                .cloned()
                .collect();

            if !calls.is_empty() && tool_rounds < MAX_TOOL_ROUNDS {
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
                tool_rounds += 1;
                let tool_run = run_tools(app, memory, mcp_runtime.as_mut(), &calls);
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": round.content,
                    "tool_calls": tool_run.assistant_calls,
                }));
                messages.extend(tool_run.messages);
                finish_reason.clear();
                continue;
            }

            if !calls.is_empty() && tool_rounds >= MAX_TOOL_ROUNDS && !saw_output {
                let note = "_The model reached the tool-round limit before writing a reply._";
                let _ = app.emit(
                    "chat://delta",
                    ChatDeltaEvent {
                        request_id: request.request_id.clone(),
                        delta: note.into(),
                        reasoning: String::new(),
                    },
                );
                saw_output = true;
            }

            if !saw_output && !cancelled {
                let mut notes = Vec::new();
                if malformed_total > 0 {
                    notes.push(format!(
                        "ignored {malformed_total} malformed stream chunk(s)"
                    ));
                }
                if let Some(problem) = last_problem.as_deref() {
                    notes.push(truncate_line(problem, 240));
                }
                let detail = if notes.is_empty() {
                    String::new()
                } else {
                    format!(" The stream reported: {}.", notes.join("; "))
                };
                emit_chat_error(
                    app,
                    &request.request_id,
                    format!("The engine closed the stream without sending any text.{detail}"),
                );
                self.clear_cancelled(&request.request_id);
                return Ok(());
            }

            break;
        }

        self.clear_cancelled(&request.request_id);
        let _ = app.emit(
            "chat://done",
            ChatDoneEvent {
                request_id: request.request_id,
                finish_reason: if cancelled {
                    "cancelled".into()
                } else if finish_reason.is_empty() {
                    "stop".into()
                } else {
                    finish_reason
                },
                usage,
                elapsed_ms: started.elapsed().as_millis() as u64,
                first_token_ms,
                cancelled,
            },
        );
        Ok(())
    }

}

impl Drop for EngineManager {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(child) = guard.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

/// The managed state is an `Arc<EngineManager>`, so worker threads can obtain
/// the same instance from the app handle.
fn self_arc<R: Runtime>(app: &AppHandle<R>) -> Arc<EngineManager> {
    app.state::<Arc<EngineManager>>().inner().clone()
}

fn spawn_log_reader<R: Runtime>(
    app: AppHandle<R>,
    engine: Arc<EngineManager>,
    stream: impl Read + Send + 'static,
    stream_name: &'static str,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else {
                break;
            };
            let line = truncate_line(&line, 4_000);
            let _ = app.emit(
                "engine://log",
                EngineLog {
                    stream: stream_name.into(),
                    line: line.clone(),
                    timestamp_ms: now_ms(),
                },
            );
            if let Some(status) = classify_log_line(&line) {
                engine.note_status(&app, status, line.trim());
            }
        }
    });
}

fn spawn_health_probe<R: Runtime>(app: AppHandle<R>, host: String, port: u16) {
    tauri::async_runtime::spawn(async move {
        let client = match reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .build()
        {
            Ok(client) => client,
            Err(_) => reqwest::Client::new(),
        };
        let url = format!("http://{host}:{port}/health");

        for _ in 0..900 {
            let engine = app.state::<Arc<EngineManager>>().inner().clone();
            if !engine.is_running() {
                return;
            }
            if engine.snapshot().status == EngineStatus::Ready {
                return;
            }
            if let Ok(response) = client
                .get(&url)
                .timeout(Duration::from_secs(2))
                .send()
                .await
            {
                if response.status().is_success() && engine.is_running() {
                    engine.set_state(
                        &app,
                        EngineStatus::Ready,
                        format!("Janas is ready on {host}:{port}"),
                    );
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(1_000)).await;
        }
    });
}

fn spawn_exit_watcher<R: Runtime>(app: AppHandle<R>, engine: Arc<EngineManager>) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(1_200)).await;
            if !engine.is_running() {
                if engine.snapshot().status != EngineStatus::Disconnected {
                    engine.set_state(
                        &app,
                        EngineStatus::Disconnected,
                        "janas-server exited. Check the engine log for details.",
                    );
                    let _ = app.emit("engine://exited", ());
                }
                return;
            }
        }
    });
}

fn emit_chat_error(app: &AppHandle, request_id: &str, message: impl Into<String>) {
    let _ = app.emit(
        "chat://error",
        ChatErrorEvent {
            request_id: request_id.to_string(),
            message: message.into(),
        },
    );
}

/// Translate the studio's engine settings into the flags janas-server
/// documents, plus the JANAS_ATTN/JANAS_KV environment overrides its library
/// reads. Empty or "auto" values are left out so the engine keeps its own
/// defaults; extra_args are appended afterwards and win over these.
fn apply_engine_settings(command: &mut Command, settings: &AppSettings) {
    // The Janas adapter may choose `--ctx-size` when that is what the probed
    // build advertises. In that case do not also pass the legacy `--ctx`.
    let uses_long_ctx = shell_words::split(&settings.extra_args)
        .map(|arguments| arguments.iter().any(|argument| argument == "--ctx-size"))
        .unwrap_or(false);
    if settings.ctx_size >= 512 && !uses_long_ctx {
        command.arg("--ctx").arg(settings.ctx_size.to_string());
    }
    if settings.cache_gib > 0.0 {
        command
            .arg("--cache")
            .arg(format!("{:.1}", settings.cache_gib));
    }

    match settings.mtp_mode.trim().to_ascii_lowercase().as_str() {
        "off" => {
            command.arg("--no-mtp");
        }
        "file" => {
            let path = settings.mtp_path.trim();
            if !path.is_empty() {
                command.arg("--mtp").arg(crate::settings::expand_home(path));
            }
        }
        _ => {}
    }
    let draft = settings.draft_path.trim();
    if !draft.is_empty() {
        command.arg("--draft").arg(crate::settings::expand_home(draft));
    }

    for (flag, value) in [
        ("--prompt-on", &settings.prompt_on),
        ("--reply-on", &settings.reply_on),
        ("--drafts-on", &settings.drafts_on),
        ("--mode", &settings.engine_mode),
    ] {
        let value = value.trim();
        if !value.is_empty() && !value.eq_ignore_ascii_case("auto") {
            command.arg(flag).arg(value);
        }
    }

    let think = settings.think.trim().to_ascii_lowercase();
    if think == "on" || think == "off" {
        command.arg("--think").arg(think);
    }
    if settings.npu_int8 {
        command.arg("--npu-int8");
    }

    // Janas exposes attention and KV-cache precision through its environment
    // rather than command-line flags. Keep the UI values in the engine's own
    // vocabulary: exact is float attention and fast is the 16-bit query path.
    match settings::normalize_janas_attention(&settings.janas_attention).as_str() {
        "exact" => {
            command.env("JANAS_ATTN", "float");
        }
        "fast" => {
            command.env("JANAS_ATTN", "int16");
        }
        _ => {}
    }
    match settings::normalize_janas_kv_cache(&settings.janas_kv_cache).as_str() {
        "8" => {
            command.env("JANAS_KV", "8");
        }
        "16" => {
            command.env("JANAS_KV", "16");
        }
        _ => {}
    }
}

fn rank(status: EngineStatus) -> u8 {
    match status {
        EngineStatus::Disconnected => 0,
        EngineStatus::Indexing => 1,
        EngineStatus::LoadingModel => 2,
        EngineStatus::Ready => 3,
    }
}

fn classify_log_line(line: &str) -> Option<EngineStatus> {
    let lower = line.to_ascii_lowercase();
    if lower.contains("loading")
        || lower.contains("opening")
        || lower.contains("resident")
        || lower.contains("expert cache")
        || lower.contains("reading weights")
    {
        Some(EngineStatus::LoadingModel)
    } else if lower.contains("index") || lower.contains("scanning") || lower.contains("converting") {
        Some(EngineStatus::Indexing)
    } else {
        None
    }
}

const MAX_SSE_BUFFER: usize = 8 * 1024 * 1024;
const MAX_TOOL_CALLS: usize = 8;
const MAX_TOOL_ROUNDS: usize = 6;

#[derive(Debug, Clone, Default)]
struct ToolCallDraft {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, Default)]
struct StreamRound {
    content: String,
    tool_calls: Vec<ToolCallDraft>,
    finish_reason: String,
    usage: Option<serde_json::Value>,
    malformed: usize,
    last_problem: Option<String>,
}

enum PayloadOutcome {
    Continue(Option<(String, String)>),
    Done,
    EngineError(String),
}

#[derive(Default)]
struct PayloadBatch {
    deltas: Vec<(String, String)>,
    done: bool,
    error: Option<String>,
}

enum SseItem {
    Data(String),
    Warning(String),
}

/// A forgiving Server-Sent Events reader. It accepts `data:` lines with LF or
/// CRLF, waits for multi-line JSON to complete, tolerates comments and unknown
/// fields, and also accepts newline-delimited raw JSON from servers that do not
/// implement SSE framing. Nothing here indexes a string by byte offset.
#[derive(Default)]
struct SseDecoder {
    buffer: Vec<u8>,
    data: String,
    warned_overflow: bool,
}

impl SseDecoder {
    fn feed(&mut self, chunk: &[u8]) -> Vec<SseItem> {
        let mut items = Vec::new();
        self.buffer.extend_from_slice(chunk);

        while let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line_bytes: Vec<u8> = self.buffer.drain(..=position).collect();
            let line = String::from_utf8_lossy(&line_bytes);
            self.consume_line(&line, &mut items);
        }

        if self.buffer.len() > MAX_SSE_BUFFER {
            self.buffer.clear();
            self.data.clear();
            if !self.warned_overflow {
                self.warned_overflow = true;
                items.push(SseItem::Warning(
                    "A stream line exceeded the 8 MiB safety limit and was discarded.".into(),
                ));
            }
        }

        items
    }

    fn finish(&mut self) -> Vec<SseItem> {
        let mut items = Vec::new();
        if !self.buffer.is_empty() {
            let line = String::from_utf8_lossy(&self.buffer).to_string();
            self.buffer.clear();
            self.consume_line(&line, &mut items);
        }
        if !self.data.is_empty() {
            items.push(SseItem::Data(std::mem::take(&mut self.data)));
        }
        items
    }

    fn consume_line(&mut self, line: &str, items: &mut Vec<SseItem>) {
        let line = line.trim_end_matches(|character| character == '\r' || character == '\n');

        if line.is_empty() {
            if !self.data.is_empty() {
                items.push(SseItem::Data(std::mem::take(&mut self.data)));
            }
            return;
        }
        if line.starts_with(':') {
            return;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(rest.trim_start());
            let trimmed = self.data.trim();
            if trimmed == "[DONE]" || serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
                items.push(SseItem::Data(std::mem::take(&mut self.data)));
            }
            return;
        }
        if line.starts_with("event:") || line.starts_with("id:") || line.starts_with("retry:") {
            return;
        }

        // Non-SSE servers sometimes send one JSON object per line.
        let trimmed = line.trim();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            if !self.data.is_empty() {
                items.push(SseItem::Data(std::mem::take(&mut self.data)));
            }
            items.push(SseItem::Data(trimmed.to_string()));
        } else if !trimmed.is_empty() {
            items.push(SseItem::Warning(format!(
                "Ignored an unrecognised stream line ({} characters).",
                trimmed.chars().count()
            )));
        }
    }
}

fn drain_payloads(items: Vec<SseItem>, round: &mut StreamRound) -> PayloadBatch {
    let mut batch = PayloadBatch::default();
    for item in items {
        match item {
            SseItem::Warning(warning) => {
                round.malformed += 1;
                round.last_problem = Some(warning);
            }
            SseItem::Data(payload) => match consume_payload(&payload, round) {
                PayloadOutcome::Continue(Some(delta)) => batch.deltas.push(delta),
                PayloadOutcome::Continue(None) => {}
                PayloadOutcome::Done => {
                    batch.done = true;
                    break;
                }
                PayloadOutcome::EngineError(message) => {
                    batch.error = Some(message);
                    break;
                }
            },
        }
    }
    batch
}

fn consume_payload(payload: &str, round: &mut StreamRound) -> PayloadOutcome {
    let trimmed = payload.trim();
    if trimmed.is_empty() {
        return PayloadOutcome::Continue(None);
    }
    if trimmed == "[DONE]" {
        return PayloadOutcome::Done;
    }

    let value: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(value) => value,
        Err(err) => {
            round.malformed += 1;
            round.last_problem = Some(format!("Malformed stream JSON: {err}"));
            return PayloadOutcome::Continue(None);
        }
    };

    if let Some(error) = value.get("error").filter(|error| !error.is_null()) {
        return PayloadOutcome::EngineError(error_text(error));
    }

    if let Some(usage) = value.get("usage").filter(|usage| !usage.is_null()) {
        round.usage = Some(usage.clone());
    }

    let Some(choice) = value
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return PayloadOutcome::Continue(None);
    };

    let mut content = String::new();
    let mut reasoning = String::new();

    if let Some(delta) = choice.get("delta").filter(|delta| delta.is_object()) {
        if let Some(value) = delta.get("content") {
            content = text_from_value(value);
        }
        for key in ["reasoning_content", "reasoning", "reasoning_text"] {
            if let Some(value) = delta.get(key) {
                let text = text_from_value(value);
                if !text.is_empty() {
                    reasoning = text;
                    break;
                }
            }
        }
        collect_tool_calls(delta, round);
    }

    // Some servers put text directly on the choice for the final chunk.
    if content.is_empty() {
        if let Some(value) = choice.get("text") {
            content = text_from_value(value);
        }
    }

    if let Some(reason) = choice
        .get("finish_reason")
        .and_then(serde_json::Value::as_str)
        .filter(|reason| !reason.is_empty())
    {
        round.finish_reason = reason.to_string();
    }

    if !content.is_empty() {
        round.content.push_str(&content);
    }

    if content.is_empty() && reasoning.is_empty() {
        PayloadOutcome::Continue(None)
    } else {
        PayloadOutcome::Continue(Some((content, reasoning)))
    }
}

fn text_from_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| part.get("content").and_then(serde_json::Value::as_str))
            })
            .collect::<Vec<_>>()
            .join(""),
        serde_json::Value::Object(map) => map
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

fn collect_tool_calls(delta: &serde_json::Value, round: &mut StreamRound) {
    if let Some(calls) = delta
        .get("tool_calls")
        .and_then(serde_json::Value::as_array)
    {
        for (position, call) in calls.iter().enumerate().take(MAX_TOOL_CALLS) {
            let index = call
                .get("index")
                .and_then(serde_json::Value::as_u64)
                .map(|index| index as usize)
                .unwrap_or(position);
            merge_tool_call(round, index, call);
        }
    }

    // The legacy single-function shape is still emitted by some models.
    if let Some(function) = delta
        .get("function_call")
        .filter(|function| function.is_object())
    {
        let wrapped = serde_json::json!({
            "id": "",
            "type": "function",
            "function": function,
        });
        merge_tool_call(round, 0, &wrapped);
    }
}

fn merge_tool_call(round: &mut StreamRound, index: usize, call: &serde_json::Value) {
    if index >= MAX_TOOL_CALLS {
        round.malformed += 1;
        round.last_problem = Some("A stream tried to open more than 8 tool calls.".into());
        return;
    }
    while round.tool_calls.len() <= index {
        round.tool_calls.push(ToolCallDraft::default());
    }
    let slot = &mut round.tool_calls[index];

    if let Some(id) = call
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
    {
        if slot.id.is_empty() {
            slot.id = id.to_string();
        }
    }

    if let Some(function) = call.get("function").filter(|value| value.is_object()) {
        if let Some(name) = function
            .get("name")
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.is_empty())
        {
            if slot.name.is_empty() {
                slot.name = name.to_string();
            }
        }
        if let Some(arguments) = function.get("arguments").filter(|value| !value.is_null()) {
            match arguments {
                serde_json::Value::String(text) => slot.arguments.push_str(text),
                other => slot.arguments.push_str(&other.to_string()),
            }
        }
    }
}

const RETRY_CHARS_PER_TOKEN: usize = 3;
const RETRY_MESSAGE_OVERHEAD: usize = 8;
const RETRY_REQUEST_OVERHEAD: usize = 512;

fn request_output_reserve(context_window: usize, requested_max: Option<u32>) -> usize {
    let half = (context_window / 2).max(512);
    let requested = requested_max
        .filter(|value| *value > 0)
        .map(|value| value as usize)
        .unwrap_or_else(|| (context_window / 4).clamp(1_024, 4_096));
    requested.clamp(512, half)
}

fn request_prompt_budget(context_window: usize, requested_max: Option<u32>) -> usize {
    context_window
        .saturating_sub(request_output_reserve(context_window, requested_max))
        .saturating_sub(RETRY_REQUEST_OVERHEAD)
        .max(256)
}

fn tool_schema_tokens(tools: &[serde_json::Value]) -> usize {
    if tools.is_empty() {
        return 0;
    }
    retry_text_tokens(&tools_schema(tools).to_string())
}

/// Conservative token count for the retry path. ASCII text is treated as three
/// characters per token; non-ASCII text is counted per character so CJK prompts
/// are not underestimated.
fn retry_text_tokens(text: &str) -> usize {
    let mut ascii = 0usize;
    let mut wide = 0usize;
    for character in text.chars() {
        if character.is_ascii() {
            ascii += 1;
        } else {
            wide += 1;
        }
    }
    (ascii + 2) / RETRY_CHARS_PER_TOKEN + wide
}

fn retry_message_cost(message: &serde_json::Value) -> usize {
    let content_cost = match message.get("content") {
        Some(serde_json::Value::String(text)) => retry_text_tokens(text),
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .map(|part| {
                if part.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                    retry_text_tokens(
                        part.get("text")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default(),
                    )
                } else {
                    512
                }
            })
            .sum(),
        _ => 0,
    };
    content_cost + RETRY_MESSAGE_OVERHEAD
}

fn clip_retry_text(text: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens.saturating_mul(RETRY_CHARS_PER_TOKEN);
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut = text
        .char_indices()
        .nth(max_chars)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    format!(
        "{}\n\n[… older context trimmed to fit the engine window …]",
        text[..cut].trim_end()
    )
}

fn context_limit_from_error(body: &str) -> Option<u32> {
    let lower = body.to_ascii_lowercase();
    for marker in ["\"n_ctx\"", "context size (", "context ("] {
        let Some(start) = lower.find(marker) else {
            continue;
        };
        let digits: String = body[start + marker.len()..]
            .chars()
            .skip_while(|character| !character.is_ascii_digit())
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(value) = digits.parse::<u32>() {
            if value >= 512 {
                return Some(value);
            }
        }
    }
    None
}

/// Rebuild a prompt around the current user turn when the server reports that
/// it allocates a smaller context than the one we budgeted for. The system
/// message keeps its opening instructions; older conversation and oversized
/// attached context are dropped. Tool-call fragments are omitted because a
/// partial assistant/tool pair is not valid chat history.
fn trim_messages_to_context(
    messages: &[serde_json::Value],
    context_window: u32,
    requested_max: Option<u32>,
) -> Option<Vec<serde_json::Value>> {
    let context_window = context_window as usize;
    if context_window < 512 {
        return None;
    }

    let reserve = request_output_reserve(context_window, requested_max);
    let budget = context_window
        .saturating_sub(reserve)
        .saturating_sub(RETRY_REQUEST_OVERHEAD);
    if budget < 256 {
        return None;
    }

    let current_index = messages.iter().rposition(|message| {
        message.get("role").and_then(serde_json::Value::as_str) == Some("user")
    })?;
    let current = messages[current_index].clone();
    let current_cost = retry_message_cost(&current);
    if current_cost > budget {
        return None;
    }

    let system_index = messages.iter().position(|message| {
        message.get("role").and_then(serde_json::Value::as_str) == Some("system")
    });
    let mut system_message = None;
    let mut system_cost = 0usize;
    if let Some(index) = system_index {
        let mut message = messages[index].clone();
        if let Some(text) = message
            .get("content")
            .and_then(serde_json::Value::as_str)
        {
            let allowance = (budget / 2)
                .min(budget.saturating_sub(current_cost))
                .max(256);
            if retry_message_cost(&message) > allowance {
                message["content"] = serde_json::Value::String(clip_retry_text(
                    text,
                    allowance.saturating_sub(RETRY_MESSAGE_OVERHEAD),
                ));
            }
        }
        system_cost = retry_message_cost(&message);
        system_message = Some(message);
    }

    if system_cost + current_cost > budget {
        return None;
    }

    let mut remaining = budget - system_cost - current_cost;
    let mut kept_reversed = Vec::new();
    for (index, message) in messages.iter().enumerate().rev() {
        if Some(index) == system_index || index == current_index || index > current_index {
            continue;
        }
        let role = message
            .get("role")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if role == "tool" || message.get("tool_calls").is_some() {
            continue;
        }
        let cost = retry_message_cost(message);
        if cost <= remaining {
            kept_reversed.push(message.clone());
            remaining -= cost;
        } else {
            break;
        }
    }
    kept_reversed.reverse();

    let mut result = Vec::new();
    if let Some(system) = system_message {
        result.push(system);
    }
    result.extend(kept_reversed);
    result.push(current);
    Some(result)
}

fn error_text(error: &serde_json::Value) -> String {
    if let Some(message) = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .filter(|message| !message.is_empty())
    {
        return message.to_string();
    }
    if let Some(message) = error.as_str().filter(|message| !message.is_empty()) {
        return message.to_string();
    }
    truncate_line(&error.to_string(), 800)
}

fn emit_batch(
    app: &AppHandle,
    request_id: &str,
    batch: &PayloadBatch,
    saw_output: &mut bool,
    first_token_ms: &mut Option<u64>,
    started: Instant,
) {
    for (delta, reasoning) in &batch.deltas {
        *saw_output = true;
        if first_token_ms.is_none() {
            *first_token_ms = Some(started.elapsed().as_millis() as u64);
        }
        let _ = app.emit(
            "chat://delta",
            ChatDeltaEvent {
                request_id: request_id.to_string(),
                delta: delta.clone(),
                reasoning: reasoning.clone(),
            },
        );
    }
}

fn emit_chat_notice(app: &AppHandle, request_id: &str, message: String) {
    let _ = app.emit(
        "chat://notice",
        ChatNoticeEvent {
            request_id: request_id.to_string(),
            message,
        },
    );
}

fn tools_schema(mcp_tools: &[serde_json::Value]) -> serde_json::Value {
    let mut tools = vec![serde_json::json!({
        "type": "function",
        "function": {
            "name": "save_to_memory_bank",
            "description": "Save one durable fact about the user, their preferences, their project, or a decision so it can be recalled in future chats. Call this proactively when you learn such a fact. Save exactly one concise standalone fact per call. Never save secrets, passwords, API keys, transient requests, or guesses.",
            "parameters": {
                "type": "object",
                "properties": {
                    "fact": {
                        "type": "string",
                        "description": "One concise, standalone fact, written so it makes sense without the current conversation."
                    }
                },
                "required": ["fact"],
                "additionalProperties": false
            }
        }
    })];
    tools.extend(mcp_tools.iter().cloned());
    serde_json::Value::Array(tools)
}

struct ToolRun {
    assistant_calls: Vec<serde_json::Value>,
    messages: Vec<serde_json::Value>,
}

fn run_tools(
    app: &AppHandle,
    memory: &MemoryStore,
    mut mcp: Option<&mut McpRuntime>,
    calls: &[ToolCallDraft],
) -> ToolRun {
    let selected: Vec<ToolCallDraft> = calls.iter().take(MAX_TOOL_CALLS).cloned().collect();
    let mut assistant_calls = Vec::new();
    let mut ids = Vec::new();

    for call in &selected {
        let id = if call.id.trim().is_empty() {
            format!("call_{}", uuid::Uuid::new_v4().simple())
        } else {
            call.id.clone()
        };
        ids.push(id.clone());
        assistant_calls.push(serde_json::json!({
            "id": id,
            "type": "function",
            "function": {
                "name": call.name,
                "arguments": if call.arguments.trim().is_empty() { "{}".to_string() } else { call.arguments.clone() },
            },
        }));
    }

    let mut messages = Vec::new();
    for (call, id) in selected.iter().zip(ids) {
        let result = if call.name == "save_to_memory_bank" {
            match parse_tool_arguments(&call.arguments) {
                Ok(arguments) => {
                    let fact = arguments
                        .get("fact")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default();
                    if fact.is_empty() {
                        serde_json::json!({
                            "ok": false,
                            "error": "The 'fact' argument must be a non-empty string.",
                        })
                    } else {
                        let result = tool_result_json(memory, fact);
                        if result
                            .get("saved")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                        {
                            let _ = app.emit("memory://changed", memory.list());
                        }
                        result
                    }
                }
                Err(err) => serde_json::json!({
                    "ok": false,
                    "error": err,
                }),
            }
        } else if let Some(runtime) = mcp.as_deref_mut() {
            if let Some(route) = runtime.route(&call.name) {
                match parse_tool_arguments(&call.arguments) {
                    Ok(arguments) if arguments.is_object() => {
                        let _ = app.emit(
                            "mcp://activity",
                            McpActivityEvent {
                                server: route.server.clone(),
                                tool: route.tool.clone(),
                                level: "info".into(),
                                message: format!(
                                    "Calling {} on {}...",
                                    route.tool, route.server
                                ),
                            },
                        );
                        match runtime.call_tool(&call.name, arguments) {
                            Ok(value) => {
                                let is_error = value
                                    .get("isError")
                                    .and_then(serde_json::Value::as_bool)
                                    .unwrap_or(false);
                                let summary = mcp_result_summary(&value);
                                let _ = app.emit(
                                    "mcp://activity",
                                    McpActivityEvent {
                                        server: route.server.clone(),
                                        tool: route.tool.clone(),
                                        level: if is_error { "error".into() } else { "info".into() },
                                        message: if is_error {
                                            format!("{} returned an error: {summary}", route.tool)
                                        } else {
                                            format!("{} returned: {summary}", route.tool)
                                        },
                                    },
                                );
                                value
                            }
                            Err(err) => {
                                let _ = app.emit(
                                    "mcp://activity",
                                    McpActivityEvent {
                                        server: route.server.clone(),
                                        tool: route.tool.clone(),
                                        level: "error".into(),
                                        message: format!("{} failed: {err}", route.tool),
                                    },
                                );
                                serde_json::json!({ "ok": false, "error": err })
                            }
                        }
                    }
                    Ok(_) => serde_json::json!({
                        "ok": false,
                        "error": "MCP tool arguments must be a JSON object.",
                    }),
                    Err(err) => serde_json::json!({ "ok": false, "error": err }),
                }
            } else {
                serde_json::json!({
                    "ok": false,
                    "error": format!("Unknown tool '{}'.", call.name),
                })
            }
        } else {
            serde_json::json!({
                "ok": false,
                "error": format!("Unknown tool '{}'.", call.name),
            })
        };

        messages.push(serde_json::json!({
            "role": "tool",
            "tool_call_id": id,
            "name": call.name,
            "content": result.to_string(),
        }));
    }

    ToolRun {
        assistant_calls,
        messages,
    }
}

fn parse_tool_arguments(raw: &str) -> Result<serde_json::Value, String> {
    if raw.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(raw)
        .map_err(|err| format!("The tool arguments were not valid JSON: {err}"))
}

fn mcp_result_summary(value: &serde_json::Value) -> String {
    if let Some(text) = value
        .pointer("/content/0/text")
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.trim().is_empty())
    {
        return truncate_line(text, 220);
    }
    truncate_line(&value.to_string(), 220)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn janas_attention_and_kv_are_passed_as_environment_overrides() {
        let mut settings = AppSettings::default();
        settings.janas_attention = "fast".into();
        settings.janas_kv_cache = "16".into();
        settings.llama_direct_cpu_moe = true;
        settings.llama_cpu_threads = 8;

        let mut command = Command::new("janas-server");
        apply_engine_settings(&mut command, &settings);
        let env: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().to_string(),
                    value.map(|value| value.to_string_lossy().to_string()),
                )
            })
            .collect();
        assert!(env.contains(&("JANAS_ATTN".into(), Some("int16".into()))));
        assert!(env.contains(&("JANAS_KV".into(), Some("16".into()))));

        // The llama-only direct-compute settings must never leak into Janas.
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
        for flag in [
            "--cpu-moe",
            "--moe-cache-mib",
            "--moe-cache-cpu-share",
            "--threads",
        ] {
            assert!(
                !args.iter().any(|arg| arg == flag),
                "leaked {flag} to the Janas command"
            );
        }

        settings.janas_attention = "auto".into();
        settings.janas_kv_cache = "auto".into();
        let mut auto = Command::new("janas-server");
        apply_engine_settings(&mut auto, &settings);
        assert!(auto.get_envs().next().is_none());
    }

    #[test]
    fn reads_context_limits_from_both_engine_error_shapes() {
        let llama = r#"{"error":{"code":400,"message":"request (74373 tokens) exceeds the available context size (16384 tokens)","type":"exceed_context_size_error","n_prompt_tokens":74373,"n_ctx":16384}}"#;
        assert_eq!(context_limit_from_error(llama), Some(16_384));

        let janas = "The conversation (74373 tokens) and a reply do not fit in the context (16384 tokens)";
        assert_eq!(context_limit_from_error(janas), Some(16_384));
        assert_eq!(context_limit_from_error("HTTP 500: model failed"), None);
    }

    #[test]
    fn retry_trims_oversized_context_and_preserves_the_current_turn() {
        let mut messages = vec![serde_json::json!({
            "role": "system",
            "content": "Core instructions. ".repeat(2_000)
        })];
        for index in 0..20 {
            messages.push(serde_json::json!({
                "role": if index % 2 == 0 { "user" } else { "assistant" },
                "content": format!("Historical turn {index}. {}", "detail ".repeat(1_500))
            }));
        }
        messages.push(serde_json::json!({
            "role": "user",
            "content": "What should I do next?"
        }));

        let trimmed = trim_messages_to_context(&messages, 16_384, None)
            .expect("trimmed prompt");
        assert!(trimmed.len() < messages.len());
        assert_eq!(trimmed.last().unwrap()["content"], "What should I do next?");
        assert!(trimmed[0]["content"]
            .as_str()
            .unwrap_or_default()
            .contains("older context trimmed"));
        let estimated: usize = trimmed.iter().map(retry_message_cost).sum();
        assert!(
            estimated <= 11_776,
            "retry prompt still too large: {estimated}"
        );
    }

    #[test]
    fn huge_mcp_tool_schema_is_recognized_as_unfit() {
        let huge = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "huge_tool",
                "description": "x".repeat(300_000),
                "parameters": { "type": "object", "properties": {} }
            }
        })];
        let small = vec![serde_json::json!({
            "type": "function",
            "function": {
                "name": "small_tool",
                "description": "A short description",
                "parameters": { "type": "object", "properties": {} }
            }
        })];
        let budget = request_prompt_budget(16_384, Some(4_096));
        assert!(tool_schema_tokens(&huge) > budget);
        assert!(tool_schema_tokens(&small) < budget);
        let messages = vec![serde_json::json!({"role": "system", "content": "You are Janas."})];
        let message_cost: usize = messages.iter().map(retry_message_cost).sum();
        assert!(message_cost + tool_schema_tokens(&huge) > budget);
    }

    #[test]
    fn sse_decoder_joins_split_chunks_and_flushes_the_tail() {
        let mut decoder = SseDecoder::default();
        assert!(decoder
            .feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"hel")
            .is_empty());
        let items = decoder.feed(b"lo\"}}]}\r\n\r\n");
        assert_eq!(items.len(), 1);
        match &items[0] {
            SseItem::Data(payload) => assert!(payload.contains("hello")),
            SseItem::Warning(_) => panic!("expected data, got a warning"),
        }

        let items = decoder.feed(b"data: [DONE]\n\n");
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], SseItem::Data(payload) if payload == "[DONE]"));

        let mut tail = SseDecoder::default();
        assert!(tail.feed(b"data: {\"usage\":{\"total_tokens\":3}}").is_empty());
        let items = tail.finish();
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], SseItem::Data(_)));
    }

    #[test]
    fn malformed_stream_chunks_are_ignored_without_panicking() {
        let mut round = StreamRound::default();
        match consume_payload("not json", &mut round) {
            PayloadOutcome::Continue(None) => {}
            _ => panic!("malformed payload should be skipped"),
        }
        assert_eq!(round.malformed, 1);
        assert!(round.last_problem.is_some());

        match consume_payload("{\"error\":\"engine exploded\"}", &mut round) {
            PayloadOutcome::EngineError(message) => assert_eq!(message, "engine exploded"),
            _ => panic!("engine errors should surface"),
        }
    }

    #[test]
    fn reads_content_arrays_and_reasoning_aliases() {
        let mut round = StreamRound::default();
        let payload = r#"{"choices":[{"delta":{"content":[{"type":"text","text":"Hello"}],"reasoning_content":"thinking"},"finish_reason":"stop"}]}"#;
        match consume_payload(payload, &mut round) {
            PayloadOutcome::Continue(Some((content, reasoning))) => {
                assert_eq!(content, "Hello");
                assert_eq!(reasoning, "thinking");
            }
            _ => panic!("content parts should stream"),
        }
        assert_eq!(round.content, "Hello");
        assert_eq!(round.finish_reason, "stop");
    }

    #[test]
    fn assembles_tool_calls_from_fragments() {
        let mut round = StreamRound::default();
        let first = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"save_to_memory_bank","arguments":"{\"fact\":\"The user"}}]},"finish_reason":null}]}"#;
        let second = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":" likes Rust.\"}"}}]},"finish_reason":"tool_calls"}]}"#;
        let _ = consume_payload(first, &mut round);
        let _ = consume_payload(second, &mut round);
        assert_eq!(round.tool_calls.len(), 1);
        assert_eq!(round.tool_calls[0].id, "call_1");
        assert_eq!(round.tool_calls[0].name, "save_to_memory_bank");
        assert_eq!(round.tool_calls[0].arguments, "{\"fact\":\"The user likes Rust.\"}");
        assert_eq!(round.finish_reason, "tool_calls");
        assert_eq!(
            tools_schema(&[])[0]["function"]["name"],
            "save_to_memory_bank"
        );
    }

    #[test]
    fn classifies_engine_stages() {
        assert_eq!(
            classify_log_line("loading model weights"),
            Some(EngineStatus::LoadingModel)
        );
        assert_eq!(
            classify_log_line("expert cache: 8 GiB"),
            Some(EngineStatus::LoadingModel)
        );
        assert_eq!(classify_log_line("listening on 127.0.0.1"), None);
    }

    #[test]
    fn cancellation_flag_is_shared_with_the_stream() {
        let engine = EngineManager::new(8080);
        let flag = engine.register_chat("req-test");
        assert!(!flag.load(Ordering::Relaxed));
        engine.cancel_chat("req-test");
        assert!(flag.load(Ordering::Relaxed));
        engine.clear_cancelled("req-test");
        assert!(!engine.cancelled.lock().unwrap().contains_key("req-test"));
    }

    #[test]
    fn status_order_only_moves_forward() {
        assert!(rank(EngineStatus::LoadingModel) > rank(EngineStatus::Indexing));
        assert!(rank(EngineStatus::Ready) > rank(EngineStatus::LoadingModel));
    }
}
