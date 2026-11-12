//! MCP over stdio.
//!
//! A single client implementation powers both the MCP Hub probe and model
//! tool calls during chat. Startup and call failures carry the child's exit
//! status and the tail of stderr, so a server that refuses to start is no
//! longer indistinguishable from one that is merely slow.

use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    error::{AppError, AppResult},
    settings::{expand_home, resolve_binary},
    util::truncate_line,
};

/// Installing an npx package or loading a local model can take much longer
/// than the old eight-second limit.
const START_TIMEOUT: Duration = Duration::from_secs(30);
const TOOL_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_LOG_LINES: usize = 80;
const MAX_STDERR_LINES: usize = 64;
const MAX_TOOL_NAME: usize = 64;

#[derive(Debug, Clone, Deserialize)]
pub struct McpServerSpec {
    #[serde(default)]
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// One `KEY=VALUE` per line. Values are never written to the probe log.
    #[serde(default)]
    pub env: String,
    #[serde(default)]
    pub cwd: String,
}

impl McpServerSpec {
    fn display_name(&self) -> String {
        let name = self.name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
        std::path::Path::new(self.command.trim())
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("MCP server")
            .to_string()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(skip_serializing)]
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpProbe {
    pub ok: bool,
    pub server_name: String,
    pub server_version: String,
    pub tools: Vec<McpTool>,
    pub log: Vec<String>,
    pub message: String,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpActivityEvent {
    pub server: String,
    pub tool: String,
    pub level: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct McpToolRoute {
    pub server: String,
    pub tool: String,
}

struct McpStartError {
    message: String,
    log: Vec<String>,
}

struct McpSession {
    display_name: String,
    server_name: String,
    server_version: String,
    tools: Vec<McpTool>,
    child: Child,
    stdin: ChildStdin,
    receiver: mpsc::Receiver<String>,
    stderr: Arc<Mutex<Vec<String>>>,
    stderr_thread: Option<JoinHandle<()>>,
    stderr_logged: usize,
    next_id: u64,
    log: Vec<String>,
}

impl McpSession {
    fn start(spec: &McpServerSpec) -> Result<Self, McpStartError> {
        let display_name = spec.display_name();
        let mut log = vec![format!(
            "$ {} {}",
            spec.command.trim(),
            spec.args.join(" ")
        )];
        if !spec.cwd.trim().is_empty() {
            log.push(format!("cwd: {}", spec.cwd.trim()));
        }

        let resolved = resolve_binary(&spec.command).ok_or_else(|| McpStartError {
            message: format!(
                "Could not find the MCP command `{}`. Use a full path or check PATH.",
                spec.command.trim()
            ),
            log: log.clone(),
        })?;

        let env = parse_env(&spec.env).map_err(|message| McpStartError {
            message: format!("{display_name}: {message}"),
            log: log.clone(),
        })?;

        let cwd = if spec.cwd.trim().is_empty() {
            None
        } else {
            let expanded = expand_home(spec.cwd.trim());
            if !expanded.is_dir() {
                return Err(McpStartError {
                    message: format!(
                        "The working directory for {display_name} does not exist or is not a directory: {}",
                        expanded.display()
                    ),
                    log,
                });
            }
            Some(expanded)
        };

        let mut command = Command::new(&resolved);
        command
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !env.is_empty() {
            command.envs(env);
        }
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }

        let mut child = command.spawn().map_err(|err| McpStartError {
            message: format!(
                "Could not start {display_name} ({}): {err}. Check the command and arguments.",
                resolved.display()
            ),
            log: log.clone(),
        })?;

        let mut stdin = child.stdin.take().ok_or_else(|| McpStartError {
            message: format!("{display_name} has no stdin."),
            log: log.clone(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| McpStartError {
            message: format!("{display_name} has no stdout."),
            log: log.clone(),
        })?;
        let stderr = child.stderr.take().ok_or_else(|| McpStartError {
            message: format!("{display_name} has no stderr."),
            log: log.clone(),
        })?;

        let (sender, receiver) = mpsc::channel::<String>();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });

        let stderr_buffer = Arc::new(Mutex::new(Vec::<String>::new()));
        let stderr_sink = Arc::clone(&stderr_buffer);
        let stderr_thread = thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                let line = truncate_line(line.trim_end(), 500);
                if line.is_empty() {
                    continue;
                }
                if let Ok(mut guard) = stderr_sink.lock() {
                    if guard.len() < MAX_STDERR_LINES {
                        guard.push(line);
                    }
                }
            }
        });

        // Keep stdin mutable for the handshake below.
        let _ = stdin.flush();

        let mut session = Self {
            display_name,
            server_name: String::new(),
            server_version: String::new(),
            tools: Vec::new(),
            child,
            stdin,
            receiver,
            stderr: stderr_buffer,
            stderr_thread: Some(stderr_thread),
            stderr_logged: 0,
            next_id: 1,
            log: std::mem::take(&mut log),
        };

        if let Err(message) = session.handshake() {
            session.stop();
            session.capture_stderr();
            let log = session.log.clone();
            let message = session.with_stderr(message);
            return Err(McpStartError { message, log });
        }
        session.capture_stderr();
        Ok(session)
    }

    fn handshake(&mut self) -> Result<(), String> {
        let initialize = self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "Janas Studio",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
            START_TIMEOUT,
        )?;

        if let Some(info) = initialize.get("serverInfo") {
            self.server_name = info
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            self.server_version = info
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
        }

        self.notify("notifications/initialized")?;

        let listing = self.request("tools/list", json!({}), START_TIMEOUT)?;
        if let Some(tools) = listing.get("tools").and_then(Value::as_array) {
            self.tools = tools
                .iter()
                .filter_map(|tool| {
                    let name = tool.get("name").and_then(Value::as_str)?.trim();
                    if name.is_empty() {
                        return None;
                    }
                    Some(McpTool {
                        name: name.to_string(),
                        description: tool
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        input_schema: tool
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
                    })
                })
                .collect();
        }

        Ok(())
    }

    fn notify(&mut self, method: &str) -> Result<(), String> {
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "method": method
        }))
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))?;

        let deadline = Instant::now() + timeout;
        let mut stdout_closed = false;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.receiver.recv_timeout(remaining) {
                Ok(line) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    self.push_log(trimmed);
                    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                        continue;
                    };
                    if !response_id_matches(&value, id) {
                        continue;
                    }
                    if let Some(error) = value.get("error").filter(|error| !error.is_null()) {
                        return Err(format!(
                            "MCP {method} failed: {}",
                            error_message(error)
                        ));
                    }
                    return Ok(value.get("result").cloned().unwrap_or(Value::Null));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    stdout_closed = true;
                    break;
                }
            }
        }

        // A child can close stdout a few milliseconds before the process
        // itself is reaped. Give an early exit a brief moment so the error
        // carries its exit status instead of a less useful "closed stdout".
        let status = self.child.try_wait().ok().flatten().or_else(|| {
            if !stdout_closed {
                return None;
            }
            let deadline = Instant::now() + Duration::from_millis(250);
            loop {
                match self.child.try_wait() {
                    Ok(Some(status)) => return Some(status),
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    _ => return None,
                }
            }
        });
        if status.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let stderr = self.stderr_summary();
        let mut message = if let Some(status) = status {
            format!("MCP server exited with {status} before replying to {method}.")
        } else if stdout_closed {
            format!("MCP server closed stdout before replying to {method}.")
        } else {
            format!(
                "MCP server did not reply to {method} within {} seconds; the process was stopped.",
                timeout.as_secs()
            )
        };
        if !stderr.is_empty() {
            message.push_str(" Stderr: ");
            message.push_str(&stderr);
        }
        Err(message)
    }

    fn call_tool(&mut self, tool: &str, arguments: Value) -> Result<Value, String> {
        self.request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
            TOOL_TIMEOUT,
        )
    }

    fn write_message(&mut self, value: &Value) -> Result<(), String> {
        writeln!(self.stdin, "{value}")
            .and_then(|_| self.stdin.flush())
            .map_err(|err| {
                self.push_log(&format!("write failed: {err}"));
                format!("Could not write to the MCP server: {err}")
            })
    }

    fn push_log(&mut self, line: &str) {
        if self.log.len() < MAX_LOG_LINES {
            self.log.push(truncate_line(line, 600));
        }
    }

    fn capture_stderr(&mut self) {
        let lines = self
            .stderr
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        for line in lines.iter().skip(self.stderr_logged) {
            self.push_log(&format!("[stderr] {line}"));
        }
        self.stderr_logged = lines.len();
    }

    fn stderr_summary(&self) -> String {
        let lines = self
            .stderr
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        lines
            .iter()
            .rev()
            .take(3)
            .rev()
            .map(|line| truncate_line(line, 260))
            .collect::<Vec<_>>()
            .join(" | ")
    }

    fn with_stderr(&self, message: String) -> String {
        let stderr = self.stderr_summary();
        if stderr.is_empty() || message.contains("Stderr:") {
            message
        } else {
            format!("{message} Stderr: {stderr}")
        }
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct McpRuntime {
    sessions: Vec<McpSession>,
    routes: HashMap<String, RuntimeRoute>,
    tools: Vec<Value>,
    warnings: Vec<String>,
}

struct RuntimeRoute {
    session: usize,
    server: String,
    tool: String,
}

impl McpRuntime {
    pub fn connect(specs: &[McpServerSpec]) -> Self {
        let mut sessions = Vec::new();
        let mut warnings = Vec::new();

        for spec in specs {
            if spec.command.trim().is_empty() {
                warnings.push(format!("{}: the command is empty.", spec.display_name()));
                continue;
            }
            match McpSession::start(spec) {
                Ok(session) => sessions.push(session),
                Err(err) => warnings.push(format!("{}: {}", spec.display_name(), err.message)),
            }
        }

        let mut routes = HashMap::new();
        let mut tools = Vec::new();
        let mut used_names = HashSet::new();

        for (session_index, session) in sessions.iter().enumerate() {
            for tool in &session.tools {
                let exposed = unique_tool_name(&mut used_names, session_index, &tool.name);
                let schema = normalized_schema(&tool.input_schema);
                let description = if tool.description.trim().is_empty() {
                    format!("{} from the MCP server {}.", tool.name, session.display_name)
                } else {
                    format!(
                        "{} (MCP server: {})",
                        tool.description.trim(),
                        session.display_name
                    )
                };
                routes.insert(
                    exposed.clone(),
                    RuntimeRoute {
                        session: session_index,
                        server: session.display_name.clone(),
                        tool: tool.name.clone(),
                    },
                );
                tools.push(json!({
                    "type": "function",
                    "function": {
                        "name": exposed,
                        "description": description,
                        "parameters": schema
                    }
                }));
            }
        }

        Self {
            sessions,
            routes,
            tools,
            warnings,
        }
    }

    pub fn tool_definitions(&self) -> Vec<Value> {
        self.tools.clone()
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn route(&self, exposed: &str) -> Option<McpToolRoute> {
        self.routes.get(exposed).map(|route| McpToolRoute {
            server: route.server.clone(),
            tool: route.tool.clone(),
        })
    }

    pub fn call_tool(&mut self, exposed: &str, arguments: Value) -> Result<Value, String> {
        let route = self
            .routes
            .get(exposed)
            .ok_or_else(|| format!("Unknown MCP tool `{exposed}`."))?;
        let session = self
            .sessions
            .get_mut(route.session)
            .ok_or_else(|| format!("The MCP server for `{exposed}` is no longer available."))?;
        session.call_tool(&route.tool, arguments)
    }
}

pub fn probe_server(spec: &McpServerSpec) -> McpProbe {
    let started = Instant::now();
    match McpSession::start(spec) {
        Ok(mut session) => {
            session.capture_stderr();
            let tools = session.tools.clone();
            let log = session.log.clone();
            let server_name = if session.server_name.trim().is_empty() {
                spec.display_name()
            } else {
                session.server_name.clone()
            };
            let server_version = session.server_version.clone();
            session.stop();
            McpProbe {
                ok: true,
                server_name,
                server_version,
                message: format!(
                    "Connected. {} tool{} exposed.",
                    tools.len(),
                    if tools.len() == 1 { "" } else { "s" }
                ),
                tools,
                log,
                elapsed_ms: started.elapsed().as_millis() as u64,
            }
        }
        Err(err) => McpProbe {
            ok: false,
            server_name: spec.display_name(),
            server_version: String::new(),
            tools: Vec::new(),
            message: err.message,
            log: err.log,
            elapsed_ms: started.elapsed().as_millis() as u64,
        },
    }
}

#[tauri::command]
pub async fn mcp_probe(server: McpServerSpec) -> AppResult<McpProbe> {
    if server.command.trim().is_empty() {
        return Err(AppError::message("An MCP server needs a command to run."));
    }
    tauri::async_runtime::spawn_blocking(move || probe_server(&server))
        .await
        .map_err(|err| AppError::message(format!("The MCP probe stopped: {err}")))
}

fn parse_env(input: &str) -> Result<Vec<(String, String)>, String> {
    let mut values = Vec::new();
    for (index, raw) in input.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!(
                "environment line {} is not KEY=VALUE",
                index + 1
            ));
        };
        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err(format!("environment line {} has an invalid variable name", index + 1));
        }
        let value = strip_quotes(value.trim());
        if value.contains('\0') {
            return Err(format!(
                "environment line {} contains a NUL byte",
                index + 1
            ));
        }
        values.push((key.to_string(), value.to_string()));
    }
    Ok(values)
}

fn strip_quotes(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return &value[1..value.len() - 1];
        }
    }
    value
}

fn normalized_schema(schema: &Value) -> Value {
    if !schema.is_object() {
        return json!({ "type": "object", "properties": {} });
    }
    let mut schema = schema.clone();
    if schema.get("type").is_none() {
        schema["type"] = json!("object");
    }
    if schema.get("type") == Some(&json!("object")) && schema.get("properties").is_none() {
        schema["properties"] = json!({});
    }
    schema
}

fn sanitize_tool_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "tool".to_string()
    } else {
        trimmed.to_string()
    }
}

fn unique_tool_name(used: &mut HashSet<String>, server_index: usize, tool: &str) -> String {
    let prefix = format!("mcp_{}_", server_index + 1);
    let base = sanitize_tool_component(tool);
    let mut suffix_number = 1usize;

    loop {
        let suffix = if suffix_number == 1 {
            String::new()
        } else {
            format!("_{suffix_number}")
        };
        let room = MAX_TOOL_NAME
            .saturating_sub(prefix.len())
            .saturating_sub(suffix.len())
            .max(1);
        let head: String = base.chars().take(room).collect();
        let candidate = format!("{prefix}{head}{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix_number += 1;
        if suffix_number > 9_999 {
            // Practically unreachable, but never spin forever on a pathological
            // server that exposes thousands of identically named tools.
            let fallback = format!("{prefix}{suffix_number}");
            used.insert(fallback.clone());
            return fallback;
        }
    }
}

fn response_id_matches(value: &Value, id: u64) -> bool {
    match value.get("id") {
        Some(Value::Number(number)) => number.as_u64() == Some(id),
        Some(Value::String(text)) => text == &id.to_string(),
        _ => false,
    }
}

fn error_message(error: &Value) -> String {
    if let Some(message) = error
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
    {
        return truncate_line(message, 800);
    }
    if let Some(message) = error.as_str().filter(|message| !message.is_empty()) {
        return truncate_line(message, 800);
    }
    truncate_line(&error.to_string(), 800)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_environment_and_quoted_values() {
        let parsed = parse_env(
            "TOKEN=abc\n# comment\nexport ROOT=\"/tmp/my project\"\nEMPTY=\n",
        )
        .expect("environment");
        assert_eq!(
            parsed,
            vec![
                ("TOKEN".into(), "abc".into()),
                ("ROOT".into(), "/tmp/my project".into()),
                ("EMPTY".into(), String::new()),
            ]
        );
        assert!(parse_env("NOT_AN_ASSIGNMENT").is_err());
        assert!(parse_env("BAD-NAME=1").is_err());
    }

    #[test]
    fn exposes_unique_bounded_tool_names() {
        let mut used = HashSet::new();
        let first = unique_tool_name(&mut used, 0, "read file");
        let second = unique_tool_name(&mut used, 0, "read file");
        assert_eq!(first, "mcp_1_read_file");
        assert_ne!(first, second);
        assert!(first.len() <= MAX_TOOL_NAME);
        assert!(second.len() <= MAX_TOOL_NAME);
        let long = unique_tool_name(&mut used, 2, &"x".repeat(200));
        assert!(long.len() <= MAX_TOOL_NAME);
    }

    #[test]
    #[cfg(unix)]
    fn probe_reports_early_exit_and_stderr() {
        let script = std::env::temp_dir().join(format!(
            "janas-mcp-early-exit-{}.sh",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &script,
            "#!/bin/sh\necho 'missing model path' >&2\nexit 2\n",
        )
        .expect("write script");
        let spec = McpServerSpec {
            name: "early exit".into(),
            command: "/bin/sh".into(),
            args: vec![script.to_string_lossy().to_string()],
            env: String::new(),
            cwd: String::new(),
        };
        let probe = probe_server(&spec);
        assert!(!probe.ok);
        assert!(probe.message.contains("exited with"), "{}", probe.message);
        assert!(probe.log.iter().any(|line| line.contains("missing model")));
        let _ = std::fs::remove_file(script);
    }

    #[test]
    #[cfg(unix)]
    fn runtime_discovers_and_calls_a_tool_with_environment() {
        let script = std::env::temp_dir().join(format!(
            "janas-mcp-tool-runtime-{}.sh",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &script,
            r#"#!/bin/sh
if [ "$MCP_TEST_VALUE" != "works" ]; then
  echo 'missing test environment' >&2
  exit 3
fi
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1"}}}'
      ;;
    *'"method":"tools/list"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"Echo text","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}}]}}'
      ;;
    *'"method":"tools/call"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"echo ok"}]}}'
      ;;
  esac
done
"#,
        )
        .expect("write fake server");

        let spec = McpServerSpec {
            name: "fake".into(),
            command: "/bin/sh".into(),
            args: vec![script.to_string_lossy().to_string()],
            env: "MCP_TEST_VALUE=works".into(),
            cwd: String::new(),
        };
        let mut runtime = McpRuntime::connect(&[spec]);
        assert!(runtime.warnings().is_empty());
        let tools = runtime.tool_definitions();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["function"]["name"], "mcp_1_echo");
        let result = runtime
            .call_tool("mcp_1_echo", json!({ "text": "hello" }))
            .expect("tool call");
        assert_eq!(result["content"][0]["text"], "echo ok");
        let _ = std::fs::remove_file(script);
    }
}
