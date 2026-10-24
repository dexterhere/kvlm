// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded inspection of little-endian, single-file GGUF v3 declarations.
//! No tensor payload is read or executed. Container validation does not establish
//! model authenticity, complete model semantics, runtime compatibility, or fit.

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    collections::HashSet,
    fmt,
    fs::OpenOptions,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

const HEADER_LIMIT: u64 = 16 * 1024 * 1024;
const STRING_LIMIT: u64 = 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub struct GgufMetadata {
    pub file_bytes: u64,
    pub tensor_count: u64,
    pub tensor_bytes: u64,
    pub alignment: u32,
    pub architecture: Option<String>,
    pub llama: Option<LlamaMetadata>,
    pub warnings: Vec<LayoutWarning>,
}

/// Typed declarations only. Missing fields are not derived from other values.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LlamaMetadata {
    pub context_length: Option<u32>,
    pub block_count: Option<u32>,
    pub embedding_length: Option<u32>,
    pub attention_heads: Option<u32>,
    pub kv_heads: Option<u32>,
    pub key_length: Option<u32>,
    pub value_length: Option<u32>,
}

/// Recognized extension declarations; an empty list is not proof of full attention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutWarning {
    ExpertMetadata,
    SlidingWindowMetadata,
    RecurrentMetadata,
    HybridMetadata,
    AdapterMetadata,
    NonCausalAttention,
    NonModelType,
    TensorLayoutMetadata,
    AuxiliaryMetadata,
}

#[derive(Debug)]
pub enum GgufError {
    Io(io::Error),
    Invalid(&'static str),
    Limit(&'static str),
    Unsupported(&'static str),
}

impl From<io::Error> for GgufError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl fmt::Display for GgufError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "GGUF input could not be read: {error}"),
            Self::Invalid(reason) => write!(formatter, "Invalid GGUF: {reason}"),
            Self::Limit(reason) => write!(formatter, "GGUF inspection limit: {reason}"),
            Self::Unsupported(reason) => write!(formatter, "Unsupported GGUF: {reason}"),
        }
    }
}

impl std::error::Error for GgufError {}

/// Inspect a user-selected regular file, including symlinks to regular cache blobs.
/// Unix nonblocking open prevents a FIFO path from hanging before the type check.
/// This synchronous function belongs in spawn_blocking when called from a UI.
pub fn inspect_file(path: &Path) -> Result<GgufMetadata, GgufError> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(GgufError::Invalid("input must be a regular file"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let mut file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(GgufError::Invalid("input must be a regular file"));
    }
    inspect_reader(&mut file)
}

/// Reset a seekable reader to its start and inspect only header/metadata/directory.
/// The reader must report its real length. Gaps and trailing data are permitted;
/// explicit split metadata, nested arrays and unrecognized tensor types are rejected.
pub fn inspect_reader<R: Read + Seek>(reader: &mut R) -> Result<GgufMetadata, GgufError> {
    let file_bytes = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(0))?;
    let mut input = Input {
        reader,
        length: file_bytes,
        position: 0,
    };
    if input.fixed::<4>()? != *b"GGUF" {
        return Err(GgufError::Invalid("magic"));
    }
    if input.u32()? != 3 {
        return Err(GgufError::Unsupported(
            "only little-endian version 3 is supported",
        ));
    }
    let tensor_count = input.u64()?;
    let entry_count = input.u64()?;
    if tensor_count > 65_536 || entry_count > 8192 {
        return Err(GgufError::Limit("tensor or metadata count"));
    }
    let mut keys = HashSet::new();
    let mut architecture = None;
    let mut alignment = 32;
    let mut llama = LlamaMetadata::default();
    let mut warnings = Vec::new();
    for _ in 0..entry_count {
        let key = input.string(256)?;
        if key.is_empty() || !keys.insert(key.clone()) {
            return Err(GgufError::Invalid("empty or duplicate metadata key"));
        }
        if key.starts_with("split.") {
            return Err(GgufError::Unsupported("split metadata"));
        }
        let value_type = input.u32()?;
        let value = input.value(value_type)?;
        match key.as_str() {
            "general.architecture" => {
                let Value::Text(name) = value else {
                    return Err(GgufError::Invalid("architecture type"));
                };
                if name.is_empty() || name.len() > 128 {
                    return Err(GgufError::Invalid("architecture length"));
                }
                architecture = Some(name);
            }
            "general.alignment" => {
                let Value::U32(bytes) = value else {
                    return Err(GgufError::Invalid("alignment type"));
                };
                if bytes < 8 || !bytes.is_power_of_two() || bytes > STRING_LIMIT as u32 {
                    return Err(GgufError::Invalid("alignment"));
                }
                alignment = bytes;
            }
            "llama.context_length" => llama.context_length = Some(positive(value)?),
            "llama.block_count" => llama.block_count = Some(positive(value)?),
            "llama.embedding_length" => llama.embedding_length = Some(positive(value)?),
            "llama.attention.head_count" => llama.attention_heads = Some(positive(value)?),
            "llama.attention.head_count_kv" => llama.kv_heads = Some(positive(value)?),
            "llama.attention.key_length" => llama.key_length = Some(positive(value)?),
            "llama.attention.value_length" => llama.value_length = Some(positive(value)?),
            "general.type" => {
                let Value::Text(kind) = value else {
                    return Err(GgufError::Invalid("model type"));
                };
                if kind != "model" {
                    warn(&mut warnings, LayoutWarning::NonModelType);
                }
            }
            "llama.attention.causal" => {
                let Value::Bool(causal) = value else {
                    return Err(GgufError::Invalid("causal attention type"));
                };
                if !causal {
                    warn(&mut warnings, LayoutWarning::NonCausalAttention);
                }
            }
            _ => {}
        }
        if key.starts_with("llama.expert")
            || key.starts_with("llama.moe_")
            || key.starts_with("llama.attention.value_expert")
            || key == "llama.interleave_moe_layer_step"
        {
            warn(&mut warnings, LayoutWarning::ExpertMetadata);
        }
        if key.starts_with("llama.attention.sliding_window") {
            warn(&mut warnings, LayoutWarning::SlidingWindowMetadata);
        }
        if key.starts_with("llama.ssm.")
            || key.starts_with("llama.recurrent.")
            || key == "llama.attention.recurrent_layers"
        {
            warn(&mut warnings, LayoutWarning::RecurrentMetadata);
        }
        if key == "llama.full_attention_interval" {
            warn(&mut warnings, LayoutWarning::HybridMetadata);
        }
        if key.starts_with("llama.adapters.") || key.starts_with("adapter.") {
            warn(&mut warnings, LayoutWarning::AdapterMetadata);
        }
        if key == "llama.tensor_data_layout" {
            warn(&mut warnings, LayoutWarning::TensorLayoutMetadata);
        }
        if key.starts_with("clip.") || key.starts_with("vision.") || key.starts_with("mtp.") {
            warn(&mut warnings, LayoutWarning::AuxiliaryMetadata);
        }
    }

    let mut names = HashSet::new();
    let mut ranges = Vec::new();
    let mut tensor_bytes = 0_u64;
    for _ in 0..tensor_count {
        let name = input.string(64)?;
        if name.is_empty() || !names.insert(name) {
            return Err(GgufError::Invalid("empty or duplicate tensor name"));
        }
        let dimensions = input.u32()?;
        if !(1..=4).contains(&dimensions) {
            return Err(GgufError::Unsupported("tensor dimensions"));
        }
        let mut elements = 1_u64;
        let mut first_dimension = 0;
        for dimension in 0..dimensions {
            let size = input.u64()?;
            if size == 0 || size > i64::MAX as u64 {
                return Err(GgufError::Invalid("tensor dimension"));
            }
            if dimension == 0 {
                first_dimension = size;
            }
            elements = elements
                .checked_mul(size)
                .ok_or(GgufError::Invalid("tensor size overflow"))?;
        }
        let (block_elements, block_bytes) = tensor_type(input.u32()?)?;
        if !first_dimension.is_multiple_of(block_elements) {
            return Err(GgufError::Invalid("quantized tensor row alignment"));
        }
        let bytes = (elements / block_elements)
            .checked_mul(block_bytes)
            .ok_or(GgufError::Invalid("tensor byte size overflow"))?;
        tensor_bytes = tensor_bytes
            .checked_add(bytes)
            .ok_or(GgufError::Invalid("tensor byte sum overflow"))?;
        let offset = input.u64()?;
        if !offset.is_multiple_of(u64::from(alignment)) {
            return Err(GgufError::Invalid("tensor offset alignment"));
        }
        let end = offset
            .checked_add(bytes)
            .ok_or(GgufError::Invalid("tensor range overflow"))?;
        ranges.push((offset, end));
    }
    let mask = u64::from(alignment) - 1;
    let data_start = input
        .position
        .checked_add(mask)
        .ok_or(GgufError::Invalid("data alignment overflow"))?
        & !mask;
    ranges.sort_unstable();
    let mut previous_end = 0;
    for (start, end) in ranges {
        if start < previous_end
            || data_start
                .checked_add(end)
                .is_none_or(|end| end > file_bytes)
        {
            return Err(GgufError::Invalid("overlapping or truncated tensor range"));
        }
        previous_end = end;
    }
    let llama = (architecture.as_deref() == Some("llama")).then_some(llama);
    Ok(GgufMetadata {
        file_bytes,
        tensor_count,
        tensor_bytes,
        alignment,
        architecture,
        llama,
        warnings,
    })
}

fn warn(warnings: &mut Vec<LayoutWarning>, warning: LayoutWarning) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

fn positive(value: Value) -> Result<u32, GgufError> {
    match value {
        Value::U32(value) if value > 0 => Ok(value),
        _ => Err(GgufError::Invalid("architecture scalar type or zero value")),
    }
}

fn tensor_type(kind: u32) -> Result<(u64, u64), GgufError> {
    // shortcut: common storage types only; extend with upstream-verified block tests.
    match kind {
        0 => Ok((1, 4)),
        1 | 30 => Ok((1, 2)),
        2 => Ok((32, 18)),
        3 => Ok((32, 20)),
        6 => Ok((32, 22)),
        7 => Ok((32, 24)),
        8 => Ok((32, 34)),
        10 => Ok((256, 84)),
        11 => Ok((256, 110)),
        12 => Ok((256, 144)),
        13 => Ok((256, 176)),
        14 => Ok((256, 210)),
        _ => Err(GgufError::Unsupported("tensor type")),
    }
}

enum Value {
    U32(u32),
    Bool(bool),
    Text(String),
    Other,
}

struct Input<'a, R> {
    reader: &'a mut R,
    length: u64,
    position: u64,
}

impl<R: Read + Seek> Input<'_, R> {
    fn advance(&mut self, bytes: u64) -> Result<(), GgufError> {
        let end = self
            .position
            .checked_add(bytes)
            .ok_or(GgufError::Limit("header byte count"))?;
        if end > HEADER_LIMIT {
            return Err(GgufError::Limit("header and tensor directory bytes"));
        }
        if end > self.length {
            return Err(GgufError::Invalid("truncated header or directory"));
        }
        self.position = end;
        Ok(())
    }

    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], GgufError> {
        self.advance(N as u64)?;
        let mut bytes = [0; N];
        self.reader.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<u32, GgufError> {
        Ok(u32::from_le_bytes(self.fixed()?))
    }
    fn u64(&mut self) -> Result<u64, GgufError> {
        Ok(u64::from_le_bytes(self.fixed()?))
    }

    fn skip(&mut self, bytes: u64) -> Result<(), GgufError> {
        self.advance(bytes)?;
        self.reader.seek(SeekFrom::Start(self.position))?;
        Ok(())
    }

    fn string(&mut self, limit: u64) -> Result<String, GgufError> {
        let length = self.u64()?;
        if length > limit {
            return Err(GgufError::Limit("string bytes"));
        }
        self.advance(length)?;
        let mut bytes = vec![0; length as usize];
        self.reader.read_exact(&mut bytes)?;
        String::from_utf8(bytes).map_err(|_| GgufError::Invalid("UTF-8 string"))
    }

    fn value(&mut self, kind: u32) -> Result<Value, GgufError> {
        match kind {
            4 => Ok(Value::U32(self.u32()?)),
            7 => match self.fixed::<1>()?[0] {
                0 => Ok(Value::Bool(false)),
                1 => Ok(Value::Bool(true)),
                _ => Err(GgufError::Invalid("boolean value")),
            },
            8 => Ok(Value::Text(self.string(STRING_LIMIT)?)),
            9 => {
                let element = self.u32()?;
                let count = self.u64()?;
                if count > 1_000_000 {
                    return Err(GgufError::Limit("array elements"));
                }
                if element == 9 {
                    return Err(GgufError::Unsupported("nested metadata arrays"));
                }
                let width = scalar_width(element)?;
                if element == 8 || element == 7 {
                    for _ in 0..count {
                        self.value(element)?;
                    }
                } else {
                    self.skip(
                        count
                            .checked_mul(width)
                            .ok_or(GgufError::Limit("array byte count"))?,
                    )?;
                }
                Ok(Value::Other)
            }
            _ => {
                self.skip(scalar_width(kind)?)?;
                Ok(Value::Other)
            }
        }
    }
}

fn scalar_width(kind: u32) -> Result<u64, GgufError> {
    match kind {
        0 | 1 | 7 => Ok(1),
        2 | 3 => Ok(2),
        4..=6 => Ok(4),
        8 | 10..=12 => Ok(8),
        _ => Err(GgufError::Invalid("metadata value type")),
    }
}
