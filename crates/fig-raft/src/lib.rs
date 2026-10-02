//! `fig-raft`: Raft consensus, deterministic core first.
//!
//! The core (`Node`) is a pure state machine: no I/O, no threads, no clock.
//! Inputs are [`Event`]s (messages, timer firings, client proposals);
//! outputs are [`Effect`]s (outbound messages, committed entries, timer
//! resets). All timing, transport, and durability live outside — first in
//! the [`sim`] harness (seeded virtual network), later in sockets + disk.
//!
//! Wire types derive serde now so R3 networking reuses them byte-for-byte.

pub mod core;
pub mod persist;
pub mod sim;

pub use core::{Dirty, Effect, Entry, Event, Message, Node, NodeId, Role};
pub use persist::Store;
