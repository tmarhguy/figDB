//! Crash-mid-flush gate: every prefix of the flush publish sequence
//! (`write tmp → rename → manifest → WAL clear`) reopens cleanly.
//!
//! Each test plants the on-disk litter of a SIGKILL at one crash window and
//! asserts open succeeds with exactly the acknowledged state — never a
//! `Corruption` from a half-published table.

use fig_core::Config;
use fig_sstable::SstableWriter;
use fig_storage::{Database, Manifest};
use fig_wal::FsyncPolicy;

const THRESHOLD: usize = 64;

fn open(dir: &std::path::Path) -> Database {
    Database::open(dir, Config::default(), FsyncPolicy::Always, THRESHOLD).unwrap()
}

fn table_dir(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("tables")
}

/// Manifest must always agree with the open table set.
fn assert_manifest_agrees(dir: &std::path::Path, db: &Database) {
    let bytes = std::fs::read(table_dir(dir).join("MANIFEST")).expect("MANIFEST exists");
    let m: Manifest = serde_json::from_slice(&bytes).expect("manifest decodes");
    assert_eq!(
        m.tables.len(),
        db.table_count(),
        "manifest must list exactly the open tables"
    );
}

#[test]
fn staging_litter_from_killed_writer_is_ignored() {
    // Crash window 1: killed while the table bytes were still staging.
    let dir = tempfile::tempdir().unwrap();
    let mut db = open(dir.path());
    db.put(b"k".to_vec(), b"v".to_vec()).unwrap();
    db.flush().unwrap();
    drop(db);
    // Plant a killed writer's litter: partial table tmp + partial manifest tmp.
    std::fs::write(
        table_dir(dir.path()).join("sst-000001.sst.tmp"),
        b"half-written table bytes",
    )
    .unwrap();
    std::fs::write(table_dir(dir.path()).join("MANIFEST.tmp"), b"{\"part").unwrap();
    let db = open(dir.path());
    assert_eq!(db.get(b"k").unwrap(), Some(b"v".to_vec()));
    assert_eq!(db.table_count(), 1);
    // Litter cleaned: a second open sees a quiet directory.
    assert_manifest_agrees(dir.path(), &db);
    drop(db);
    let db = open(dir.path());
    assert_eq!(db.table_count(), 1);
    assert!(!table_dir(dir.path()).join("sst-000001.sst.tmp").exists());
    assert!(!table_dir(dir.path()).join("MANIFEST.tmp").exists());
}

#[test]
fn unlisted_table_from_killed_manifest_write_is_dropped() {
    // Crash window 2: table renamed to its final name, manifest never updated.
    // Even a fully VALID unlisted table must not leak into reads — it was
    // never published, and the WAL still holds its data.
    let dir = tempfile::tempdir().unwrap();
    let mut db = open(dir.path());
    db.put(b"real".to_vec(), b"1".to_vec()).unwrap();
    db.flush().unwrap();
    drop(db);
    // Forge the orphan: a valid table the manifest does not list.
    let orphan = table_dir(dir.path()).join("sst-000001.sst");
    let mut w = SstableWriter::create(&orphan, 1024).unwrap();
    w.put(b"evil", b"x").unwrap();
    w.finish().unwrap();
    let db = open(dir.path());
    assert_eq!(db.get(b"real").unwrap(), Some(b"1".to_vec()));
    assert_eq!(
        db.get(b"evil").unwrap(),
        None,
        "unpublished table must never leak into reads"
    );
    assert_eq!(db.table_count(), 1);
    assert!(
        !orphan.exists(),
        "orphan must be reaped so it can never confuse a later open"
    );
    assert_manifest_agrees(dir.path(), &db);
}

#[test]
fn manifest_survives_many_flushes_and_reopens() {
    // Steady-state invariant across a realistic flush/reopen interleaving.
    let dir = tempfile::tempdir().unwrap();
    let mut db = open(dir.path());
    for i in 0..50u8 {
        db.put(vec![i], vec![i]).unwrap();
    }
    assert!(db.table_count() >= 1);
    assert_manifest_agrees(dir.path(), &db);
    drop(db);
    let mut db = open(dir.path());
    for i in 0..50u8 {
        assert_eq!(db.get(&[i]).unwrap(), Some(vec![i]));
    }
    // More writes after reopen keep ids dense (no reuse, no gaps in manifest).
    for i in 50..80u8 {
        db.put(vec![i], vec![i]).unwrap();
    }
    assert_manifest_agrees(dir.path(), &db);
    let bytes = std::fs::read(table_dir(dir.path()).join("MANIFEST")).unwrap();
    let m: Manifest = serde_json::from_slice(&bytes).unwrap();
    let mut ids = m.tables.clone();
    ids.sort_unstable();
    assert_eq!(ids, m.tables, "manifest order must be oldest→newest");
    assert!(
        m.next_table_id > *m.tables.iter().max().unwrap_or(&0),
        "next id must advance past every published table"
    );
}

#[test]
fn legacy_directory_without_manifest_is_adopted() {
    // Directories written before manifests existed open cleanly and gain one.
    let dir = tempfile::tempdir().unwrap();
    let tables = table_dir(dir.path());
    std::fs::create_dir_all(&tables).unwrap();
    std::fs::create_dir_all(dir.path().join("wal")).unwrap();
    for (id, key) in [(3u64, "a"), (7u64, "b")] {
        let p = tables.join(format!("sst-{id:06}.sst"));
        let mut w = SstableWriter::create(&p, 1024).unwrap();
        w.put(key.as_bytes(), b"v").unwrap();
        w.finish().unwrap();
    }
    let db = open(dir.path());
    assert_eq!(db.get(b"a").unwrap(), Some(b"v".to_vec()));
    assert_eq!(db.get(b"b").unwrap(), Some(b"v".to_vec()));
    assert_eq!(db.table_count(), 2);
    // Adopted + manifest written: next flush must not reuse id 4..=7.
    assert_manifest_agrees(dir.path(), &db);
    drop(db);
    let mut db = open(dir.path());
    db.put(b"c".to_vec(), b"v".to_vec()).unwrap();
    db.flush().unwrap();
    assert_eq!(db.table_count(), 3);
    assert_manifest_agrees(dir.path(), &db);
    assert_eq!(db.get(b"c").unwrap(), Some(b"v".to_vec()));
}
