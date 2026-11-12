//! Minimal llama-server stand-in for the dual-engine integration harness.
//!
//! The harness compiles this file with `rustc` into the disposable managed bin
//! directory, then exercises the real `EngineSupervisor` and `LlamaRunner`
//! against it. It answers the probe commands, serves `/health` and `/metrics`,
//! and writes recognisable startup lines to stdout so log forwarding can be
//! asserted end to end.

use std::{
    env,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    process,
    thread,
    time::Duration,
};

const HELP: &str = "\
--model PATH                 model to load
--host HOST                  listen address
--port PORT                  listen port
--ctx-size N                 context size
--n-gpu-layers N             layers to offload
--flash-attn [on|off|auto]   flash attention
--cache-type-k TYPE          K cache type
--cache-type-v TYPE          V cache type
--metrics                    expose /metrics
--jinja                      use the model chat template
--top-k N                    top-k sampling
--alias STR                  model alias
";

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.iter().any(|arg| arg == "--version") {
        println!("version: 6999 (fake-harness)");
        return;
    }
    if args.iter().any(|arg| arg == "--help") {
        println!("{HELP}");
        return;
    }

    let mut port = 18081u16;
    let mut model = "unknown".to_string();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--port" => {
                if let Some(value) = args.get(index + 1) {
                    port = value.parse().unwrap_or(port);
                    index += 1;
                }
            }
            "--model" => {
                if let Some(value) = args.get(index + 1) {
                    model = value.clone();
                    index += 1;
                }
            }
            _ => {}
        }
        index += 1;
    }

    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("HARNESS_FAKE_SERVER_BIND_FAILED {port}: {err}");
            process::exit(2);
        }
    };

    println!("HARNESS_FAKE_SERVER_STARTED model={model}");
    println!("HARNESS_FAKE_SERVER_READY 127.0.0.1:{port}");
    let _ = io::stdout().flush();

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                thread::spawn(move || handle(stream));
            }
            Err(err) => {
                eprintln!("HARNESS_FAKE_SERVER_ACCEPT_FAILED: {err}");
                break;
            }
        }
    }
}

fn handle(mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buffer = [0u8; 8_192];
    let read = stream.read(&mut buffer).unwrap_or(0);
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.split('?').next())
        .unwrap_or("/");

    let (status, content_type, body): (&str, &str, String) = match path {
        "/health" => ("200 OK", "application/json", "{\"status\":\"ok\"}\n".into()),
        "/metrics" => (
            "200 OK",
            "text/plain; version=0.0.4",
            concat!(
                "# HELP llamacpp:tokens_predicted_total Tokens generated\n",
                "# TYPE llamacpp:tokens_predicted_total counter\n",
                "llamacpp:tokens_predicted_total 42\n",
                "# HELP llamacpp:prompt_tokens_total Prompt tokens processed\n",
                "# TYPE llamacpp:prompt_tokens_total counter\n",
                "llamacpp:prompt_tokens_total 7\n",
                "# HELP llamacpp:kv_cache_usage_ratio KV cache usage\n",
                "# TYPE llamacpp:kv_cache_usage_ratio gauge\n",
                "llamacpp:kv_cache_usage_ratio 0.25\n",
            )
            .into(),
        ),
        "/v1/chat/completions" => (
            "200 OK",
            "text/event-stream",
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n"
                .into(),
        ),
        _ => ("404 Not Found", "text/plain", "not found\n".into()),
    };

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}
