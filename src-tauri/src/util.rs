use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch, used for UI timestamps and duration math.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

/// Keep process output bounded before it crosses the IPC boundary.
pub fn truncate_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let mut clipped: String = line.chars().take(max_chars).collect();
    clipped.push('…');
    clipped
}
