//! Seeded virtual network: deterministic time, delivery, and failure.
//!
//! The harness owns everything the core refuses to: clocks, transport, and
//! faults. Time advances in discrete ticks; each node has an election
//! countdown (re-randomized on [`Effect::ResetElectionTimer`]) and leaders
//! gain a heartbeat countdown. Messages take one tick unless a partition
//! drops them. Same seed → same run, every time.

use crate::core::{Effect, Event, Message, Node, NodeId, Role};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::{HashMap, HashSet};

const ELECTION_MIN: u64 = 150;
const ELECTION_MAX: u64 = 300;
const HEARTBEAT_EVERY: u64 = 50;

/// One cluster under test.
pub struct Sim {
    nodes: HashMap<NodeId, Node>,
    /// Application state per node: applied commands in order.
    pub applied: HashMap<NodeId, Vec<Vec<u8>>>,
    now: u64,
    pending: Vec<Queued>,
    election_at: HashMap<NodeId, u64>,
    heartbeat_at: HashMap<NodeId, u64>,
    /// Blocked directed links: (`from`, `to`) pairs that drop everything.
    blocked: HashSet<(NodeId, NodeId)>,
    rng: StdRng,
}

struct Queued {
    deliver_at: u64,
    from: NodeId,
    to: NodeId,
    msg: Message,
}

impl Sim {
    pub fn new(n: u64, seed: u64) -> Self {
        let ids: Vec<NodeId> = (0..n).collect();
        let mut sim = Self {
            nodes: HashMap::new(),
            applied: HashMap::new(),
            now: 0,
            pending: Vec::new(),
            election_at: HashMap::new(),
            heartbeat_at: HashMap::new(),
            blocked: HashSet::new(),
            rng: StdRng::seed_from_u64(seed),
        };
        for id in &ids {
            let peers = ids.iter().copied().filter(|p| p != id).collect();
            sim.nodes.insert(*id, Node::new(*id, peers));
            sim.applied.insert(*id, Vec::new());
        }
        for id in &ids {
            sim.reset_election(*id);
            sim.heartbeat_at.insert(*id, HEARTBEAT_EVERY);
        }
        sim
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[&id]
    }

    pub fn ids(&self) -> Vec<NodeId> {
        let mut ids: Vec<_> = self.nodes.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Current leaders (id, term). Empty during an election or partition.
    pub fn leaders(&self) -> Vec<(NodeId, u64)> {
        self.nodes
            .values()
            .filter(|n| n.is_leader())
            .map(|n| (n.id(), n.term()))
            .collect()
    }

    /// Sever all traffic between `a` and `b` (both directions).
    pub fn partition(&mut self, a: NodeId, b: NodeId) {
        self.blocked.insert((a, b));
        self.blocked.insert((b, a));
    }

    /// Heal every link.
    pub fn heal_all(&mut self) {
        self.blocked.clear();
    }

    fn reset_election(&mut self, id: NodeId) {
        let wait = self.rng.gen_range(ELECTION_MIN..=ELECTION_MAX);
        self.election_at.insert(id, self.now + wait);
    }

    fn deliver(&mut self, from: NodeId, to: NodeId, msg: Message) {
        if self.blocked.contains(&(from, to)) {
            return; // Partition drops silently (sender retries by timer).
        }
        self.pending.push(Queued {
            deliver_at: self.now + 1,
            from,
            to,
            msg,
        });
    }

    fn handle_effects(&mut self, id: NodeId, effects: Vec<Effect>) {
        for fx in effects {
            match fx {
                Effect::Send { to, msg } => self.deliver(id, to, msg),
                Effect::Apply { entries } => {
                    for e in entries {
                        self.applied.get_mut(&id).unwrap().push(e.command);
                    }
                }
                Effect::ResetElectionTimer => self.reset_election(id),
                Effect::NotLeader { .. } => {}
            }
        }
    }

    /// Run one tick: fire due timers, deliver due messages.
    pub fn tick(&mut self) {
        self.now += 1;
        // Timers first (deterministic order by node id).
        for id in self.ids() {
            if self.nodes[&id].role() != Role::Leader && self.now >= self.election_at[&id] {
                let fx = self
                    .nodes
                    .get_mut(&id)
                    .unwrap()
                    .poll(Event::ElectionTimeout);
                self.handle_effects(id, fx);
                self.reset_election(id);
            }
            if self.nodes[&id].role() == Role::Leader && self.now >= self.heartbeat_at[&id] {
                let fx = self
                    .nodes
                    .get_mut(&id)
                    .unwrap()
                    .poll(Event::HeartbeatTimeout);
                self.handle_effects(id, fx);
                self.heartbeat_at.insert(id, self.now + HEARTBEAT_EVERY);
            }
        }
        // Then messages due now (also id-ordered for determinism).
        let mut due: Vec<Queued> = Vec::new();
        self.pending.retain(|q| {
            if q.deliver_at <= self.now {
                due.push(Queued {
                    deliver_at: q.deliver_at,
                    from: q.from,
                    to: q.to,
                    msg: q.msg.clone(),
                });
                false
            } else {
                true
            }
        });
        due.sort_by_key(|q| (q.to, q.from));
        for q in due {
            if !self.nodes.contains_key(&q.to) {
                continue;
            }
            let fx = self.nodes.get_mut(&q.to).unwrap().poll(Event::Message {
                from: q.from,
                msg: q.msg,
            });
            self.handle_effects(q.to, fx);
        }
    }

    pub fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.tick();
        }
    }

    /// Run until exactly one leader exists (or `ticks` expire). Returns it.
    pub fn elect_leader(&mut self, ticks: u64) -> Option<(NodeId, u64)> {
        for _ in 0..ticks {
            self.tick();
            let leaders = self.leaders();
            if leaders.len() == 1 {
                return Some(leaders[0]);
            }
        }
        let leaders = self.leaders();
        (leaders.len() == 1).then(|| leaders[0])
    }

    /// Offer a command to the leader. Panics without exactly one leader.
    pub fn propose(&mut self, command: Vec<u8>) {
        let leaders = self.leaders();
        assert_eq!(leaders.len(), 1, "propose needs exactly one leader");
        let fx = self
            .nodes
            .get_mut(&leaders[0].0)
            .unwrap()
            .poll(Event::Propose { command });
        // A proposal to the leader never answers NotLeader.
        assert!(
            fx.iter().all(|e| !matches!(e, Effect::NotLeader { .. })),
            "leader refused a proposal"
        );
        self.handle_effects(leaders[0].0, fx);
    }

    /// Committed entries replicated on every listed node (by command bytes).
    pub fn committed_on(&self, ids: &[NodeId]) -> Vec<Vec<u8>> {
        let mut it = ids.iter();
        let first = &self.nodes[it.next().expect("need a node")].committed();
        for id in it {
            assert_eq!(
                &self.nodes[id].committed(),
                first,
                "node {id} diverged from the committed prefix"
            );
        }
        first.iter().map(|e| e.command.clone()).collect()
    }

    /// Every entry the node reports committed must also be applied exactly
    /// once, in order (the Apply-effect contract, checked end to end).
    pub fn check_apply_contract(&self, id: NodeId) {
        let committed: Vec<Vec<u8>> = self.nodes[&id]
            .committed()
            .iter()
            .map(|e| e.command.clone())
            .collect();
        assert_eq!(
            &self.applied[&id], &committed,
            "node {id}: applied state diverged from committed log"
        );
    }
}
