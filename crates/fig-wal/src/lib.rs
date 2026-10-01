//! `fig-wal`: checksummed write-ahead log with segment recovery.
//!
//! Framed records + CRC-32, dense sequence numbers, append with an explicit fsync
//! policy, replay, torn-tail truncation, corrupt-record handling, and segment
//! rotation. `sync()` is the acknowledgement point: everything before the last
//! sync survives a crash; the unsynced tail may be lost but never replays torn.

pub mod record;
pub mod segment;
pub mod wal;

pub use record::{WalEntry, WalOp};
pub use wal::{FsyncPolicy, RecoveryReport, Wal, WalOptions};
