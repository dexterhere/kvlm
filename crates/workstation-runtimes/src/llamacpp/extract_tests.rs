// SPDX-License-Identifier: GPL-3.0-or-later

use super::extract::{ExtractError, LIMITS, Limits, extract_tar_gz};
use flate2::{Compression, write::GzEncoder};
use std::{
    fs,
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tar::{Builder, EntryType, Header};

fn no() -> bool {
    false
}
const NO: &(dyn Fn() -> bool + Sync) = &no;
static NEXT: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Dir(pub PathBuf);
impl Dir {
    pub fn new() -> Self {
        let path = PathBuf::from(format!(
            "/tmp/kvlm-extract-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) enum Item {
    Dir(&'static str),
    File(&'static str, Vec<u8>, u32),
    Link(&'static str, &'static str),
    Hard(&'static str, &'static str),
    Device(&'static str),
}

/// Raw header so hostile names (`..`, absolute) can be written; the builder refuses them.
fn header(name: &str, kind: EntryType, size: u64, mode: u32) -> Header {
    let mut h = Header::new_gnu();
    h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
    h.set_entry_type(kind);
    h.set_size(size);
    h.set_mode(mode);
    h
}

pub(crate) fn tgz(items: &[Item]) -> Vec<u8> {
    let mut builder = Builder::new(Vec::new());
    for item in items {
        match item {
            Item::Dir(name) => {
                let mut h = header(name, EntryType::Directory, 0, 0o755);
                h.set_cksum();
                builder.append(&h, std::io::empty()).unwrap();
            }
            Item::File(name, body, mode) => {
                let mut h = header(name, EntryType::Regular, body.len() as u64, *mode);
                h.set_cksum();
                builder.append(&h, body.as_slice()).unwrap();
            }
            Item::Link(name, target) | Item::Hard(name, target) => {
                let kind = if matches!(item, Item::Link(..)) {
                    EntryType::Symlink
                } else {
                    EntryType::Link
                };
                let mut h = header(name, kind, 0, 0o777);
                h.as_old_mut().linkname[..target.len()].copy_from_slice(target.as_bytes());
                h.set_cksum();
                builder.append(&h, std::io::empty()).unwrap();
            }
            Item::Device(name) => {
                let mut h = header(name, EntryType::Char, 0, 0o600);
                h.set_cksum();
                builder.append(&h, std::io::empty()).unwrap();
            }
        }
    }
    let tar = builder.into_inner().unwrap();
    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    gz.write_all(&tar).unwrap();
    gz.finish().unwrap()
}

fn run(items: &[Item]) -> (Dir, Result<super::extract::Summary, ExtractError>) {
    let dir = Dir::new();
    let dest = dir.0.join("out");
    fs::create_dir(&dest).unwrap();
    let result = extract_tar_gz(tgz(items).as_slice(), &dest, LIMITS, NO);
    (dir, result)
}

fn refused(items: &[Item]) -> Dir {
    let (dir, result) = run(items);
    assert!(matches!(result, Err(ExtractError::Unsafe(_))), "{result:?}");
    // Nothing may appear next to the destination, whatever the archive asked for.
    let siblings: Vec<_> = fs::read_dir(&dir.0).unwrap().flatten().collect();
    assert_eq!(siblings.len(), 1);
    dir
}

#[test]
fn a_normal_build_extracts_with_modes_and_sibling_links() {
    let (dir, result) = run(&[
        Item::Dir("llama-b1/"),
        Item::File("llama-b1/llama-server", b"#!/bin/sh\n".to_vec(), 0o755),
        Item::File("llama-b1/LICENSE", b"MIT".to_vec(), 0o644),
        Item::File("llama-b1/libx.so.0.1", b"lib".to_vec(), 0o755),
        Item::Link("llama-b1/libx.so.0", "libx.so.0.1"),
        Item::Link("llama-b1/libx.so", "libx.so.0"),
    ]);
    assert_eq!(result.unwrap().entries, 6);
    let out = dir.0.join("out");
    let mode = |n: &str| fs::metadata(out.join(n)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode("llama-server"), 0o700);
    assert_eq!(mode("LICENSE"), 0o600);
    assert_eq!(fs::read(out.join("libx.so")).unwrap(), b"lib");
    assert!(
        fs::symlink_metadata(out.join("libx.so"))
            .unwrap()
            .is_symlink()
    );
}

#[test]
fn escaping_paths_are_refused_and_create_nothing_outside() {
    refused(&[Item::File("llama-b1/../evil", b"x".to_vec(), 0o644)]);
    refused(&[Item::File("/etc/evil", b"x".to_vec(), 0o644)]);
    refused(&[Item::File("llama-b1/a/../../evil", b"x".to_vec(), 0o644)]);
    refused(&[Item::File("llama-b1/sp ace", b"x".to_vec(), 0o644)]);
    refused(&[Item::File("llama-b1/-flag", b"x".to_vec(), 0o644)]);
}

#[test]
fn links_must_be_sibling_files_that_exist() {
    let file = || Item::File("llama-b1/real", b"x".to_vec(), 0o644);
    refused(&[file(), Item::Link("llama-b1/l", "/etc/passwd")]);
    refused(&[file(), Item::Link("llama-b1/l", "../real")]);
    refused(&[file(), Item::Link("llama-b1/l", "sub/real")]);
    refused(&[file(), Item::Link("llama-b1/l", "missing")]);
    refused(&[file(), Item::Link("llama-b1/l", "l")]);
    refused(&[
        file(),
        Item::Link("llama-b1/a", "b"),
        Item::Link("llama-b1/b", "a"),
    ]);
}

#[test]
fn hard_links_devices_and_duplicates_are_refused() {
    refused(&[
        Item::File("llama-b1/real", b"x".to_vec(), 0o644),
        Item::Hard("llama-b1/h", "real"),
    ]);
    refused(&[Item::Device("llama-b1/null")]);
    refused(&[
        Item::File("llama-b1/a", b"1".to_vec(), 0o644),
        Item::File("llama-b1/a", b"2".to_vec(), 0o644),
    ]);
    refused(&[
        Item::File("llama-b1/a", b"1".to_vec(), 0o644),
        Item::Link("llama-b1/a", "a"),
    ]);
}

#[test]
fn layout_must_be_one_top_level_directory() {
    refused(&[
        Item::File("one/a", b"1".to_vec(), 0o644),
        Item::File("two/b", b"2".to_vec(), 0o644),
    ]);
    refused(&[Item::File("loose-file", b"1".to_vec(), 0o644)]);
}

#[test]
fn an_existing_link_at_the_destination_is_never_written_through() {
    let dir = Dir::new();
    let dest = dir.0.join("out");
    fs::create_dir(&dest).unwrap();
    let victim = dir.0.join("victim");
    fs::write(&victim, b"precious").unwrap();
    symlink(&victim, dest.join("llama-server")).unwrap();
    let archive = tgz(&[Item::File("llama-b1/llama-server", b"EVIL".to_vec(), 0o755)]);
    let result = extract_tar_gz(archive.as_slice(), &dest, LIMITS, NO);
    assert!(matches!(result, Err(ExtractError::Unsafe(_))));
    assert_eq!(fs::read(&victim).unwrap(), b"precious");
}

#[test]
fn entry_and_decompressed_byte_caps_stop_the_extraction() {
    let many: Vec<Item> = (0..5).map(|_| Item::Dir("llama-b1/d/")).collect();
    let dir = Dir::new();
    let dest = dir.0.join("out");
    fs::create_dir(&dest).unwrap();
    let tight = Limits {
        max_entries: 3,
        max_bytes: LIMITS.max_bytes,
    };
    assert_eq!(
        extract_tar_gz(tgz(&many).as_slice(), &dest, tight, NO),
        Err(ExtractError::TooManyEntries)
    );

    // A few KiB of gzip that inflates past the cap: stopped by the decompressed count.
    let bomb = tgz(&[Item::File("llama-b1/zeros", vec![0u8; 4 << 20], 0o644)]);
    assert!(bomb.len() < 64 << 10);
    let dest2 = dir.0.join("out2");
    fs::create_dir(&dest2).unwrap();
    let tight = Limits {
        max_entries: 10,
        max_bytes: 1 << 20,
    };
    assert_eq!(
        extract_tar_gz(bomb.as_slice(), &dest2, tight, NO),
        Err(ExtractError::TooLarge)
    );
    assert!(fs::metadata(dest2.join("zeros")).map_or(0, |m| m.len()) <= 1 << 20);
}

#[test]
fn garbage_is_corrupt_not_a_panic() {
    let dir = Dir::new();
    let dest = dir.0.join("out");
    fs::create_dir(&dest).unwrap();
    assert_eq!(
        extract_tar_gz(&b"this is not gzip"[..], &dest, LIMITS, NO),
        Err(ExtractError::Corrupt)
    );
    let mut cut = tgz(&[Item::File("llama-b1/a", vec![7u8; 100_000], 0o644)]);
    cut.truncate(cut.len() / 2);
    assert!(extract_tar_gz(cut.as_slice(), &dest, LIMITS, NO).is_err());
}

#[test]
fn extended_headers_and_oversized_long_names_are_refused_before_buffering() {
    let dir = Dir::new();
    let dest = dir.0.join("out");
    fs::create_dir(&dest).unwrap();

    let mut builder = Builder::new(Vec::new());
    builder
        .append_pax_extensions([("comment", &b"hello"[..])])
        .unwrap();
    let mut h = header("llama-b1/a", EntryType::Regular, 1, 0o644);
    h.set_cksum();
    builder.append(&h, &b"x"[..]).unwrap();
    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    gz.write_all(&builder.into_inner().unwrap()).unwrap();
    let pax = gz.finish().unwrap();
    assert_eq!(
        extract_tar_gz(pax.as_slice(), &dest, LIMITS, NO),
        Err(ExtractError::Unsafe("extended tar headers"))
    );

    let mut builder = Builder::new(Vec::new());
    let mut h = Header::new_gnu();
    h.set_entry_type(EntryType::Regular);
    h.set_size(1);
    h.set_mode(0o644);
    builder
        .append_data(&mut h, format!("llama-b1/{}", "a".repeat(5000)), &b"x"[..])
        .unwrap();
    let mut gz = GzEncoder::new(Vec::new(), Compression::fast());
    gz.write_all(&builder.into_inner().unwrap()).unwrap();
    let long = gz.finish().unwrap();
    assert_eq!(
        extract_tar_gz(long.as_slice(), &dest, LIMITS, NO),
        Err(ExtractError::Unsafe("oversized long name"))
    );
    assert!(fs::read_dir(&dest).unwrap().next().is_none());
}

#[test]
fn cancellation_stops_extraction_and_usage_is_reported_for_shared_caps() {
    let dir = Dir::new();
    let dest = dir.0.join("out");
    fs::create_dir(&dest).unwrap();
    let archive = tgz(&[Item::File("llama-b1/a", vec![1u8; 100_000], 0o644)]);
    let stop = || true;
    assert_eq!(
        extract_tar_gz(archive.as_slice(), &dest, LIMITS, &stop),
        Err(ExtractError::Cancelled)
    );
    let used = extract_tar_gz(archive.as_slice(), &dest, LIMITS, NO).unwrap();
    assert!(used.bytes >= 100_000 && used.entries == 1);
}
