use super::*;

type P = Proposer<u8, &'static str>;

fn b(counter: u64, node: u8) -> Ballot<u8> {
    Ballot { counter, node }
}

fn genesis() -> Chosen<u8, &'static str> {
    let mut first = Chosen::genesis(1, "g");
    first.value.config.acceptors = [1, 2, 3].into();
    first
}

fn with(value: &str, version: u64) -> Agreed<u8, &str> {
    let mut v = genesis().value;
    v.version = version;
    v.value = value;
    v
}

fn promise(
    ballot: &Ballot<u8>,
    accepted: Option<(Ballot<u8>, Agreed<u8, &'static str>)>,
) -> Reply<u8, &'static str> {
    Reply::Promise {
        config: 0,
        ballot: ballot.clone(),
        accepted: accepted.map(|(b, v)| (b, Some(v))),
    }
}

fn accepted(ballot: &Ballot<u8>) -> Reply<u8, &'static str> {
    Reply::Accepted {
        config: 0,
        ballot: ballot.clone(),
    }
}

/// Starts a round and returns its requests.
fn begin(p: &mut P) -> Vec<(u8, Request<u8, &'static str>)> {
    let Step::Send(requests) = p.begin() else {
        panic!("no requests")
    };
    requests
}

fn ballot_of(requests: &[(u8, Request<u8, &'static str>)]) -> Ballot<u8> {
    match &requests[0].1 {
        Request::Prepare { ballot, .. } | Request::Accept { ballot, .. } => ballot.clone(),
    }
}

#[test]
fn a_change_takes_two_round_trips() {
    let mut p = P::new(9, genesis());
    let reqs = begin(&mut p);
    assert_eq!(reqs.len(), 3);
    // The proposer has the genesis value, so acceptors may leave it out.
    let Request::Prepare { have, .. } = &reqs[0].1 else {
        panic!()
    };
    assert_eq!(have, &Some(b(0, 1)));
    let ballot = ballot_of(&reqs);

    let omitted = Reply::Promise {
        config: 0,
        ballot: ballot.clone(),
        accepted: Some((b(0, 1), None)),
    };
    assert_eq!(p.receive(1, omitted.clone()), Step::Wait);
    // A second answer from the same acceptor doesn't count twice.
    assert_eq!(p.receive(1, omitted.clone()), Step::Wait);
    assert_eq!(p.receive(2, omitted), Step::Read(genesis().value));

    let Step::Send(reqs) = p.decide(Change::Set("x")) else {
        panic!()
    };
    let Request::Accept { value, .. } = &reqs[0].1 else {
        panic!()
    };
    assert_eq!(value, &with("x", 1));
    assert_eq!(p.receive(3, accepted(&ballot)), Step::Wait);
    let done = Step::Done(Chosen {
        value: with("x", 1),
        ballot: Some(ballot),
    });
    assert_eq!(p.receive(1, accepted(&ballot_of(&reqs))), done);
    assert_eq!(p.known().value, with("x", 1));
}

#[test]
fn an_omitted_value_is_the_one_the_round_began_with() {
    let mut p = P::new(9, genesis());
    let ballot = ballot_of(&begin(&mut p));
    // A late answer to an earlier round brings a newer value in the same
    // configuration: the round goes on.
    let newer = Chosen {
        value: with("newer", 5),
        ballot: Some(b(4, 2)),
    };
    assert_eq!(
        p.receive(
            3,
            Reply::Stale {
                learned: newer.clone()
            }
        ),
        Step::Wait
    );
    // A majority leaves out the value the round said it has: the one at
    // ballot (0, 1), not the newer one. That is older than what the
    // proposer knows now, so the round starts again from the newer value.
    let omitted = Reply::Promise {
        config: 0,
        ballot: ballot.clone(),
        accepted: Some((b(0, 1), None)),
    };
    p.receive(1, omitted.clone());
    assert_eq!(p.receive(2, omitted), Step::Retry { wait: false });
    assert_eq!(p.known(), &newer);
    let Request::Prepare { have, .. } = &begin(&mut p)[0].1 else {
        panic!("not a prepare")
    };
    assert_eq!(have, &newer.ballot);
}

#[test]
fn a_read_never_goes_back_behind_a_value_learned_during_the_round() {
    let mut p = P::new(9, genesis());
    let ballot = ballot_of(&begin(&mut p));
    // Acceptors 1 and 2 promise this round. Before their promises arrive,
    // they accept version 1 from another proposer at a higher ballot, and
    // this node learns it.
    let learned = Chosen {
        value: with("one", 1),
        ballot: Some(b(5, 7)),
    };
    p.learn(learned.clone());
    let promise = Reply::Promise {
        config: 0,
        ballot: ballot.clone(),
        accepted: Some((b(0, 1), None)),
    };
    p.receive(1, promise.clone());
    assert_eq!(p.receive(2, promise), Step::Retry { wait: false });
    assert_eq!(p.known(), &learned);
}

#[test]
fn keeping_a_value_does_not_go_back_behind_one_learned_since_the_read() {
    let mut p = P::new(9, genesis());
    let ballot = ballot_of(&begin(&mut p));
    let promise = Reply::Promise {
        config: 0,
        ballot: ballot.clone(),
        accepted: Some((b(0, 1), None)),
    };
    p.receive(1, promise.clone());
    assert!(matches!(p.receive(2, promise), Step::Read(_)));
    let learned = Chosen {
        value: with("one", 1),
        ballot: Some(b(5, 7)),
    };
    p.learn(learned.clone());
    assert_eq!(p.decide(Change::Keep), Step::Done(learned.clone()));
    assert_eq!(p.known(), &learned);
}

#[test]
fn a_newer_version_in_an_older_configuration_is_not_learned() {
    let mut start = genesis();
    start.value.config.number = 2;
    let mut p = P::new(9, start.clone());
    let mut older = genesis();
    older.value.version = 5;
    p.learn(older);
    assert_eq!(p.known(), &start);
}

#[test]
fn keeping_an_agreed_value_sends_nothing() {
    let mut p = P::new(9, genesis());
    let ballot = ballot_of(&begin(&mut p));
    let v = with("x", 1);
    p.receive(1, promise(&ballot, Some((b(1, 5), v.clone()))));
    assert_eq!(
        p.receive(2, promise(&ballot, Some((b(1, 5), v.clone())))),
        Step::Read(v.clone())
    );
    let done = Step::Done(Chosen {
        value: v,
        ballot: Some(b(1, 5)),
    });
    assert_eq!(p.decide(Change::Keep), done);
}

#[test]
fn an_unagreed_value_is_written_back_before_a_change() {
    let mut p = P::new(9, genesis());
    let ballot = ballot_of(&begin(&mut p));
    let v = with("x", 1);
    p.receive(1, promise(&ballot, Some((b(1, 5), v.clone()))));
    // Acceptor 2 has an older value: "x" may be accepted by 1 only.
    let old = Some((b(0, 1), genesis().value));
    let Step::Send(reqs) = p.receive(2, promise(&ballot, old)) else {
        panic!()
    };
    let Request::Accept {
        value, ballot: at, ..
    } = &reqs[0].1
    else {
        panic!()
    };
    assert_eq!((value, at), (&v, &ballot));
    p.receive(1, accepted(&ballot));
    // Once it's agreed, a new round reads it.
    assert_eq!(p.receive(2, accepted(&ballot)), Step::Retry { wait: false });
    let next = ballot_of(&begin(&mut p));
    assert!(next > ballot);
    // Acceptor 3 missed the write-back, but "x" at `ballot` is the value
    // this proposer had agreed, so it's read as agreed.
    p.receive(1, promise(&next, Some((ballot.clone(), v.clone()))));
    let behind = Some((b(0, 1), genesis().value));
    assert_eq!(p.receive(3, promise(&next, behind)), Step::Read(v.clone()));
    let done = Step::Done(Chosen {
        value: v,
        ballot: Some(ballot),
    });
    assert_eq!(p.decide(Change::Keep), done);
}

#[test]
fn a_closing_value_moves_to_the_next_configuration() {
    let mut p = P::new(9, genesis());
    let ballot = ballot_of(&begin(&mut p));
    let mut closing = with("x", 1);
    closing.next = Some(closing.config.successor([3, 4, 5].into(), false));
    p.receive(1, promise(&ballot, Some((b(1, 5), closing.clone()))));
    let Step::Send(reqs) = p.receive(2, promise(&ballot, None)) else {
        panic!()
    };
    // Not agreed yet: written back in configuration 0 first.
    assert!(matches!(reqs[0].1, Request::Accept { config: 0, .. }));
    p.receive(1, accepted(&ballot));
    let Step::Send(reqs) = p.receive(2, accepted(&ballot)) else {
        panic!()
    };
    let targets: Vec<u8> = reqs.iter().map(|(to, _)| *to).collect();
    assert_eq!(targets, [3, 4, 5]);
    assert!(matches!(
        reqs[0].1,
        Request::Prepare {
            config: 1,
            have: None,
            ..
        }
    ));

    // The new acceptors hold nothing: the value opened from the closing
    // one is agreed.
    let ballot = ballot_of(&reqs);
    let empty = |config| Reply::Promise {
        config,
        ballot: ballot.clone(),
        accepted: None,
    };
    // An answer from a node outside the configuration is ignored.
    assert_eq!(p.receive(1, empty(1)), Step::Wait);
    p.receive(3, empty(1));
    let opened = closing.open().unwrap();
    assert_eq!(p.receive(4, empty(1)), Step::Read(opened.clone()));
    assert_eq!(opened.config.number, 1);
    assert_eq!(opened.version, 2);
}

#[test]
fn closing_the_configuration() {
    let mut p = P::new(9, genesis());
    let ballot = ballot_of(&begin(&mut p));
    let v = Some((b(0, 1), genesis().value));
    p.receive(1, promise(&ballot, v.clone()));
    p.receive(2, promise(&ballot, v));
    let Step::Send(reqs) = p.decide(Change::Close(
        "c",
        genesis().value.config.successor([2, 3, 4].into(), false),
    )) else {
        panic!()
    };
    let Request::Accept { value, .. } = &reqs[0].1 else {
        panic!()
    };
    assert_eq!(
        value.next,
        Some(value.config.successor([2, 3, 4].into(), false))
    );
    assert_eq!(value.acceptors(), &[2, 3, 4].into());
    p.receive(1, accepted(&ballot));
    // Done once the closing value is agreed; the next change opens the
    // new configuration.
    let Step::Done(chosen) = p.receive(2, accepted(&ballot)) else {
        panic!()
    };
    assert_eq!(chosen.value.config.number, 0);
    let reqs = begin(&mut p);
    assert!(matches!(reqs[0], (2, Request::Prepare { config: 1, .. })));
}

#[test]
fn refusals_from_a_majority_end_the_round() {
    let mut p = P::new(9, genesis());
    let ballot = ballot_of(&begin(&mut p));
    let rejected = Reply::Rejected {
        config: 0,
        ballot: ballot.clone(),
        promised: b(7, 2),
    };
    assert_eq!(p.receive(1, rejected.clone()), Step::Wait);
    assert_eq!(p.unreachable(2), Step::Retry { wait: true });
    // The next ballot is above the one that was promised.
    assert_eq!(ballot_of(&begin(&mut p)), b(8, 9));
    // Answers to the old round are ignored.
    assert_eq!(p.receive(3, rejected), Step::Wait);
}

#[test]
fn a_too_high_answer_takes_the_counter_back() {
    let mut p = P::new(9, genesis());
    // Another proposer's high ballot moves this one's counter up.
    let ballot = ballot_of(&begin(&mut p));
    let rejected = Reply::Rejected {
        config: 0,
        ballot,
        promised: b(crate::MAX_COUNTER_STEP, 2),
    };
    p.receive(1, rejected);
    assert_eq!(p.unreachable(2), Step::Retry { wait: true });
    let ballot = ballot_of(&begin(&mut p));
    assert_eq!(ballot, b(crate::MAX_COUNTER_STEP + 1, 9));

    // An acceptor that has not yet promised any ballot.
    let too_high = Reply::TooHigh {
        config: 0,
        ballot,
        limit: crate::MAX_COUNTER_STEP,
    };
    assert_eq!(p.receive(1, too_high), Step::Wait);
    assert_eq!(p.unreachable(2), Step::Retry { wait: true });
    assert_eq!(ballot_of(&begin(&mut p)), b(crate::MAX_COUNTER_STEP, 9));
}

#[test]
fn an_implausible_rejection_does_not_exhaust_the_counter() {
    let mut p = P::new(9, genesis());
    let ballot = ballot_of(&begin(&mut p));
    let rejected = Reply::Rejected {
        config: 0,
        ballot,
        promised: b(u64::MAX, 2),
    };
    p.receive(1, rejected);
    assert_eq!(p.unreachable(2), Step::Retry { wait: true });
    assert_eq!(ballot_of(&begin(&mut p)), b(2, 9));
}

#[test]
fn stale_foreign_and_malformed_feedback_cannot_change_the_counter() {
    for wrong in 0..4 {
        let mut p = P::new(9, genesis());
        let ballot = ballot_of(&begin(&mut p));
        let mut reply = Reply::Rejected {
            config: 0,
            ballot: ballot.clone(),
            promised: b(100, 1),
        };
        let mut from = 1;
        if let Reply::Rejected {
            config,
            ballot,
            promised,
        } = &mut reply
        {
            match wrong {
                0 => *config = 9,
                1 => ballot.counter += 1,
                2 => from = 99,
                _ => *promised = b(0, 1),
            }
        }
        p.receive(from, reply);
        p.abort();
        assert_eq!(ballot_of(&begin(&mut p)), b(2, 9));
    }
}

#[test]
fn a_protected_quorum_keeps_working_after_one_maximum_rejection() {
    let mut first = genesis();
    first.value.config.acceptors = [1, 2, 3, 4].into();
    first.value.config.protected = true;
    let mut p = P::new(9, first.clone());
    let ballot = ballot_of(&begin(&mut p));
    p.receive(
        4,
        Reply::Rejected {
            config: 0,
            ballot: ballot.clone(),
            promised: b(u64::MAX, 4),
        },
    );
    for from in 1..=3 {
        let step = p.receive(
            from,
            promise(
                &ballot,
                Some((first.ballot.clone().unwrap(), first.value.clone())),
            ),
        );
        if from == 3 {
            assert!(matches!(step, Step::Read(_)));
        }
    }
    assert!(matches!(p.decide(Change::Keep), Step::Done(_)));
    assert_eq!(ballot_of(&begin(&mut p)), b(2, 9));
}

#[test]
fn recovery_starts_above_a_previously_certified_ballot() {
    let mut known = genesis();
    known.ballot = Some(b(5 * crate::MAX_COUNTER_STEP, 2));
    let mut p = P::new(9, known);
    assert_eq!(
        ballot_of(&begin(&mut p)),
        b(5 * crate::MAX_COUNTER_STEP + 1, 9)
    );
}

#[test]
fn a_stale_answer_restarts_with_the_learned_value() {
    let mut p = P::new(9, genesis());
    begin(&mut p);
    let mut later = with("y", 5);
    later.config.number = 2;
    later.config.acceptors = [4, 5, 6].into();
    let learned = Chosen {
        value: later,
        ballot: Some(b(3, 4)),
    };
    let stale = Reply::Stale {
        learned: learned.clone(),
    };
    assert_eq!(p.receive(1, stale.clone()), Step::Retry { wait: false });
    assert_eq!(p.known(), &learned);
    let reqs = begin(&mut p);
    assert!(matches!(reqs[0], (4, Request::Prepare { config: 2, .. })));
    // The same answer again, late, changes nothing.
    assert_eq!(p.receive(2, stale), Step::Wait);
}

#[test]
#[should_panic(expected = "before a value was read")]
fn decide_needs_a_read() {
    P::new(9, genesis()).decide(Change::Keep);
}

#[test]
fn a_promise_with_a_value_above_its_ballot_counts_as_a_failure() {
    let mut p = P::new(9, genesis());
    let ballot = ballot_of(&begin(&mut p));
    // No acceptor following the protocol reports a value accepted at a
    // ballot above the one it promises.
    let reply = promise(&ballot, Some((b(99, 1), with("x", 1))));
    assert_eq!(p.receive(1, reply), Step::Wait);
    assert_eq!(p.unreachable(2), Step::Retry { wait: true });
    // The counter didn't move up to the reported ballot.
    assert_eq!(ballot_of(&begin(&mut p)), b(2, 9));
}

#[test]
fn an_implausible_too_high_answer_counts_as_a_failure() {
    for limit in [0, crate::MAX_COUNTER_STEP - 1, 1 << 40] {
        let mut p = P::new(9, genesis());
        let ballot = ballot_of(&begin(&mut p));
        let too_high = Reply::TooHigh {
            config: 0,
            ballot,
            limit,
        };
        assert_eq!(p.receive(1, too_high), Step::Wait);
        assert_eq!(p.unreachable(2), Step::Retry { wait: true });
        assert_eq!(ballot_of(&begin(&mut p)), b(2, 9), "limit {limit}");
    }
}

#[test]
fn only_acceptors_of_the_round_count_as_failures() {
    let mut p = P::new(9, genesis());
    begin(&mut p);
    // Node 99 isn't an acceptor, for example one of an older configuration.
    assert_eq!(p.unreachable(99), Step::Wait);
    // With three acceptors, one failure leaves a quorum; two don't.
    assert_eq!(p.unreachable(1), Step::Wait);
    assert_eq!(p.unreachable(2), Step::Retry { wait: true });
}

/// A copy of `genesis` with `acceptors` in configuration 1, where no value
/// can be agreed if `acceptors` is empty, or has one acceptor and is
/// protected.
fn in_config(protected: bool, acceptors: &[u8]) -> Chosen<u8, &'static str> {
    let mut c = genesis();
    c.value.version = 5;
    c.value.config.number = 1;
    c.value.config.protected = protected;
    c.value.config.acceptors = acceptors.iter().copied().collect();
    c
}

#[test]
#[should_panic(expected = "no value can be agreed in the next configuration")]
fn decide_refuses_to_close_for_a_configuration_that_cannot_agree() {
    let mut p = P::new(9, genesis());
    let ballot = ballot_of(&begin(&mut p));
    p.receive(1, promise(&ballot, None));
    let Step::Read(current) = p.receive(2, promise(&ballot, None)) else {
        panic!("no read")
    };
    let next = current.config.successor([1].into(), true);
    p.decide(Change::Close("x", next));
}

#[test]
fn values_in_configurations_that_cannot_agree_are_not_learned() {
    for (protected, acceptors) in [(false, &[][..]), (true, &[4][..])] {
        let mut p = P::new(9, genesis());
        p.learn(in_config(protected, acceptors));
        assert_eq!(p.known(), &genesis());

        // Nor from a stale answer.
        let ballot = ballot_of(&begin(&mut p));
        let stale = Reply::Stale {
            learned: in_config(protected, acceptors),
        };
        p.receive(1, stale);
        assert_eq!(p.known(), &genesis());
        assert_eq!(ballot_of(&begin(&mut p)), b(ballot.counter + 1, 9));
    }
}

#[test]
fn a_round_in_a_configuration_that_cannot_agree_ends() {
    // A caller can still start a proposer there. Each round ends at the
    // first failure, without a panic.
    let mut p = P::new(9, in_config(true, &[1]));
    begin(&mut p);
    assert_eq!(p.unreachable(1), Step::Retry { wait: true });
}

#[test]
fn a_stale_answer_between_rounds_is_learned() {
    let mut p = P::new(9, genesis());
    let mut next = genesis();
    next.value.version = 5;
    next.value.config.number = 1;
    let stale = Reply::Stale {
        learned: next.clone(),
    };
    assert_eq!(p.receive(1, stale), Step::Wait);
    assert_eq!(p.known(), &next);
}

mod properties {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;
    use crate::strategies::{self, Node, Value};

    /// Something that can happen to a proposer. Replies may come from any
    /// node, with any content, as from faulty acceptors.
    #[derive(Clone, Debug)]
    enum Op {
        Begin,
        Abort,
        Unreachable(Node),
        /// If `aim` is true, the reply's configuration and ballot are
        /// replaced with those of the current round, so that it gets past
        /// the first checks.
        Receive {
            from: Node,
            reply: Reply<Node, Value>,
            aim: bool,
        },
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            1 => Just(Op::Begin),
            1 => Just(Op::Abort),
            1 => strategies::node().prop_map(Op::Unreachable),
            8 => (strategies::node(), strategies::reply(), prop::bool::weighted(0.8))
                .prop_map(|(from, reply, aim)| Op::Receive { from, reply, aim }),
        ]
    }

    fn aim_at(reply: &mut Reply<Node, Value>, round: &(u64, Ballot<Node>)) {
        match reply {
            Reply::Promise { config, ballot, .. }
            | Reply::Accepted { config, ballot }
            | Reply::Rejected { config, ballot, .. }
            | Reply::TooHigh { config, ballot, .. } => {
                *config = round.0;
                *ballot = round.1.clone();
            }
            Reply::Stale { .. } => {}
        }
    }

    /// The change to make to a value that was read: any of the three kinds.
    fn change(current: &Agreed<Node, Value>) -> Change<Node, Value> {
        match current.version % 3 {
            0 => Change::Keep,
            1 => Change::Set(current.value.wrapping_add(1)),
            _ => {
                let acceptors = BTreeSet::from([0, 1, 2]);
                Change::Close(current.value, current.config.successor(acceptors, false))
            }
        }
    }

    proptest! {
        /// Whatever replies it gets, a proposer never panics, never forgets
        /// a newer value it knew, and sends requests only to the acceptors
        /// of the configuration of its round.
        #[test]
        fn a_proposer_survives_any_replies(ops in prop::collection::vec(op(), 0..40)) {
            let mut first = Chosen::genesis(0, 0);
            first.value.config.acceptors = BTreeSet::from([0, 1, 2]);
            let mut p = Proposer::<Node, Value>::new(9, first);
            // The configuration number and ballot of the current round.
            let mut round = None;
            for op in ops {
                let version = p.known().value.version;
                let mut step = match op {
                    Op::Begin => p.begin(),
                    Op::Abort => {
                        p.abort();
                        Step::Wait
                    }
                    Op::Unreachable(from) => p.unreachable(from),
                    Op::Receive { from, mut reply, aim } => {
                        if aim && let Some(round) = &round {
                            aim_at(&mut reply, round);
                        }
                        p.receive(from, reply)
                    }
                };
                if let Step::Read(current) = &step {
                    step = p.decide(change(current));
                }
                if let Step::Send(requests) = &step {
                    let config = match &p.phase {
                        Phase::Prepare { config, .. } | Phase::Accept { config, .. } => config,
                        _ => panic!("requests to send, but no round"),
                    };
                    for (to, req) in requests {
                        prop_assert!(config.acceptors.contains(to), "a request to {}", to);
                        let (Request::Prepare { config: n, ballot, .. }
                        | Request::Accept { config: n, ballot, .. }) = req;
                        prop_assert_eq!(*n, config.number);
                        round = Some((*n, ballot.clone()));
                    }
                }
                prop_assert!(p.known().value.version >= version, "the known value went back");
            }
        }
    }
}
