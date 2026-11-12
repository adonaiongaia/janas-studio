//! Small, read-only GGUF metadata reader.
//!
//! The UI needs the model's real transformer block count so GPU offload can be
//! capped at the number of layers the file actually contains. GGUF stores that
//! in metadata (`<architecture>.block_count`), before the tensor data, so this
//! reader never loads model weights.

use std::{
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
};

use serde::Serialize;

use crate::{
    error::{AppError, AppResult},
    settings::expand_home,
};

const TYPE_UINT8: u32 = 0;
const TYPE_INT8: u32 = 1;
const TYPE_UINT16: u32 = 2;
const TYPE_INT16: u32 = 3;
const TYPE_UINT32: u32 = 4;
const TYPE_INT32: u32 = 5;
const TYPE_FLOAT32: u32 = 6;
const TYPE_BOOL: u32 = 7;
const TYPE_STRING: u32 = 8;
const TYPE_ARRAY: u32 = 9;
const TYPE_UINT64: u32 = 10;
const TYPE_INT64: u32 = 11;
const TYPE_FLOAT64: u32 = 12;

const MAX_METADATA_ENTRIES: u64 = 1_000_000;
const MAX_KEY_BYTES: u64 = 16 * 1024;
const MAX_ARCHITECTURE_BYTES: u64 = 16 * 1024;
const MAX_NAME_BYTES: u64 = 16 * 1024;
const MAX_CHAT_TEMPLATE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_STRING_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ARRAY_ENTRIES: u64 = 16_000_000;
const MAX_ARRAY_DEPTH: u8 = 8;

/// Metadata the offload control needs from one GGUF file.
#[derive(Debug, Clone, Serialize)]
pub struct GgufModelInfo {
    pub path: String,
    pub architecture: Option<String>,
    /// The transformer block count (`block_count`) declared by the model.
    pub layer_count: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct GgufFeatures {
    pub name: Option<String>,
    pub chat_template: Option<String>,
}

#[derive(Debug, Default)]
struct Metadata {
    architecture: Option<String>,
    layer_count: Option<u32>,
    name: Option<String>,
    chat_template: Option<String>,
}

/// Read only the metadata header of a GGUF model.
pub fn inspect(path: &str) -> AppResult<GgufModelInfo> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(AppError::message(
            "Choose a .gguf model before reading its layer count.",
        ));
    }

    let expanded = expand_home(trimmed);
    if !expanded.is_file() {
        return Err(AppError::message(format!(
            "Could not read GGUF metadata because {} does not exist.",
            expanded.display()
        )));
    }

    let file = File::open(&expanded).map_err(|err| {
        AppError::message(format!(
            "Could not open {} to read its layer count: {err}",
            expanded.display()
        ))
    })?;
    let metadata = read_metadata(&mut BufReader::new(file)).map_err(|err| {
        AppError::message(format!(
            "Could not read GGUF metadata from {}: {err}",
            expanded.display()
        ))
    })?;

    Ok(GgufModelInfo {
        path: expanded.to_string_lossy().to_string(),
        architecture: metadata.architecture,
        layer_count: metadata.layer_count,
    })
}

/// Best-effort helper used when settings are loaded or saved. A malformed or
/// missing model must not stop the rest of the settings from being persisted.
pub fn layer_count(path: &str) -> Option<u32> {
    inspect(path).ok().and_then(|info| info.layer_count)
}

/// Read the small pieces of GGUF metadata the model-library badges need. A
/// malformed file must not prevent the rest of the library from loading.
pub fn features(path: &str) -> Option<GgufFeatures> {
    let expanded = expand_home(path.trim());
    let file = File::open(expanded).ok()?;
    let metadata = read_metadata(&mut BufReader::new(file)).ok()?;
    Some(GgufFeatures {
        name: metadata.name,
        chat_template: metadata.chat_template,
    })
}

fn read_metadata<R: Read + Seek>(reader: &mut R) -> Result<Metadata, String> {
    let mut magic = [0u8; 4];
    read_exact(reader, &mut magic)?;
    if &magic != b"GGUF" {
        return Err("this is not a GGUF file (missing GGUF header)".into());
    }

    let version = read_u32(reader)?;
    if !(2..=3).contains(&version) {
        return Err(format!(
            "unsupported GGUF version {version}; this build reads version 2 and 3 files"
        ));
    }
    let _tensor_count = read_u64(reader)?;
    let metadata_count = read_u64(reader)?;
    if metadata_count > MAX_METADATA_ENTRIES {
        return Err(format!(
            "metadata declares an unreasonable {metadata_count} entries"
        ));
    }

    let mut metadata = Metadata::default();
    for _ in 0..metadata_count {
        let key_bytes = read_sized(reader, MAX_KEY_BYTES, "metadata key")?;
        let key = String::from_utf8_lossy(&key_bytes);
        let value_type = read_u32(reader)?;

        if key == "general.architecture" {
            if value_type == TYPE_STRING {
                let raw = read_sized(
                    reader,
                    MAX_ARCHITECTURE_BYTES,
                    "general.architecture value",
                )?;
                let architecture = String::from_utf8_lossy(&raw).trim().to_string();
                if !architecture.is_empty() {
                    metadata.architecture = Some(architecture);
                }
            } else {
                skip_value(reader, value_type, 0)?;
            }
            continue;
        }

        if key == "general.name" {
            if value_type == TYPE_STRING {
                metadata.name = read_metadata_string(reader, MAX_NAME_BYTES)?;
            } else {
                skip_value(reader, value_type, 0)?;
            }
            continue;
        }

        if key == "tokenizer.chat_template" {
            if value_type == TYPE_STRING {
                metadata.chat_template = read_metadata_string(reader, MAX_CHAT_TEMPLATE_BYTES)?;
            } else {
                skip_value(reader, value_type, 0)?;
            }
            continue;
        }

        if key == "block_count" || key.ends_with(".block_count") {
            if let Some(value) = read_unsigned_value(reader, value_type)? {
                if value > 0 && value <= u32::MAX as u64 {
                    metadata.layer_count = Some(value as u32);
                }
            }
            continue;
        }

        skip_value(reader, value_type, 0)?;
    }

    Ok(metadata)
}

fn read_metadata_string<R: Read + Seek>(reader: &mut R, max: u64) -> Result<Option<String>, String> {
    let length = read_u64(reader)?;
    if length > max {
        skip_bytes(reader, length)?;
        return Ok(None);
    }
    let length = usize::try_from(length).map_err(|_| "metadata string is too large".to_string())?;
    let mut bytes = vec![0u8; length];
    read_exact(reader, &mut bytes)?;
    Ok(Some(String::from_utf8_lossy(&bytes).to_string()))
}

fn fixed_value_size(value_type: u32) -> Option<u64> {
    match value_type {
        TYPE_UINT8 | TYPE_INT8 | TYPE_BOOL => Some(1),
        TYPE_UINT16 | TYPE_INT16 => Some(2),
        TYPE_UINT32 | TYPE_INT32 | TYPE_FLOAT32 => Some(4),
        TYPE_UINT64 | TYPE_INT64 | TYPE_FLOAT64 => Some(8),
        _ => None,
    }
}

fn skip_value<R: Read + Seek>(reader: &mut R, value_type: u32, depth: u8) -> Result<(), String> {
    if depth > MAX_ARRAY_DEPTH {
        return Err("metadata arrays are nested too deeply".into());
    }
    if let Some(size) = fixed_value_size(value_type) {
        return skip_bytes(reader, size);
    }

    match value_type {
        TYPE_STRING => {
            let length = read_u64(reader)?;
            if length > MAX_STRING_BYTES {
                return Err(format!(
                    "metadata string declares an unreasonable {length} bytes"
                ));
            }
            skip_bytes(reader, length)
        }
        TYPE_ARRAY => {
            let inner_type = read_u32(reader)?;
            let count = read_u64(reader)?;
            if count > MAX_ARRAY_ENTRIES {
                return Err(format!(
                    "metadata array declares an unreasonable {count} entries"
                ));
            }
            if let Some(size) = fixed_value_size(inner_type) {
                let bytes = size
                    .checked_mul(count)
                    .ok_or_else(|| "metadata array size overflowed".to_string())?;
                return skip_bytes(reader, bytes);
            }
            for _ in 0..count {
                skip_value(reader, inner_type, depth + 1)?;
            }
            Ok(())
        }
        other => Err(format!("unsupported GGUF metadata type {other}")),
    }
}

fn read_unsigned_value<R: Read + Seek>(
    reader: &mut R,
    value_type: u32,
) -> Result<Option<u64>, String> {
    match value_type {
        TYPE_UINT8 => Ok(Some(read_u8(reader)? as u64)),
        TYPE_INT8 => {
            let value = read_u8(reader)? as i8;
            Ok((value >= 0).then_some(value as u64))
        }
        TYPE_UINT16 => Ok(Some(read_u16(reader)? as u64)),
        TYPE_INT16 => {
            let value = read_u16(reader)? as i16;
            Ok((value >= 0).then_some(value as u64))
        }
        TYPE_UINT32 => Ok(Some(read_u32(reader)? as u64)),
        TYPE_INT32 => {
            let value = read_u32(reader)? as i32;
            Ok((value >= 0).then_some(value as u64))
        }
        TYPE_UINT64 => Ok(Some(read_u64(reader)?)),
        TYPE_INT64 => {
            let value = read_u64(reader)? as i64;
            Ok((value >= 0).then_some(value as u64))
        }
        TYPE_BOOL => {
            let _ = read_u8(reader)?;
            Ok(None)
        }
        other => {
            skip_value(reader, other, 0)?;
            Ok(None)
        }
    }
}

fn read_sized<R: Read>(reader: &mut R, max: u64, label: &str) -> Result<Vec<u8>, String> {
    let length = read_u64(reader)?;
    if length > max {
        return Err(format!(
            "{label} declares an unreasonable {length} bytes"
        ));
    }
    let length = usize::try_from(length).map_err(|_| format!("{label} is too large"))?;
    let mut bytes = vec![0u8; length];
    read_exact(reader, &mut bytes)?;
    Ok(bytes)
}

fn read_exact<R: Read>(reader: &mut R, buffer: &mut [u8]) -> Result<(), String> {
    reader
        .read_exact(buffer)
        .map_err(|err| format!("unexpected end of GGUF metadata: {err}"))
}

fn read_u8<R: Read>(reader: &mut R) -> Result<u8, String> {
    let mut bytes = [0u8; 1];
    read_exact(reader, &mut bytes)?;
    Ok(bytes[0])
}

fn read_u16<R: Read>(reader: &mut R) -> Result<u16, String> {
    let mut bytes = [0u8; 2];
    read_exact(reader, &mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32<R: Read>(reader: &mut R) -> Result<u32, String> {
    let mut bytes = [0u8; 4];
    read_exact(reader, &mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64<R: Read>(reader: &mut R) -> Result<u64, String> {
    let mut bytes = [0u8; 8];
    read_exact(reader, &mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn skip_bytes<R: Seek>(reader: &mut R, length: u64) -> Result<(), String> {
    if length == 0 {
        return Ok(());
    }
    let offset = i64::try_from(length).map_err(|_| "metadata offset is too large".to_string())?;
    reader
        .seek(SeekFrom::Current(offset))
        .map_err(|err| format!("could not skip GGUF metadata: {err}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn header(metadata_count: u64) -> Vec<u8> {
        let mut bytes = b"GGUF".to_vec();
        bytes.extend(3u32.to_le_bytes());
        bytes.extend(0u64.to_le_bytes());
        bytes.extend(metadata_count.to_le_bytes());
        bytes
    }

    fn push_key(bytes: &mut Vec<u8>, key: &str) {
        bytes.extend((key.len() as u64).to_le_bytes());
        bytes.extend(key.as_bytes());
    }

    fn push_string(bytes: &mut Vec<u8>, value: &str) {
        bytes.extend((value.len() as u64).to_le_bytes());
        bytes.extend(value.as_bytes());
    }

    #[test]
    fn reads_block_count_and_architecture() {
        let mut bytes = header(2);
        push_key(&mut bytes, "general.architecture");
        bytes.extend(TYPE_STRING.to_le_bytes());
        push_string(&mut bytes, "qwen2");
        push_key(&mut bytes, "qwen2.block_count");
        bytes.extend(TYPE_UINT32.to_le_bytes());
        bytes.extend(28u32.to_le_bytes());

        let metadata = read_metadata(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(metadata.architecture.as_deref(), Some("qwen2"));
        assert_eq!(metadata.layer_count, Some(28));
    }

    #[test]
    fn skips_arrays_before_block_count() {
        let mut bytes = header(2);
        push_key(&mut bytes, "tokenizer.ggml.tokens");
        bytes.extend(TYPE_ARRAY.to_le_bytes());
        bytes.extend(TYPE_STRING.to_le_bytes());
        bytes.extend(2u64.to_le_bytes());
        push_string(&mut bytes, "hello");
        push_string(&mut bytes, "world");
        push_key(&mut bytes, "llama.block_count");
        bytes.extend(TYPE_UINT64.to_le_bytes());
        bytes.extend(80u64.to_le_bytes());

        let metadata = read_metadata(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(metadata.layer_count, Some(80));
    }

    #[test]
    fn keeps_reading_after_block_count_for_chat_template() {
        let mut bytes = header(3);
        push_key(&mut bytes, "general.architecture");
        bytes.extend(TYPE_STRING.to_le_bytes());
        push_string(&mut bytes, "qwen3");
        push_key(&mut bytes, "qwen3.block_count");
        bytes.extend(TYPE_UINT32.to_le_bytes());
        bytes.extend(28u32.to_le_bytes());
        push_key(&mut bytes, "tokenizer.chat_template");
        bytes.extend(TYPE_STRING.to_le_bytes());
        push_string(&mut bytes, "{%- if tools %}<tool_call>{% endif %}{%- if enable_thinking %}think{% endif %}");

        let metadata = read_metadata(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(metadata.layer_count, Some(28));
        let template = metadata.chat_template.expect("chat template");
        assert!(template.contains("enable_thinking"));
    }

    #[test]
    fn rejects_non_gguf_files() {
        let error = read_metadata(&mut Cursor::new(b"not a gguf file".to_vec())).unwrap_err();
        assert!(error.contains("not a GGUF"));
    }
}