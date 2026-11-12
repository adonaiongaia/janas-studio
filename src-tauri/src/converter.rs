use std::{
    fs,
    io::{BufReader, Read},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::{
    error::{AppError, AppResult},
    settings::{self, AppSettings},
    util::{now_ms, truncate_line},
};

#[derive(Debug, Clone, Deserialize)]
pub struct ConvertRequest {
    pub input: String,
    pub output: String,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversionStarted {
    pub run_id: String,
    pub command: String,
    pub converter_path: String,
    pub output_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConverterLog {
    pub run_id: String,
    pub stream: String,
    pub line: String,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConverterProgress {
    pub run_id: String,
    pub percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversionFinished {
    pub run_id: String,
    pub success: bool,
    pub code: Option<i32>,
    pub elapsed_ms: u64,
    pub output_path: String,
    pub output_size: u64,
    pub message: String,
}

struct ActiveConversion {
    run_id: String,
    child: Arc<Mutex<Child>>,
}

#[derive(Default)]
pub struct ConverterManager {
    active: Mutex<Option<ActiveConversion>>,
}

impl ConverterManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(
        &self,
        app: &AppHandle,
        settings: &AppSettings,
        request: ConvertRequest,
    ) -> AppResult<ConversionStarted> {
        if self
            .active
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
        {
            return Err(AppError::message(
                "A conversion is already running. Cancel it or wait for it to finish.",
            ));
        }

        let input = settings::expand_home(&request.input);
        let output = settings::expand_home(&request.output);

        if !input.is_file() {
            return Err(AppError::message(format!(
                "The input file does not exist: {}",
                input.display()
            )));
        }
        if !input
            .extension()
            .map(|ext| ext.eq_ignore_ascii_case("gguf"))
            .unwrap_or(false)
        {
            return Err(AppError::message(
                "Choose a .gguf file. Janas converts GGUF models into its .jns format.",
            ));
        }
        if output.as_os_str().is_empty() {
            return Err(AppError::message("Choose an output file for the .jns model."));
        }
        if output == input {
            return Err(AppError::message(
                "The output file must be different from the input file.",
            ));
        }
        if !output
            .extension()
            .map(|ext| ext.eq_ignore_ascii_case("jns"))
            .unwrap_or(false)
        {
            return Err(AppError::message("The output file must end in .jns."));
        }
        if output.exists() && !request.overwrite {
            return Err(AppError::message(format!(
                "{} already exists. Enable overwrite or choose another name.",
                output.display()
            )));
        }
        if let Some(parent) = output.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let (converter, is_positional) = resolve_converter(&settings.converter_path)?;
        let mut command = Command::new(&converter);
        if is_positional {
            command.arg(&input).arg(&output);
        } else {
            command
                .arg("--input")
                .arg(&input)
                .arg("--output")
                .arg(&output);
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

        let display_command = if is_positional {
            format!(
                "{} \"{}\" \"{}\"",
                converter.display(),
                input.display(),
                output.display()
            )
        } else {
            format!(
                "{} --input \"{}\" --output \"{}\"",
                converter.display(),
                input.display(),
                output.display()
            )
        };

        let mut child = command.spawn().map_err(|err| {
            AppError::message(format!(
                "Could not start {}: {err}. Check the converter path in Settings.",
                converter.display()
            ))
        })?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(child));
        let run_id = uuid::Uuid::new_v4().to_string();
        let started_ms = now_ms();
        let run_id_for_state = run_id.clone();

        *self
            .active
            .lock()
            .map_err(|_| AppError::message("Converter state is locked."))?
        = Some(ActiveConversion {
            run_id: run_id.clone(),
            child: child.clone(),
        });

        let started_payload = ConversionStarted {
            run_id: run_id.clone(),
            command: display_command.clone(),
            converter_path: converter.to_string_lossy().to_string(),
            output_path: output.to_string_lossy().to_string(),
        };
        let _ = app.emit("convert://started", started_payload.clone());
        let _ = app.emit(
            "convert://log",
            ConverterLog {
                run_id: run_id.clone(),
                stream: "system".into(),
                line: format!("$ {display_command}"),
                timestamp_ms: started_ms,
            },
        );

        let last_percent = Arc::new(Mutex::new(None::<f64>));
        let mut readers = Vec::new();
        if let Some(stdout) = stdout {
            readers.push(spawn_reader(
                app.clone(),
                run_id.clone(),
                stdout,
                "stdout",
                last_percent.clone(),
            ));
        }
        if let Some(stderr) = stderr {
            readers.push(spawn_reader(
                app.clone(),
                run_id.clone(),
                stderr,
                "stderr",
                last_percent.clone(),
            ));
        }

        spawn_monitor(
            app.clone(),
            self_arc(app),
            child,
            run_id_for_state,
            output,
            started_ms,
            readers,
        );

        Ok(started_payload)
    }

    pub fn cancel(&self, app: &AppHandle) -> AppResult<()> {
        let Ok(guard) = self.active.lock() else {
            return Err(AppError::message("Converter state is locked."));
        };
        let Some(active) = guard.as_ref() else {
            return Ok(());
        };
        let run_id = active.run_id.clone();
        if let Ok(child) = active.child.lock() {
            #[cfg(unix)]
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
            #[cfg(not(unix))]
            {
                let mut child = child;
                let _ = child.kill();
            }
        }
        let _ = app.emit(
            "convert://log",
            ConverterLog {
                run_id,
                stream: "system".into(),
                line: "Cancellation requested — stopping the converter…".into(),
                timestamp_ms: now_ms(),
            },
        );
        Ok(())
    }

    fn clear(&self, run_id: &str) {
        if let Ok(mut guard) = self.active.lock() {
            if guard
                .as_ref()
                .map(|active| active.run_id == run_id)
                .unwrap_or(false)
            {
                *guard = None;
            }
        }
    }
}

fn self_arc(app: &AppHandle) -> Arc<ConverterManager> {
    use tauri::Manager;
    app.state::<Arc<ConverterManager>>().inner().clone()
}

/// Upstream Janas ships `gguf2jns` (positional arguments); this studio also
/// accepts a `janas-convert` style binary with `--input/--output`.
fn resolve_converter(configured: &str) -> AppResult<(PathBuf, bool)> {
    let mut candidates = vec![configured.trim().to_string()];
    if configured.trim().is_empty() || configured.trim() == "janas-convert" {
        candidates.push("gguf2jns".into());
    }
    if configured.trim() == "janas-convert" {
        candidates.push("janas-converter".into());
    }

    for candidate in candidates {
        if candidate.is_empty() {
            continue;
        }
        if let Some(path) = settings::resolve_binary(&candidate) {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let positional = name.contains("gguf2jns") || name.contains("jns_planes");
            return Ok((path, positional));
        }
    }

    Err(AppError::message(
        "Could not find a converter. Set the full path to janas-convert (or upstream's gguf2jns) in Settings.",
    ))
}

fn spawn_reader(
    app: AppHandle,
    run_id: String,
    stream: impl Read + Send + 'static,
    stream_name: &'static str,
    last_percent: Arc<Mutex<Option<f64>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut pending: Vec<u8> = Vec::new();
        let mut buffer = [0u8; 8_192];

        // Converters commonly redraw a percentage with a bare carriage return.
        // Treat \r and \n both as line boundaries so progress is live.
        let handle_line = |bytes: &[u8]| {
            let line = truncate_line(String::from_utf8_lossy(bytes).trim_end(), 4_000);
            if line.is_empty() {
                return;
            }
            let _ = app.emit(
                "convert://log",
                ConverterLog {
                    run_id: run_id.clone(),
                    stream: stream_name.into(),
                    line: line.clone(),
                    timestamp_ms: now_ms(),
                },
            );
            if let Some(percent) = parse_percent(&line) {
                let should_emit = {
                    let mut guard = match last_percent.lock() {
                        Ok(guard) => guard,
                        Err(_) => return,
                    };
                    let changed = guard
                        .map(|previous| (previous - percent).abs() >= 0.25)
                        .unwrap_or(true);
                    if changed {
                        *guard = Some(percent);
                    }
                    changed
                };
                if should_emit {
                    let _ = app.emit(
                        "convert://progress",
                        ConverterProgress {
                            run_id: run_id.clone(),
                            percent: Some(percent),
                        },
                    );
                }
            }
        };

        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    for byte in &buffer[..read] {
                        if *byte == b'\n' || *byte == b'\r' {
                            if !pending.is_empty() {
                                handle_line(&pending);
                                pending.clear();
                            }
                        } else {
                            pending.push(*byte);
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if !pending.is_empty() {
            handle_line(&pending);
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn spawn_monitor(
    app: AppHandle,
    manager: Arc<ConverterManager>,
    child: Arc<Mutex<Child>>,
    run_id: String,
    output: PathBuf,
    started_ms: u64,
    readers: Vec<std::thread::JoinHandle<()>>,
) {
    std::thread::spawn(move || {
        let code = loop {
            {
                let Ok(mut guard) = child.lock() else {
                    break None;
                };
                match guard.try_wait() {
                    Ok(Some(status)) => break status.code(),
                    Ok(None) => {}
                    Err(_) => break None,
                }
            }
            std::thread::sleep(Duration::from_millis(150));
        };

        // Let the pipe readers drain fully before announcing the result.
        for reader in readers {
            let _ = reader.join();
        }

        let output_size = fs::metadata(&output).map(|meta| meta.len()).unwrap_or(0);
        let success = code == Some(0) && output.is_file();
        let message = if success {
            format!("Conversion complete — {}", human_size(output_size))
        } else if code == Some(0) {
            "The converter finished but no .jns file was written.".into()
        } else {
            format!(
                "The converter exited with {}.",
                code.map(|value| value.to_string())
                    .unwrap_or_else(|| "an unknown status".into())
            )
        };

        let _ = app.emit(
            "convert://finished",
            ConversionFinished {
                run_id: run_id.clone(),
                success,
                code,
                elapsed_ms: now_ms().saturating_sub(started_ms),
                output_path: output.to_string_lossy().to_string(),
                output_size,
                message,
            },
        );
        manager.clear(&run_id);
    });
}

fn parse_percent(line: &str) -> Option<f64> {
    let bytes = line.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'%' {
            continue;
        }
        let mut start = index;
        while start > 0 && (bytes[start - 1].is_ascii_digit() || bytes[start - 1] == b'.') {
            start -= 1;
        }
        if start == index {
            continue;
        }
        if let Ok(value) = line[start..index].trim().parse::<f64>() {
            return Some(value.clamp(0.0, 100.0));
        }
    }
    None
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_from_converter_output() {
        assert_eq!(parse_percent("converting tensors: 42%"), Some(42.0));
        assert_eq!(parse_percent("100.0% done"), Some(100.0));
        assert_eq!(parse_percent("72%"), Some(72.0));
        assert_eq!(parse_percent("loading model"), None);
        assert_eq!(parse_percent("150%"), Some(100.0));
    }

    #[test]
    fn rejects_malformed_percentages_without_panicking() {
        assert_eq!(parse_percent("%"), None);
        assert_eq!(parse_percent("abc%"), None);
        assert_eq!(parse_percent("naïve 12%"), Some(12.0));
    }

    #[test]
    fn formats_sizes() {
        assert_eq!(human_size(512), "512.0 B");
        assert_eq!(human_size(2_500_000_000), "2.3 GB");
    }
}
