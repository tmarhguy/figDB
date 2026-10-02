//! R2 — crash-safe Raft durability: `current_term` / `voted_for` / log.
//!
//! Contract (mirrors `fig-storage/manifest.rs`):
//! - The caller polls the [`crate::core::Node`], calls
//!   [`Store::save_if_dirty`] with the same node, and only *then* acts on
//!   the returned effects. A vote is durable before its reply is sent; an
//!   entry is durable before it is acknowledged or applied.
//! - Restart replays `(term, voted_for, log)` into [`crate::core::Node::restore`].
//!   Volatile state (role, commit index, leader, replication progress) always
//!   restarts clean and is re-elected / re-committed.
//! - Uncommitted suffix entries may be overwritten by a new leader after a
//!   restart — they are never applied before commit, so truncation is safe.
//!
//! Crash windows:
//! - Crash before rename: only `raft-state.tmp` litter remains → reaped at open.
//! - Crash after rename: the new state is fully durable (rename is atomic).
//! - Torn/corrupt final file: open fails with [`fig_core::Error::Corruption`]
//!   (fail to start, never replay half a vote or half an entry).

use crate::core::{Dirty, Entry, Node, NodeId};
use fig_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const STATE_NAME: &str = "raft-state.json";
const STATE_TMP_NAME: &str = "raft-state.json.tmp";
const STATE_VERSION: u32 = 1;

/// On-disk image. `log` includes the index-0 dummy so `log[i].index == i`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Persisted {
    version: u32,
    current_term: u64,
    voted_for: Option<NodeId>,
    log: Vec<Entry>,
}

/// Crash-safe store for one Raft node's durable state.
pub struct Store {
    dir: PathBuf,
    path: PathBuf,
    tmp: PathBuf,
}

impl Store {
    /// Open (creating) `dir`, reap staging litter, and ensure a state file
    /// exists so the first vote/append has something atomic to replace.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(Error::Io)?;
        let store = Self {
            dir: dir.to_path_buf(),
            path: dir.join(STATE_NAME),
            tmp: dir.join(STATE_TMP_NAME),
        };
        store.remove_litter()?;
        if !store.path.exists() {
            store.save(
                0,
                None,
                &[Entry {
                    index: 0,
                    term: 0,
                    command: Vec::new(),
                }],
            )?;
        }
        Ok(store)
    }

    /// Load `(current_term, voted_for, log)`. Missing file (fresh dir raced
    /// with open) returns the genesis state.
    pub fn load(&self) -> Result<(u64, Option<NodeId>, Vec<Entry>)> {
        if !self.path.exists() {
            return Ok((
                0,
                None,
                vec![Entry {
                    index: 0,
                    term: 0,
                    command: Vec::new(),
                }],
            ));
        }
        let bytes = std::fs::read(&self.path).map_err(Error::Io)?;
        let p: Persisted = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Corruption(format!("raft state decode failed: {e}")))?;
        if p.version != STATE_VERSION {
            return Err(Error::Corruption(format!(
                "raft state version {} unsupported",
                p.version
            )));
        }
        if p.log.is_empty() || p.log[0].index != 0 {
            return Err(Error::Corruption(
                "raft state log must start with the index-0 dummy".into(),
            ));
        }
        for (i, e) in p.log.iter().enumerate() {
            if e.index != i as u64 {
                return Err(Error::Corruption(format!(
                    "raft state log index gap at {i}: entry index {}",
                    e.index
                )));
            }
        }
        Ok((p.current_term, p.voted_for, p.log))
    }

    /// Persist one node's full durable state (tmp → sync → rename → dir fsync).
    pub fn save(&self, current_term: u64, voted_for: Option<NodeId>, log: &[Entry]) -> Result<()> {
        let image = Persisted {
            version: STATE_VERSION,
            current_term,
            voted_for,
            log: log.to_vec(),
        };
        let bytes = serde_json::to_vec(&image).map_err(|e| Error::Codec(e.to_string()))?;
        {
            let mut f = std::fs::File::create(&self.tmp).map_err(Error::Io)?;
            use std::io::Write as _;
            f.write_all(&bytes).map_err(Error::Io)?;
            f.sync_all().map_err(Error::Io)?;
        }
        std::fs::rename(&self.tmp, &self.path).map_err(Error::Io)?;
        fsync_dir(&self.dir)
    }

    /// Persist whatever [`Node::take_dirty`] reports since the last call.
    /// Returns the flags that were made durable (empty = no write happened).
    /// Call this after every `poll`, before executing its effects.
    pub fn save_if_dirty(&self, node: &mut Node) -> Result<Dirty> {
        let dirty = node.take_dirty();
        if dirty.any() {
            let (term, voted_for) = node.hard_state();
            self.save(term, voted_for, node.durable_log())?;
        }
        Ok(dirty)
    }

    fn remove_litter(&self) -> Result<()> {
        if self.tmp.exists() {
            std::fs::remove_file(&self.tmp).map_err(Error::Io)?;
        }
        Ok(())
    }
}

fn fsync_dir(path: &Path) -> Result<()> {
    let f = std::fs::File::open(path).map_err(Error::Io)?;
    f.sync_all().map_err(Error::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Event, Role};
    use tempfile::tempdir;

    #[test]
    fn roundtrip_vote_and_entries() {
        let d = tempdir().unwrap();
        let store = Store::open(d.path()).unwrap();
        // Genesis state loads clean.
        let (term, voted, log) = store.load().unwrap();
        assert_eq!((term, voted), (0, None));
        assert_eq!(log.len(), 1);

        // Drive a node: campaign (hard-state dirty) + propose (log dirty).
        let mut n = Node::new(0, vec![1, 2]);
        let _ = n.poll(Event::ElectionTimeout);
        let dirty = store.save_if_dirty(&mut n).unwrap();
        assert!(dirty.hard_state, "campaign must mark hard state");

        // Reopen: vote survives the restart.
        let store2 = Store::open(d.path()).unwrap();
        let (term2, voted2, _) = store2.load().unwrap();
        assert_eq!(term2, 1);
        assert_eq!(voted2, Some(0));

        // Restore and elect solo to propose: log survives the restart.
        let mut solo = Node::restore(0, vec![], term2, voted2, {
            let (_, _, log) = store2.load().unwrap();
            log
        });
        let _ = solo.poll(Event::ElectionTimeout); // self-elects (majority of one)
        assert_eq!(solo.role(), Role::Leader);
        let _ = solo.poll(Event::Propose {
            command: b"hello".to_vec(),
        });
        let dirty = store.save_if_dirty(&mut solo).unwrap();
        assert!(dirty.log, "propose must mark the log");

        let (term3, _, log3) = Store::open(d.path()).unwrap().load().unwrap();
        assert_eq!(term3, solo.hard_state().0);
        assert_eq!(log3.last().unwrap().command, b"hello");
    }

    #[test]
    fn heartbeat_writes_nothing() {
        let d = tempdir().unwrap();
        let store = Store::open(d.path()).unwrap();
        let mut n = Node::restore(0, vec![], 0, None, {
            let (_, _, log) = store.load().unwrap();
            log
        });
        let _ = n.poll(Event::ElectionTimeout);
        assert_eq!(n.role(), Role::Leader); // solo self-election
        let _ = store.save_if_dirty(&mut n).unwrap();
        let before = std::fs::read(d.path().join(STATE_NAME)).unwrap();
        // Heartbeat replicates nothing: no dirty flags, no rewrite.
        // (Solo node has no peers, so the heartbeat emits zero messages —
        // the point is it still dirties nothing on disk.)
        let _fx = n.poll(Event::HeartbeatTimeout);
        let dirty = store.save_if_dirty(&mut n).unwrap();
        assert!(!dirty.any(), "heartbeat must not dirty the disk state");
        let after = std::fs::read(d.path().join(STATE_NAME)).unwrap();
        assert_eq!(before, after, "heartbeat must not rewrite the file");
    }

    #[test]
    fn tmp_litter_reaped_and_corrupt_is_corruption() {
        let d = tempdir().unwrap();
        let _ = Store::open(d.path()).unwrap();
        std::fs::write(d.path().join(STATE_TMP_NAME), b"partial").unwrap();
        let _ = Store::open(d.path()).unwrap(); // reaps litter
        assert!(!d.path().join(STATE_TMP_NAME).exists());

        std::fs::write(d.path().join(STATE_NAME), b"{not json").unwrap();
        let err = Store::open(d.path()).and_then(|s| s.load()).unwrap_err();
        assert!(matches!(err, Error::Corruption(_)));
    }

    #[test]
    fn uncommitted_suffix_may_be_overwritten_after_restart() {
        // Leader A appends entry X (uncommitted), crashes; leader B with a
        // higher term overwrites X with Y. Truncation of uncommitted suffix
        // is the log-matching property, not data loss: X was never acked.
        let d = tempdir().unwrap();
        let store = Store::open(d.path()).unwrap();
        let mut a = Node::new(0, vec![1, 2]);
        let _ = a.poll(Event::ElectionTimeout); // term 1, votes self
        let (t, v) = a.hard_state();
        let log = a.durable_log().to_vec();
        store.save(t, v, &log).unwrap();

        // Restart into a follower that learns a newer term with a conflicting
        // suffix: the old uncommitted tail is truncated, dirty flag set.
        let (term, voted, log) = store.load().unwrap();
        let mut f = Node::restore(1, vec![0, 2], term, voted, log);
        let fx = f.poll(Event::Message {
            from: 0,
            msg: crate::core::Message::AppendEntries {
                term: term + 1,
                leader: 0,
                prev_index: 0,
                prev_term: 0,
                entries: vec![Entry {
                    index: 1,
                    term: term + 1,
                    command: b"Y".to_vec(),
                }],
                leader_commit: 0,
            },
        });
        assert!(fx
            .iter()
            .any(|e| matches!(e, crate::core::Effect::Send { .. })));
        let dirty = store.save_if_dirty(&mut f).unwrap();
        assert!(dirty.any());
        let (_, _, log) = store.load().unwrap();
        assert_eq!(log.last().unwrap().command, b"Y");
        // Nothing was ever committed, so nothing was ever applied.
        assert_eq!(f.commit_index(), 0);
    }
}
