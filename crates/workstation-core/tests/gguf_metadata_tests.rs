// SPDX-License-Identifier: GPL-3.0-or-later

use std::{
    fs,
    io::{self, Cursor, Read, Seek, SeekFrom},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use workstation_core::models::gguf::{GgufError, LayoutWarning, inspect_file, inspect_reader};

const GGUF: &[u8; 4] = b"GGUF";
const VERSION: u32 = 3;
const TYPE_U32: u32 = 4;
const TYPE_BOOL: u32 = 7;
const TYPE_STRING: u32 = 8;
const TYPE_ARRAY: u32 = 9;

fn u32(value: u32, output: &mut Vec<u8>) {
    output.extend(value.to_le_bytes());
}

fn u64(value: u64, output: &mut Vec<u8>) {
    output.extend(value.to_le_bytes());
}

fn string(bytes: &[u8], output: &mut Vec<u8>) {
    u64(bytes.len() as u64, output);
    output.extend(bytes);
}

fn metadata(key: &[u8], value_type: u32, value: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    string(key, &mut output);
    u32(value_type, &mut output);
    output.extend(value);
    output
}

fn u32_value(value: u32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

fn bool_value(value: u8) -> Vec<u8> {
    vec![value]
}

fn string_value(value: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    string(value, &mut output);
    output
}

fn array_value(element_type: u32, count: u64, elements: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    u32(element_type, &mut output);
    u64(count, &mut output);
    output.extend(elements);
    output
}

fn header(metadata: &[Vec<u8>], tensors: &[Vec<u8>]) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend(GGUF);
    u32(VERSION, &mut output);
    u64(tensors.len() as u64, &mut output);
    u64(metadata.len() as u64, &mut output);
    for item in metadata {
        output.extend(item);
    }
    for item in tensors {
        output.extend(item);
    }
    output
}

fn tensor(name: &[u8], dims: &[u64], type_id: u32, offset: u64) -> Vec<u8> {
    let mut output = Vec::new();
    string(name, &mut output);
    u32(dims.len() as u32, &mut output);
    for dimension in dims {
        u64(*dimension, &mut output);
    }
    u32(type_id, &mut output);
    u64(offset, &mut output);
    output
}

fn aligned(mut output: Vec<u8>, alignment: usize) -> Vec<u8> {
    output.resize(output.len().div_ceil(alignment) * alignment, 0);
    output
}

fn one_f32_tensor() -> Vec<u8> {
    let mut output = aligned(header(&[], &[tensor(b"weight", &[1], 0, 0)]), 32);
    output.extend(0f32.to_le_bytes());
    output
}

fn one_block_tensor(type_id: u32, block_elements: u64, block_bytes: usize) -> Vec<u8> {
    let mut output = aligned(
        header(&[], &[tensor(b"weight", &[block_elements], type_id, 0)]),
        32,
    );
    output.resize(output.len() + block_bytes, 0);
    output
}

fn invalid(input: &[u8]) {
    assert!(matches!(
        inspect_reader(&mut Cursor::new(input)),
        Err(GgufError::Invalid(_))
    ));
}

fn limited(input: &[u8]) {
    assert!(matches!(
        inspect_reader(&mut Cursor::new(input)),
        Err(GgufError::Limit(_))
    ));
}

fn unsupported(input: &[u8]) {
    assert!(matches!(
        inspect_reader(&mut Cursor::new(input)),
        Err(GgufError::Unsupported(_))
    ));
}

#[test]
fn accepts_minimal_header_and_unknown_architecture_without_fit_claims() {
    let empty = header(&[], &[]);
    let empty_result = inspect_reader(&mut Cursor::new(empty)).unwrap();
    assert_eq!(empty_result.file_bytes, 24);
    assert_eq!(empty_result.tensor_count, 0);
    assert_eq!(empty_result.tensor_bytes, 0);
    assert_eq!(empty_result.alignment, 32);
    assert_eq!(empty_result.architecture, None);
    assert_eq!(empty_result.llama, None);

    let unknown = header(
        &[metadata(
            b"general.architecture",
            TYPE_STRING,
            &string_value(b"future-net"),
        )],
        &[],
    );
    let unknown_result = inspect_reader(&mut Cursor::new(unknown)).unwrap();
    assert_eq!(unknown_result.architecture.as_deref(), Some("future-net"));
    assert_eq!(unknown_result.llama, None);
}

#[test]
fn retains_positive_llama_declarations_regardless_of_metadata_order() {
    let input = header(
        &[
            metadata(b"llama.block_count", TYPE_U32, &u32_value(32)),
            metadata(b"llama.attention.head_count", TYPE_U32, &u32_value(32)),
            metadata(b"llama.attention.head_count_kv", TYPE_U32, &u32_value(8)),
            metadata(b"llama.embedding_length", TYPE_U32, &u32_value(4096)),
            metadata(b"llama.context_length", TYPE_U32, &u32_value(8192)),
            metadata(
                b"general.architecture",
                TYPE_STRING,
                &string_value(b"llama"),
            ),
        ],
        &[],
    );
    let result = inspect_reader(&mut Cursor::new(input)).unwrap();
    let llama = result.llama.unwrap();
    assert_eq!(llama.context_length, Some(8192));
    assert_eq!(llama.block_count, Some(32));
    assert_eq!(llama.embedding_length, Some(4096));
    assert_eq!(llama.attention_heads, Some(32));
    assert_eq!(llama.kv_heads, Some(8));
    assert_eq!(llama.key_length, None);
    assert_eq!(llama.value_length, None);
}

#[test]
fn rejects_bad_fixed_fields_and_all_prefixes_of_a_complete_fixture() {
    let valid = one_f32_tensor();
    for prefix in 0..valid.len() {
        assert!(
            inspect_reader(&mut Cursor::new(&valid[..prefix])).is_err(),
            "prefix {prefix}"
        );
    }

    let mut wrong_magic = header(&[], &[]);
    wrong_magic[0] = b'B';
    invalid(&wrong_magic);
    let mut unsupported_version = header(&[], &[]);
    unsupported_version[4..8].copy_from_slice(&2u32.to_le_bytes());
    unsupported(&unsupported_version);

    let mut excessive_kvs = header(&[], &[]);
    excessive_kvs[16..24].copy_from_slice(&8193u64.to_le_bytes());
    limited(&excessive_kvs);
    let mut excessive_tensors = header(&[], &[]);
    excessive_tensors[8..16].copy_from_slice(&65537u64.to_le_bytes());
    limited(&excessive_tensors);
}

#[test]
fn rejects_hostile_metadata_shapes_and_values() {
    invalid(&header(&[metadata(b"", TYPE_U32, &u32_value(1))], &[]));
    invalid(&header(
        &[metadata(
            b"general.name",
            TYPE_STRING,
            &string_value(&[0xff]),
        )],
        &[],
    ));
    invalid(&header(
        &[metadata(b"flag", TYPE_BOOL, &bool_value(2))],
        &[],
    ));
    invalid(&header(
        &[metadata(b"llama.context_length", TYPE_U32, &u32_value(0))],
        &[],
    ));
    invalid(&header(
        &[metadata(
            b"llama.context_length",
            TYPE_STRING,
            &string_value(b"4096"),
        )],
        &[],
    ));
    invalid(&header(
        &[
            metadata(
                b"general.architecture",
                TYPE_STRING,
                &string_value(b"llama"),
            ),
            metadata(
                b"general.architecture",
                TYPE_STRING,
                &string_value(b"llama"),
            ),
        ],
        &[],
    ));

    let mut huge_array = Vec::new();
    u32(TYPE_U32, &mut huge_array);
    u64(1_000_001, &mut huge_array);
    limited(&header(
        &[metadata(b"tokens", TYPE_ARRAY, &huge_array)],
        &[],
    ));

    let mut nested_array = Vec::new();
    u32(TYPE_ARRAY, &mut nested_array);
    u64(1, &mut nested_array);
    unsupported(&header(
        &[metadata(b"nested", TYPE_ARRAY, &nested_array)],
        &[],
    ));

    unsupported(&header(
        &[metadata(b"split.count", TYPE_U32, &u32_value(2))],
        &[],
    ));
}

#[test]
fn accepts_flat_metadata_arrays_and_rejects_bad_array_elements() {
    let names = [string_value(b"one"), string_value(b"two")].concat();
    let input = header(
        &[
            metadata(b"empty", TYPE_ARRAY, &array_value(TYPE_U32, 0, &[])),
            metadata(
                b"numbers",
                TYPE_ARRAY,
                &array_value(TYPE_U32, 2, &[1, 0, 0, 0, 2, 0, 0, 0]),
            ),
            metadata(b"flags", TYPE_ARRAY, &array_value(TYPE_BOOL, 2, &[0, 1])),
            metadata(b"names", TYPE_ARRAY, &array_value(TYPE_STRING, 2, &names)),
        ],
        &[],
    );
    assert!(inspect_reader(&mut Cursor::new(input)).is_ok());

    invalid(&header(
        &[metadata(b"bad", TYPE_ARRAY, &array_value(99, 0, &[]))],
        &[],
    ));
    invalid(&header(
        &[metadata(
            b"bad",
            TYPE_ARRAY,
            &array_value(TYPE_BOOL, 1, &[2]),
        )],
        &[],
    ));
}

#[test]
fn enforces_header_directory_cap_before_large_array_payloads() {
    let elements = vec![0; 4_000_000];
    let array = array_value(TYPE_U32, 1_000_000, &elements);
    let input = header(
        &[
            metadata(b"one", TYPE_ARRAY, &array),
            metadata(b"two", TYPE_ARRAY, &array),
            metadata(b"three", TYPE_ARRAY, &array),
            metadata(b"four", TYPE_ARRAY, &array),
            metadata(b"five", TYPE_ARRAY, &array),
        ],
        &[],
    );
    assert!(matches!(
        inspect_reader(&mut Cursor::new(input)),
        Err(GgufError::Limit("header and tensor directory bytes"))
    ));
}

#[test]
fn validates_alignment_and_tensor_layout_without_reading_payload() {
    let valid = one_f32_tensor();
    let result = inspect_reader(&mut Cursor::new(valid.clone())).unwrap();
    assert_eq!(result.tensor_count, 1);
    assert_eq!(result.tensor_bytes, 4);

    let payload_start = valid.len() as u64 - 4;
    let mut source = PayloadGuard::new(valid, payload_start);
    let result = inspect_reader(&mut source).unwrap();
    assert_eq!(result.tensor_bytes, 4);
    assert!(!source.read_payload);

    invalid(&header(
        &[metadata(b"general.alignment", TYPE_U32, &u32_value(12))],
        &[],
    ));
    invalid(&header(
        &[metadata(b"general.alignment", TYPE_U32, &u32_value(4))],
        &[],
    ));
    invalid(&header(
        &[metadata(
            b"general.alignment",
            TYPE_U32,
            &u32_value(1_048_577),
        )],
        &[],
    ));

    let misaligned = aligned(header(&[], &[tensor(b"weight", &[1], 0, 1)]), 32);
    invalid(&misaligned);
    let past_eof = aligned(header(&[], &[tensor(b"weight", &[2], 0, 0)]), 32);
    invalid(&past_eof);
    let mut overlap = aligned(
        header(
            &[],
            &[tensor(b"one", &[1], 0, 0), tensor(b"two", &[1], 0, 0)],
        ),
        32,
    );
    overlap.extend(0f32.to_le_bytes());
    invalid(&overlap);
    unsupported(&aligned(header(&[], &[tensor(b"weight", &[], 0, 0)]), 32));
    invalid(&aligned(header(&[], &[tensor(b"weight", &[0], 0, 0)]), 32));
    unsupported(&aligned(header(&[], &[tensor(b"weight", &[1], 99, 0)]), 32));
}

#[test]
fn validates_every_supported_tensor_block_and_tensor_arithmetic() {
    for (type_id, elements, bytes) in [
        (0, 1, 4),
        (1, 1, 2),
        (2, 32, 18),
        (3, 32, 20),
        (6, 32, 22),
        (7, 32, 24),
        (8, 32, 34),
        (10, 256, 84),
        (11, 256, 110),
        (12, 256, 144),
        (13, 256, 176),
        (14, 256, 210),
        (30, 1, 2),
    ] {
        let metadata =
            inspect_reader(&mut Cursor::new(one_block_tensor(type_id, elements, bytes))).unwrap();
        assert_eq!(metadata.tensor_bytes, bytes as u64, "type {type_id}");
    }

    invalid(&one_block_tensor(2, 1, 18));
    invalid(&aligned(
        header(&[], &[tensor(b"overflow", &[i64::MAX as u64, 2], 0, 0)]),
        32,
    ));
    invalid(&aligned(
        header(&[], &[tensor(b"product", &[i64::MAX as u64, 3], 0, 0)]),
        32,
    ));
}

#[test]
fn emits_conservative_layout_warnings() {
    let input = header(
        &[
            metadata(b"llama.expert_count", TYPE_U32, &u32_value(8)),
            metadata(
                b"llama.attention.sliding_window",
                TYPE_U32,
                &u32_value(4096),
            ),
            metadata(b"llama.attention.causal", TYPE_BOOL, &bool_value(0)),
            metadata(b"general.type", TYPE_STRING, &string_value(b"adapter")),
            metadata(b"llama.ssm.convolution", TYPE_U32, &u32_value(1)),
            metadata(
                b"llama.tensor_data_layout",
                TYPE_STRING,
                &string_value(b"separate"),
            ),
            metadata(b"clip.projector_type", TYPE_STRING, &string_value(b"mlp")),
        ],
        &[],
    );
    let warnings = inspect_reader(&mut Cursor::new(input)).unwrap().warnings;
    assert!(warnings.contains(&LayoutWarning::ExpertMetadata));
    assert!(warnings.contains(&LayoutWarning::SlidingWindowMetadata));
    assert!(warnings.contains(&LayoutWarning::NonCausalAttention));
    assert!(warnings.contains(&LayoutWarning::NonModelType));
    assert!(warnings.contains(&LayoutWarning::RecurrentMetadata));
    assert!(warnings.contains(&LayoutWarning::TensorLayoutMetadata));
    assert!(warnings.contains(&LayoutWarning::AuxiliaryMetadata));
}

#[test]
fn recognizes_new_layout_markers_in_isolation() {
    for (key, warning) in [
        (
            b"llama.attention.recurrent_layers".as_slice(),
            LayoutWarning::RecurrentMetadata,
        ),
        (
            b"llama.full_attention_interval".as_slice(),
            LayoutWarning::HybridMetadata,
        ),
        (
            b"llama.adapters.lora_rank".as_slice(),
            LayoutWarning::AdapterMetadata,
        ),
        (b"adapter.type".as_slice(), LayoutWarning::AdapterMetadata),
        (
            b"llama.moe_every_n_layers".as_slice(),
            LayoutWarning::ExpertMetadata,
        ),
        (
            b"llama.experts_per_group".as_slice(),
            LayoutWarning::ExpertMetadata,
        ),
        (
            b"llama.attention.value_expert_count".as_slice(),
            LayoutWarning::ExpertMetadata,
        ),
        (
            b"llama.interleave_moe_layer_step".as_slice(),
            LayoutWarning::ExpertMetadata,
        ),
    ] {
        let input = header(&[metadata(key, TYPE_U32, &u32_value(1))], &[]);
        assert_eq!(
            inspect_reader(&mut Cursor::new(input)).unwrap().warnings,
            vec![warning],
            "{key:?}"
        );
    }
}

#[test]
fn inspect_file_matches_reader_for_a_regular_file() {
    let path = unique_test_path();
    fs::write(&path, header(&[], &[])).unwrap();
    let result = inspect_file(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert_eq!(result.file_bytes, 24);
    assert_eq!(result.tensor_count, 0);

    let directory = unique_test_path();
    fs::create_dir(&directory).unwrap();
    assert!(matches!(
        inspect_file(&directory),
        Err(GgufError::Invalid(_))
    ));
    fs::remove_dir(&directory).unwrap();

    #[cfg(unix)]
    {
        let target = unique_test_path();
        let link = target.with_extension("link");
        fs::write(&target, header(&[], &[])).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(inspect_file(&link).unwrap().file_bytes, 24);
        fs::remove_file(&link).unwrap();
        fs::remove_file(&target).unwrap();
    }
}

fn unique_test_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "kvlm-gguf-test-{}-{nonce}.gguf",
        std::process::id()
    ))
}

struct PayloadGuard {
    inner: Cursor<Vec<u8>>,
    payload_start: u64,
    read_payload: bool,
}

impl PayloadGuard {
    fn new(bytes: Vec<u8>, payload_start: u64) -> Self {
        Self {
            inner: Cursor::new(bytes),
            payload_start,
            read_payload: false,
        }
    }
}

impl Read for PayloadGuard {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let start = self.inner.position();
        let read = self.inner.read(buffer)?;
        if start + read as u64 > self.payload_start {
            self.read_payload = true;
        }
        Ok(read)
    }
}

impl Seek for PayloadGuard {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}
