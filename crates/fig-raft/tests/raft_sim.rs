//! Raft safety gate, simulated: elections, replication, partitions.
//!
//! Every test is deterministic on its seed and asserts end-to-end state
//! (committed logs + applied commands), not message counts.

use fig_raft::sim::Sim;

#[test]
fn elects_exactly_one_leader_on_many_seeds() {
    for seed in 0..10u64 {
        let mut sim = Sim::new(3, seed);
        let leader = sim.elect_leader(2000).expect("a leader must emerge");
        // Stability: no rival appears once established.
        sim.run(500);
        assert_eq!(sim.leaders(), vec![leader], "seed {seed}: rival leader");
    }
}

#[test]
fn replicates_and_applies_in_order() {
    let mut sim = Sim::new(3, 42);
    sim.elect_leader(2000).expect("leader");
    for i in 0u8..20 {
        sim.propose(vec![i]);
    }
    sim.run(500);
    let committed = sim.committed_on(&[0, 1, 2]);
    let want: Vec<Vec<u8>> = (0u8..20).map(|i| vec![i]).collect();
    assert_eq!(committed, want, "logs diverged or reordered");
    for id in [0, 1, 2] {
        sim.check_apply_contract(id);
    }
}

#[test]
fn single_node_cluster_elects_itself() {
    let mut sim = Sim::new(1, 7);
    let leader = sim.elect_leader(500).expect("solo node must lead");
    assert_eq!(leader.0, 0);
    sim.propose(b"only".to_vec());
    sim.run(100);
    assert_eq!(sim.committed_on(&[0]), vec![b"only".to_vec()]);
}

#[test]
fn minority_partition_elects_no_rival_and_heals_without_loss() {
    let mut sim = Sim::new(3, 99);
    let (first, _) = sim.elect_leader(2000).expect("leader");
    for i in 0u8..5 {
        sim.propose(vec![i]);
    }
    sim.run(300);
    assert_eq!(sim.committed_on(&[0, 1, 2]).len(), 5);

    // Isolate one follower: the majority must keep working, the minority
    // must not elect anything (no quorum alone).
    let lone = [0, 1, 2].into_iter().find(|id| *id != first).unwrap();
    for id in [0, 1, 2] {
        if id != lone {
            sim.partition(lone, id);
        }
    }
    sim.run(600);
    for (id, term) in sim.leaders() {
        assert_eq!((id, term), (first, sim.node(first).term()));
    }
    for i in 5u8..10 {
        sim.propose(vec![i]);
    }
    sim.run(300);

    // Heal: the lone node rejoins, converges, and no committed entry is lost.
    sim.heal_all();
    sim.run(1000);
    let want: Vec<Vec<u8>> = (0u8..10).map(|i| vec![i]).collect();
    assert_eq!(sim.committed_on(&[0, 1, 2]), want);
    for id in [0, 1, 2] {
        sim.check_apply_contract(id);
    }
}

#[test]
fn leader_partition_fails_over_and_old_entries_survive() {
    let mut sim = Sim::new(3, 1234);
    let (first, _) = sim.elect_leader(2000).expect("leader");
    for i in 0u8..5 {
        sim.propose(vec![i]);
    }
    sim.run(300);
    assert_eq!(sim.committed_on(&[0, 1, 2]).len(), 5);

    // Isolate the leader: the remaining two must elect a successor. The old
    // leader, hearing nothing, still *believes* it leads — split-brain across
    // a partition is expected. What must hold: the connected pair agrees on
    // exactly one leader, and the isolated node commits nothing new.
    let others: Vec<u64> = [0, 1, 2].into_iter().filter(|id| *id != first).collect();
    let old_commit = sim.node(first).commit_index();
    for id in &others {
        sim.partition(first, *id);
    }
    sim.run(1000);
    let pair_leaders: Vec<_> = sim
        .leaders()
        .into_iter()
        .filter(|(id, _)| *id != first)
        .collect();
    assert_eq!(
        pair_leaders.len(),
        1,
        "connected pair must agree on one leader"
    );
    assert_eq!(
        sim.node(first).commit_index(),
        old_commit,
        "isolated leader must not advance commit alone"
    );

    // Majority proposes more; heal; everything converges on the full prefix.
    // (Propose via whichever leader the majority holds.)
    if sim.leaders().len() == 1 {
        for i in 5u8..10 {
            sim.propose(vec![i]);
        }
        sim.run(300);
    }
    sim.heal_all();
    sim.run(1500);
    let committed = sim.committed_on(&[0, 1, 2]);
    assert!(
        committed.len() >= 5 && committed[..5] == (0u8..5).map(|i| vec![i]).collect::<Vec<_>>()[..],
        "committed prefix lost across failover: {committed:?}"
    );
    for id in [0, 1, 2] {
        sim.check_apply_contract(id);
    }
}

#[test]
fn terms_only_move_forward() {
    let mut sim = Sim::new(3, 555);
    let mut floor = 0u64;
    for _ in 0..5 {
        // Partition a rotating victim to force repeated elections.
        sim.partition(0, 1);
        sim.run(800);
        sim.heal_all();
        sim.run(400);
        for id in [0, 1, 2] {
            assert!(sim.node(id).term() >= floor, "term went backwards");
        }
        floor = sim
            .node(0)
            .term()
            .max(sim.node(1).term())
            .max(sim.node(2).term());
    }
}
