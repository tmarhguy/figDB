//! `fig-sstable`: immutable sorted table files.
//!
//! Layout: `header | data_block+ | index_block | footer`, CRC-checked per
//! block. Written once, synced, then read many times — never modified.
//! Deletes are tombstone records; merging layers resolve them.

pub mod format;
pub mod writer;

pub use format::{IndexEntry, Record};
pub use writer::{SstableMeta, SstableWriter, DEFAULT_BLOCK_TARGET};
