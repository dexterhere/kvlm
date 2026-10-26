// SPDX-License-Identifier: GPL-3.0-or-later

use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use workstation_core::models::download::{Published, Storage, StorageError, finish_hex};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = PathBuf::from(format!(
            "/tmp/kvlm-store-{stamp}-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn models(&self) -> PathBuf {
        self.0.join("models")
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn sha(content: &[u8]) -> String {
    finish_hex(Sha256::new().chain_update(content))
}

#[test]
fn the_store_is_created_private_and_locked_against_a_second_opener() {
    let dir = Dir::new();
    let store = Storage::open(&dir.models()).unwrap();
    for sub in ["", "partial", "blobs"] {
        let mode = fs::metadata(dir.models().join(sub))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "{sub}");
    }
    assert_eq!(
        fs::metadata(dir.models().join(".lock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        Storage::open(&dir.models()).err(),
        Some(StorageError::InUse)
    );
    drop(store);
    Storage::open(&dir.models()).unwrap();
}

#[test]
fn directories_that_are_links_or_not_private_are_refused() {
    let dir = Dir::new();
    Storage::open(&dir.models()).unwrap();
    fs::set_permissions(
        dir.models().join("partial"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(matches!(
        Storage::open(&dir.models()),
        Err(StorageError::Unsafe(_))
    ));
    fs::set_permissions(
        dir.models().join("partial"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();

    fs::remove_dir(dir.models().join("blobs")).unwrap();
    let elsewhere = dir.0.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&elsewhere, dir.models().join("blobs")).unwrap();
    assert!(matches!(
        Storage::open(&dir.models()),
        Err(StorageError::Unsafe(_))
    ));
}

#[test]
fn a_planted_symlink_or_hard_link_at_the_part_path_is_never_written_through() {
    let dir = Dir::new();
    let store = Storage::open(&dir.models()).unwrap();
    let digest = sha(b"payload");
    let victim = dir.0.join("victim");
    fs::write(&victim, b"precious").unwrap();

    let part = dir.models().join("partial").join(format!("{digest}.part"));
    symlink(&victim, &part).unwrap();
    assert!(store.open_part(&digest, 100).is_err());
    assert_eq!(fs::read(&victim).unwrap(), b"precious");
    fs::remove_file(&part).unwrap();

    fs::hard_link(&victim, &part).unwrap();
    assert!(matches!(
        store.open_part(&digest, 100),
        Err(StorageError::Unsafe(_))
    ));
    assert_eq!(fs::read(&victim).unwrap(), b"precious");
}

#[test]
fn digests_that_are_not_canonical_never_reach_a_path() {
    let dir = Dir::new();
    let store = Storage::open(&dir.models()).unwrap();
    for bad in [
        "../x",
        "",
        &"A".repeat(64),
        &"a".repeat(63),
        &format!("{}/", "a".repeat(63)),
    ] {
        assert_eq!(
            store.open_part(bad, 1).err(),
            Some(StorageError::BadDigest),
            "{bad}"
        );
        assert_eq!(store.blob_path(bad).err(), Some(StorageError::BadDigest));
    }
}

#[test]
fn a_partial_file_resumes_hashing_and_publishes_once_verified() {
    let dir = Dir::new();
    let store = Storage::open(&dir.models()).unwrap();
    let content: Vec<u8> = (0..3_000_000u32).map(|i| (i % 253) as u8).collect();
    let digest = sha(&content);
    let size = content.len() as u64;

    let (mut file, existing) = store.open_part(&digest, size).unwrap();
    assert_eq!(existing, 0);
    file.write_all(&content[..1_500_000]).unwrap();
    drop(file);
    assert_eq!(store.part_len(&digest).unwrap(), Some(1_500_000));

    let (mut file, existing) = store.open_part(&digest, size).unwrap();
    assert_eq!(existing, 1_500_000);
    let mut hasher = store.hash_prefix(&mut file, existing).unwrap();
    file.write_all(&content[1_500_000..]).unwrap();
    hasher.update(&content[1_500_000..]);
    assert_eq!(
        store.publish(&digest, file, hasher).unwrap(),
        Published::New
    );

    let blob = store.blob_path(&digest).unwrap();
    assert_eq!(fs::read(&blob).unwrap(), content);
    assert_eq!(
        fs::metadata(&blob).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(store.part_len(&digest).unwrap(), None);
    assert!(store.verified_blob(&digest, size).unwrap());
}

#[test]
fn a_digest_mismatch_deletes_the_partial_file_and_publishes_nothing() {
    let dir = Dir::new();
    let store = Storage::open(&dir.models()).unwrap();
    let digest = sha(b"expected content");
    let (mut file, _) = store.open_part(&digest, 16).unwrap();
    file.write_all(b"corrupted bytes!").unwrap();
    let hasher = Sha256::new().chain_update(b"corrupted bytes!");
    assert_eq!(
        store.publish(&digest, file, hasher).err(),
        Some(StorageError::Integrity)
    );
    assert_eq!(store.part_len(&digest).unwrap(), None);
    assert!(!store.verified_blob(&digest, 16).unwrap());
}

#[test]
fn an_existing_good_blob_is_reused_and_a_bad_one_is_replaced_never_trusted() {
    let dir = Dir::new();
    let store = Storage::open(&dir.models()).unwrap();
    let content = b"the same model bytes";
    let digest = sha(content);
    let blob = store.blob_path(&digest).unwrap();

    fs::write(&blob, content).unwrap();
    fs::set_permissions(&blob, fs::Permissions::from_mode(0o600)).unwrap();
    let (mut file, _) = store.open_part(&digest, content.len() as u64).unwrap();
    file.write_all(content).unwrap();
    let hasher = Sha256::new().chain_update(content);
    assert_eq!(
        store.publish(&digest, file, hasher).unwrap(),
        Published::Reused
    );
    assert_eq!(store.part_len(&digest).unwrap(), None);

    // A same-sized impostor under the digest's name is not trusted: it is replaced.
    fs::write(&blob, b"IMPOSTOR model bytes").unwrap();
    let (mut file, _) = store.open_part(&digest, content.len() as u64).unwrap();
    file.write_all(content).unwrap();
    let hasher = Sha256::new().chain_update(content);
    assert_eq!(
        store.publish(&digest, file, hasher).unwrap(),
        Published::New
    );
    assert_eq!(fs::read(&blob).unwrap(), content);
}

#[test]
fn a_part_file_longer_than_the_model_is_rejected_before_use() {
    let dir = Dir::new();
    let store = Storage::open(&dir.models()).unwrap();
    let digest = sha(b"x");
    let path: &Path = &dir.models().join("partial").join(format!("{digest}.part"));
    fs::write(path, b"way too long").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        store.open_part(&digest, 3).err(),
        Some(StorageError::PartTooLong)
    );
}

#[test]
fn free_space_reserve_and_partial_budget_are_enforced() {
    let dir = Dir::new();
    let store = Storage::open(&dir.models()).unwrap();
    assert!(store.free_bytes().unwrap() > 0);
    assert!(store.check_space(0).is_ok() || store.free_bytes().unwrap() < 3 << 30);
    // Needing more than the disk could ever hold must refuse.
    assert!(matches!(
        store.check_space(u64::MAX / 2),
        Err(StorageError::NotEnoughSpace { .. })
    ));
    // A model bigger than the whole partial budget cannot be admitted.
    let digest = sha(b"y");
    assert_eq!(
        store.check_partial_budget(&digest, 65 << 30).err(),
        Some(StorageError::PartialBudget)
    );
    assert!(store.check_partial_budget(&digest, 1 << 30).is_ok());
}

#[test]
fn a_wrong_size_request_never_deletes_a_good_blob() {
    let dir = Dir::new();
    let store = Storage::open(&dir.models()).unwrap();
    let content = b"model bytes that are fine";
    let digest = sha(content);
    let blob = store.blob_path(&digest).unwrap();
    fs::write(&blob, content).unwrap();
    fs::set_permissions(&blob, fs::Permissions::from_mode(0o600)).unwrap();

    assert!(
        !store
            .verified_blob(&digest, content.len() as u64 + 1)
            .unwrap()
    );
    assert!(blob.exists(), "a size claim alone must not remove the file");
    assert!(store.verified_blob(&digest, content.len() as u64).unwrap());
}

#[test]
fn a_partial_file_swapped_after_hashing_is_not_published() {
    let dir = Dir::new();
    let store = Storage::open(&dir.models()).unwrap();
    let content = b"verified bytes";
    let digest = sha(content);
    let (mut file, _) = store.open_part(&digest, content.len() as u64).unwrap();
    file.write_all(content).unwrap();
    let hasher = Sha256::new().chain_update(content);

    // Another process replaces the name with a different file after the bytes were hashed.
    let part = dir.models().join("partial").join(format!("{digest}.part"));
    fs::remove_file(&part).unwrap();
    fs::write(&part, b"EVIL bytes here").unwrap();
    fs::set_permissions(&part, fs::Permissions::from_mode(0o600)).unwrap();

    assert!(matches!(
        store.publish(&digest, file, hasher),
        Err(StorageError::Unsafe(_))
    ));
    assert!(!store.blob_path(&digest).unwrap().exists());
}
