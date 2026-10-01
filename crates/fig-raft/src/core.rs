//! Deterministic Raft core: election, log replication, commitment.
//!
//! Safety rules enforced here (not in the harness):
//! - At most one leader per term (single vote per term per node).
//! - A candidate wins only with a majority including itself.
//! - A leader never overwrites its own log; conflicting follower suffixes are
//!   truncated before appending (log matching property).
//! - Only entries from the current term commit by counting replicas; older
//!   entries commit implicitly when a newer one does (leader completeness).
//! - Committed entries apply exactly once, in order, via [`Effect::Apply`].

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Cluster member handle. Dense `0..n` in the sim; mapped to addresses by R3.
pub type NodeId = u64;

/// One log entry. Index 0 is a dummy (`term 0`, empty command) so real
/// entries are 1-based and `log[i]` sits at position `i`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub index: u64,
    pub term: u64,
    pub command: Vec<u8>,
}

/// Wire + sim messages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    RequestVote {
        term: u64,
        candidate: NodeId,
        last_index: u64,
        last_term: u64,
    },
    RequestVoteResponse {
        term: u64,
        granted: bool,
    },
    AppendEntries {
        term: u64,
        leader: NodeId,
        prev_index: u64,
        prev_term: u64,
        entries: Vec<Entry>,
        leader_commit: u64,
    },
    AppendEntriesResponse {
        term: u64,
        success: bool,
        match_index: u64,
    },
}

/// Observable role (queries for the harness; transitions stay internal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Follower,
    Candidate,
    Leader,
}

/// Inputs to [`Node::poll`].
#[derive(Clone, Debug)]
pub enum Event {
    /// A message arrived from a peer.
    Message { from: NodeId, msg: Message },
    /// The node's election countdown fired (harness-owned).
    ElectionTimeout,
    /// The leader's heartbeat countdown fired (harness-owned).
    HeartbeatTimeout,
    /// A client command offered for replication.
    Propose { command: Vec<u8> },
}

/// Outputs of [`Node::poll`]. The harness/network executes them.
#[derive(Clone, Debug)]
pub enum Effect {
    /// Deliver `msg` to peer `to`.
    Send { to: NodeId, msg: Message },
    /// Entries newly committed: apply to the state machine exactly once, in
    /// order. Never reordered, never repeated, never uncommitted.
    Apply { entries: Vec<Entry> },
    /// Restart this node's election countdown (granted a vote, or accepted
    /// entries from a current-term leader).
    ResetElectionTimer,
    /// `Propose` arrived at a non-leader. The server answers `NOT_LEADER`.
    NotLeader { leader: Option<NodeId> },
}

/// A single Raft participant. Single-threaded by construction: `poll` takes
/// `&mut self` and returns owned effects; no interior mutability, no I/O.
pub struct Node {
    id: NodeId,
    peers: Vec<NodeId>,
    role: Role,
    current_term: u64,
    voted_for: Option<NodeId>,
    /// Full log including the index-0 dummy; `log[i].index == i` always.
    log: Vec<Entry>,
    commit_index: u64,
    last_applied: u64,
    /// Best-known leader (hint for `NotLeader`; cleared on term change).
    leader: Option<NodeId>,
    /// Candidate state: voters so far (self included).
    votes: HashSet<NodeId>,
    /// Leader state: next log index to send each peer.
    next_index: HashMap<NodeId, u64>,
    /// Leader state: highest replicated index per peer.
    match_index: HashMap<NodeId, u64>,
}

impl Node {
    pub fn new(id: NodeId, peers: Vec<NodeId>) -> Self {
        debug_assert!(!peers.contains(&id), "self must not be listed as peer");
        Self {
            id,
            peers,
            role: Role::Follower,
            current_term: 0,
            voted_for: None,
            log: vec![Entry {
                index: 0,
                term: 0,
                command: Vec::new(),
            }],
            commit_index: 0,
            last_applied: 0,
            leader: None,
            votes: HashSet::new(),
            next_index: HashMap::new(),
            match_index: HashMap::new(),
        }
    }

    pub fn id(&self) -> NodeId {
        self.id
    }
    pub fn role(&self) -> Role {
        self.role
    }
    pub fn term(&self) -> u64 {
        self.current_term
    }
    pub fn is_leader(&self) -> bool {
        self.role == Role::Leader
    }
    pub fn commit_index(&self) -> u64 {
        self.commit_index
    }
    pub fn last_index(&self) -> u64 {
        self.log.len() as u64 - 1
    }
    pub fn last_term(&self) -> u64 {
        self.log.last().expect("dummy entry always present").term
    }
    /// Full committed prefix (test/support hook; the server applies via effects).
    pub fn committed(&self) -> Vec<Entry> {
        self.log[1..=self.commit_index as usize].to_vec()
    }

    fn cluster_size(&self) -> usize {
        self.peers.len() + 1
    }

    fn majority(&self) -> usize {
        self.cluster_size() / 2 + 1
    }

    /// Handle one input event, returning the resulting effects.
    pub fn poll(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Message { from, msg } => self.on_message(from, msg),
            Event::ElectionTimeout => self.on_election_timeout(),
            Event::HeartbeatTimeout => self.on_heartbeat_timeout(),
            Event::Propose { command } => self.on_propose(command),
        }
    }

    // -- timers & proposals ------------------------------------------------

    fn on_election_timeout(&mut self) -> Vec<Effect> {
        if self.role == Role::Leader {
            return Vec::new(); // Leaders heartbeat; they never campaign.
        }
        self.current_term += 1;
        self.role = Role::Candidate;
        self.voted_for = Some(self.id);
        self.leader = None;
        self.votes.clear();
        self.votes.insert(self.id);
        let mut out = Vec::with_capacity(self.peers.len());
        for peer in self.peers.clone() {
            out.push(Effect::Send {
                to: peer,
                msg: Message::RequestVote {
                    term: self.current_term,
                    candidate: self.id,
                    last_index: self.last_index(),
                    last_term: self.last_term(),
                },
            });
        }
        // Single-node cluster: immediate self-election.
        if self.votes.len() >= self.majority() {
            out.extend(self.become_leader());
        }
        out
    }

    fn on_heartbeat_timeout(&mut self) -> Vec<Effect> {
        if self.role != Role::Leader {
            return Vec::new();
        }
        self.peers
            .clone()
            .into_iter()
            .map(|peer| self.replicate_to(peer))
            .collect()
    }

    fn on_propose(&mut self, command: Vec<u8>) -> Vec<Effect> {
        if self.role != Role::Leader {
            return vec![Effect::NotLeader {
                leader: self.leader,
            }];
        }
        let index = self.last_index() + 1;
        self.log.push(Entry {
            index,
            term: self.current_term,
            command,
        });
        debug_assert_eq!(self.log[index as usize].index, index);
        // Single-node clusters commit on self-store (majority of one);
        // larger clusters commit as AppendEntriesResponses arrive.
        let mut out = self.advance_commit();
        out.extend(
            self.peers
                .clone()
                .into_iter()
                .map(|peer| self.replicate_to(peer)),
        );
        out
    }

    // -- transitions -------------------------------------------------------

    fn become_follower(&mut self, term: u64) {
        debug_assert!(term >= self.current_term);
        self.role = Role::Follower;
        self.current_term = term;
        self.voted_for = None;
        self.leader = None;
        self.votes.clear();
        self.next_index.clear();
        self.match_index.clear();
    }

    fn become_leader(&mut self) -> Vec<Effect> {
        self.role = Role::Leader;
        self.leader = Some(self.id);
        self.votes.clear();
        let next = self.last_index() + 1;
        for peer in &self.peers {
            self.next_index.insert(*peer, next);
            self.match_index.insert(*peer, 0);
        }
        // Immediate heartbeat asserts authority (and commits a no-op-free
        // path: followers learn the term without waiting for a tick).
        self.on_heartbeat_timeout()
    }

    /// Build the AppendEntries (or heartbeat) currently due for `peer`.
    fn replicate_to(&mut self, peer: NodeId) -> Effect {
        let next = self.next_index.get(&peer).copied().unwrap_or(1).max(1);
        let prev_index = next - 1;
        let prev_term = self
            .log
            .get(prev_index as usize)
            .map(|e| e.term)
            .unwrap_or(0);
        let entries: Vec<Entry> = self.log.iter().skip(next as usize).cloned().collect();
        Effect::Send {
            to: peer,
            msg: Message::AppendEntries {
                term: self.current_term,
                leader: self.id,
                prev_index,
                prev_term,
                entries,
                leader_commit: self.commit_index,
            },
        }
    }

    // -- messages ----------------------------------------------------------

    fn on_message(&mut self, from: NodeId, msg: Message) -> Vec<Effect> {
        match msg {
            Message::RequestVote {
                term,
                candidate,
                last_index,
                last_term,
            } => self.on_request_vote(from, term, candidate, last_index, last_term),
            Message::RequestVoteResponse { term, granted } => {
                self.on_request_vote_response(from, term, granted)
            }
            Message::AppendEntries {
                term,
                leader,
                prev_index,
                prev_term,
                entries,
                leader_commit,
            } => self.on_append_entries(
                from,
                term,
                leader,
                prev_index,
                prev_term,
                entries,
                leader_commit,
            ),
            Message::AppendEntriesResponse {
                term,
                success,
                match_index,
            } => self.on_append_response(from, term, success, match_index),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn on_request_vote(
        &mut self,
        from: NodeId,
        term: u64,
        candidate: NodeId,
        last_index: u64,
        last_term: u64,
    ) -> Vec<Effect> {
        if term < self.current_term {
            return vec![Effect::Send {
                to: from,
                msg: Message::RequestVoteResponse {
                    term: self.current_term,
                    granted: false,
                },
            }];
        }
        if term > self.current_term {
            self.become_follower(term);
        }
        // Log freshness: candidate must be at least as up-to-date.
        let fresh = (last_term > self.last_term())
            || (last_term == self.last_term() && last_index >= self.last_index());
        let grantable = self.voted_for.is_none_or(|v| v == candidate);
        if fresh && grantable {
            self.voted_for = Some(candidate);
            vec![
                Effect::Send {
                    to: from,
                    msg: Message::RequestVoteResponse {
                        term: self.current_term,
                        granted: true,
                    },
                },
                Effect::ResetElectionTimer,
            ]
        } else {
            vec![Effect::Send {
                to: from,
                msg: Message::RequestVoteResponse {
                    term: self.current_term,
                    granted: false,
                },
            }]
        }
    }

    /// One ballot per voter per term: a node votes once, and the harness
    /// delivers each reply once, so inserting the voter id counts distinct
    /// ballots exactly.
    fn on_request_vote_response(&mut self, from: NodeId, term: u64, granted: bool) -> Vec<Effect> {
        if term > self.current_term {
            self.become_follower(term);
            return Vec::new();
        }
        if self.role != Role::Candidate || term != self.current_term {
            return Vec::new(); // Stale reply (or we already won/lost).
        }
        if granted {
            self.votes.insert(from);
        }
        if self.votes.len() >= self.majority() {
            return self.become_leader();
        }
        Vec::new()
    }

    #[allow(clippy::too_many_arguments)]
    fn on_append_entries(
        &mut self,
        from: NodeId,
        term: u64,
        leader: NodeId,
        prev_index: u64,
        prev_term: u64,
        entries: Vec<Entry>,
        leader_commit: u64,
    ) -> Vec<Effect> {
        if term < self.current_term {
            return vec![self.append_reply(from, false)];
        }
        if term > self.current_term {
            self.become_follower(term);
        } else if self.role != Role::Follower {
            // Same-term AppendEntries from another leader: step down (we lost
            // the election) and process as a follower.
            self.become_follower(term);
        }
        self.leader = Some(leader);
        // Log matching: our entry at prev_index must exist with prev_term.
        let consistent = match self.log.get(prev_index as usize) {
            Some(e) => e.term == prev_term,
            None => false,
        };
        if !consistent {
            return vec![self.append_reply(from, false), Effect::ResetElectionTimer];
        }
        // Truncate any conflicting suffix, then append the new entries.
        self.log.truncate(prev_index as usize + 1);
        for e in entries {
            debug_assert_eq!(e.index, self.log.len() as u64);
            self.log.push(e);
        }
        if leader_commit > self.commit_index {
            self.commit_index = leader_commit.min(self.last_index());
        }
        let mut out = vec![self.append_reply(from, true), Effect::ResetElectionTimer];
        out.extend(self.drain_apply());
        out
    }

    fn append_reply(&self, to: NodeId, success: bool) -> Effect {
        Effect::Send {
            to,
            msg: Message::AppendEntriesResponse {
                term: self.current_term,
                success,
                match_index: if success { self.last_index() } else { 0 },
            },
        }
    }

    fn on_append_response(
        &mut self,
        from: NodeId,
        term: u64,
        success: bool,
        match_index: u64,
    ) -> Vec<Effect> {
        if term > self.current_term {
            self.become_follower(term);
            return Vec::new();
        }
        if self.role != Role::Leader || term != self.current_term {
            return Vec::new();
        }
        if success {
            let known = self.match_index.get(&from).copied().unwrap_or(0);
            if match_index > known {
                self.match_index.insert(from, match_index);
            }
            self.next_index.insert(from, match_index + 1);
            self.advance_commit()
        } else {
            // Back off one entry and retry immediately (no conflict probing
            // yet — simple, correct, and bounded by the follower's log).
            let next = self.next_index.get(&from).copied().unwrap_or(1).max(1);
            self.next_index.insert(from, next.saturating_sub(1).max(1));
            vec![self.replicate_to(from)]
        }
    }

    /// Commit every prefix entry from the current term held by a majority.
    /// Older-term entries commit implicitly alongside (leader completeness).
    fn advance_commit(&mut self) -> Vec<Effect> {
        let mut out = Vec::new();
        while self.commit_index < self.last_index() {
            let n = self.commit_index + 1;
            if self.log[n as usize].term != self.current_term {
                break; // Only current-term entries commit by counting.
            }
            let holders = 1 + self.match_index.values().filter(|m| **m >= n).count();
            if holders < self.majority() {
                break;
            }
            self.commit_index = n;
            out.extend(self.drain_apply());
        }
        // Newly committed entries may let followers advance: refresh them.
        if !out.is_empty() {
            for peer in self.peers.clone() {
                let upto = self.match_index.get(&peer).copied().unwrap_or(0);
                if upto < self.commit_index {
                    out.push(self.replicate_to(peer));
                }
            }
        }
        out
    }

    /// Emit newly committed, not-yet-applied entries in order (at most once).
    fn drain_apply(&mut self) -> Vec<Effect> {
        if self.last_applied >= self.commit_index {
            return Vec::new();
        }
        let entries: Vec<Entry> =
            self.log[(self.last_applied + 1) as usize..=self.commit_index as usize].to_vec();
        self.last_applied = self.commit_index;
        vec![Effect::Apply { entries }]
    }
}
