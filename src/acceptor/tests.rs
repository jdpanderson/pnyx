use std::collections::BTreeSet;

use super::*;
use crate::Config;

type A = Acceptor<u8, &'static str>;

fn b(counter: u64, node: u8) -> Ballot<u8> {
    Ballot { counter, node }
}

fn value(version: u64, config: u64, v: &'static str) -> Agreed<u8, &'static str> {
    Agreed {
        version,
        config: Config {
            protected: false,
            number: config,
            acceptors: BTreeSet::from([1, 2, 3]),
        },
        next: None,
        value: v,
    }
}

fn prepare(config: u64, ballot: Ballot<u8>) -> Request<u8, &'static str> {
    Request::Prepare {
        config,
        ballot,
        have: None,
    }
}

#[test]
fn promises_each_ballot_once() {
    let mut a = A::default();
    let (reply, changed) = a.handle(prepare(0, b(2, 1))).unwrap();
    assert!(changed);
    assert!(matches!(reply, Reply::Promise { accepted: None, .. }));

    // The same ballot again, as from a proposer that restarted.
    let (reply, changed) = a.handle(prepare(0, b(2, 1))).unwrap();
    assert!(!changed);
    assert_eq!(
        reply,
        Reply::Rejected {
            config: 0,
            ballot: b(2, 1),
            promised: b(2, 1)
        }
    );

    let (reply, changed) = a.handle(prepare(0, b(1, 9))).unwrap();
    assert!(!changed);
    assert_eq!(
        reply,
        Reply::Rejected {
            config: 0,
            ballot: b(1, 9),
            promised: b(2, 1)
        }
    );
    let accept = Request::Accept {
        config: 0,
        ballot: b(1, 9),
        value: value(1, 0, "x"),
    };
    assert!(matches!(
        a.handle(accept).unwrap().0,
        Reply::Rejected { .. }
    ));
}

#[test]
fn refuses_counters_far_above_its_promise() {
    let mut a = A::default();
    let too_high = |ballot, limit| Reply::TooHigh {
        config: 0,
        ballot,
        limit,
    };
    // Nothing promised yet: the limit counts from zero.
    let far = b(MAX_COUNTER_STEP + 1, 1);
    assert_eq!(
        a.handle(prepare(0, far.clone())).unwrap(),
        (too_high(far, MAX_COUNTER_STEP), false)
    );
    assert_eq!(a, A::default());
    let (reply, _) = a.handle(prepare(0, b(MAX_COUNTER_STEP, 1))).unwrap();
    assert!(matches!(reply, Reply::Promise { .. }));

    // After a promise, the limit counts from it, for accepts too.
    let far = b(2 * MAX_COUNTER_STEP + 1, 2);
    let accept = Request::Accept {
        config: 0,
        ballot: far.clone(),
        value: value(1, 0, "x"),
    };
    assert_eq!(
        a.handle(accept).unwrap(),
        (too_high(far, 2 * MAX_COUNTER_STEP), false)
    );
    let (reply, _) = a.handle(prepare(0, b(2 * MAX_COUNTER_STEP, 2))).unwrap();
    assert!(matches!(reply, Reply::Promise { .. }));
}

#[test]
fn the_counter_limit_stops_at_the_highest_counter() {
    let mut a = A::default();
    a.slots.insert(
        0,
        Slot {
            promised: Some(b(u64::MAX - 1, 1)),
            accepted: None,
        },
    );
    let (reply, _) = a.handle(prepare(0, b(u64::MAX, 1))).unwrap();
    assert!(matches!(reply, Reply::Promise { .. }));
}

#[test]
fn accepts_and_reports_the_value() {
    let mut a = A::default();
    let accept = Request::Accept {
        config: 0,
        ballot: b(1, 1),
        value: value(1, 0, "x"),
    };
    let (reply, changed) = a.handle(accept).unwrap();
    assert!(changed);
    assert_eq!(
        reply,
        Reply::Accepted {
            config: 0,
            ballot: b(1, 1)
        }
    );

    let (reply, _) = a.handle(prepare(0, b(2, 2))).unwrap();
    let Reply::Promise { accepted, .. } = reply else {
        panic!("{reply:?}")
    };
    assert_eq!(accepted, Some((b(1, 1), Some(value(1, 0, "x")))));

    // A proposer that has the value doesn't get it again.
    let have = Request::Prepare {
        config: 0,
        ballot: b(3, 2),
        have: Some(b(1, 1)),
    };
    let Reply::Promise { accepted, .. } = a.handle(have).unwrap().0 else {
        panic!()
    };
    assert_eq!(accepted, Some((b(1, 1), None)));

    assert_eq!(a.accepted(0, &b(1, 1)), Some(&value(1, 0, "x")));
    assert_eq!(a.accepted(0, &b(2, 1)), None);
    assert_eq!(a.accepted(1, &b(1, 1)), None);
}

#[test]
fn accepts_one_value_for_each_ballot() {
    let mut a = A::default();
    let accept = |v| Request::Accept {
        config: 0,
        ballot: b(1, 1),
        value: value(1, 0, v),
    };
    a.handle(accept("x")).unwrap();
    // The same accept again changes nothing.
    let (reply, changed) = a.handle(accept("x")).unwrap();
    assert!(!changed);
    assert!(matches!(reply, Reply::Accepted { .. }));
    let before = a.clone();
    assert!(a.handle(accept("y")).is_err());
    assert_eq!(a, before);
}

#[test]
fn keeps_configurations_apart() {
    let mut a = A::default();
    a.handle(prepare(0, b(5, 1))).unwrap();
    // A lower ballot is fine in another configuration.
    let (reply, _) = a.handle(prepare(1, b(1, 1))).unwrap();
    assert!(matches!(reply, Reply::Promise { accepted: None, .. }));
}

#[test]
fn refuses_a_value_from_another_configuration() {
    let mut a = A::default();
    let accept = Request::Accept {
        config: 1,
        ballot: b(1, 1),
        value: value(1, 0, "x"),
    };
    assert!(a.handle(accept).is_err());
    assert_eq!(a, A::default());
}

#[test]
fn learning_drops_old_configurations() {
    let mut a = A::default();
    a.handle(prepare(0, b(1, 1))).unwrap();
    a.handle(prepare(1, b(1, 1))).unwrap();
    let learned = Chosen {
        value: value(4, 1, "y"),
        ballot: Some(b(1, 1)),
    };
    assert!(a.learn(learned.clone(), ()));
    assert!(!a.learn(learned.clone(), ()));
    assert_eq!(a.slots.keys().collect::<Vec<_>>(), [&1]);

    let (reply, changed) = a.handle(prepare(0, b(9, 9))).unwrap();
    assert!(!changed);
    assert_eq!(reply, Reply::Stale { learned });
    assert!(matches!(
        a.handle(prepare(1, b(2, 1))).unwrap().0,
        Reply::Promise { .. }
    ));
}

#[test]
fn a_new_acceptor_answers_for_older_configurations_as_stale() {
    let learned = Chosen {
        value: value(4, 1, "y"),
        ballot: None,
    };
    let mut a = A::with_learned(learned.clone(), ());
    assert_eq!(a.learned(), Some(&learned));
    let (reply, changed) = a.handle(prepare(0, b(5, 1))).unwrap();
    assert!(!changed);
    assert_eq!(reply, Reply::Stale { learned });
}

#[test]
fn genesis_holds_the_first_value() {
    let first = Chosen::genesis(1u8, "g");
    let mut a = Acceptor::genesis(first.clone(), ());
    let Reply::Promise { accepted, .. } = a.handle(prepare(0, b(1, 2))).unwrap().0 else {
        panic!()
    };
    assert_eq!(accepted, Some((b(0, 1), Some(first.value))));
}

/// An acceptor that keeps a proof with each value, as a Byzantine layer does.
type Proven = Acceptor<u8, &'static str, u32>;

fn accept_at(
    config: u64,
    ballot: Ballot<u8>,
    value: Agreed<u8, &'static str>,
) -> Request<u8, &'static str> {
    Request::Accept {
        config,
        ballot,
        value,
    }
}

#[test]
fn an_invalid_accept_keeps_an_earlier_promise() {
    let mut a = A::default();
    a.handle(prepare(0, b(3, 1))).unwrap();
    let before = a.clone();
    assert!(a.handle(accept_at(0, b(3, 1), value(1, 1, "x"))).is_err());
    assert_eq!(a, before);
}

#[test]
fn accepted_values_has_one_for_each_configuration() {
    let mut a = A::default();
    a.handle(accept_at(0, b(1, 1), value(1, 0, "x"))).unwrap();
    // A promise without a value.
    a.handle(prepare(1, b(1, 1))).unwrap();
    a.handle(accept_at(2, b(1, 1), value(2, 2, "y"))).unwrap();
    let values: Vec<_> = a.accepted_values().map(|v| v.value).collect();
    assert_eq!(values, ["x", "y"]);
}

#[test]
fn an_endorsement_promises_its_ballot() {
    let mut a = A::default();
    a.endorse(b(3, 1), value(1, 0, "x")).unwrap();
    assert!(matches!(
        a.handle(prepare(0, b(2, 2))).unwrap().0,
        Reply::Rejected { .. }
    ));
    assert!(matches!(
        a.handle(prepare(0, b(4, 2))).unwrap().0,
        Reply::Promise { .. }
    ));
}

#[test]
fn endorses_one_value_for_each_ballot() {
    let mut a = A::default();
    a.endorse(b(3, 1), value(1, 0, "x")).unwrap();
    // The same value again, as after a restart.
    a.endorse(b(3, 1), value(1, 0, "x")).unwrap();
    assert!(a.endorse(b(3, 1), value(1, 0, "y")).is_err());
    // A higher ballot can replace a value from a round that stopped.
    a.endorse(b(4, 1), value(1, 0, "y")).unwrap();
}

#[test]
fn endorses_only_the_value_accepted_at_a_ballot() {
    let mut a = A::default();
    a.handle(accept_at(0, b(5, 1), value(1, 0, "x"))).unwrap();
    assert!(a.endorse(b(5, 1), value(1, 0, "y")).is_err());
    a.endorse(b(5, 1), value(1, 0, "x")).unwrap();
    a.endorse(b(6, 1), value(1, 0, "y")).unwrap();
}

/// `value(version, 0, v)`, closing configuration 0 for configuration 1 with
/// `acceptors`.
fn closing(
    version: u64,
    v: &'static str,
    protected: bool,
    acceptors: &[u8],
) -> Agreed<u8, &'static str> {
    let mut closing = value(version, 0, v);
    closing.next = Some(Config {
        number: 1,
        protected,
        acceptors: acceptors.iter().copied().collect(),
    });
    closing
}

#[test]
fn refuses_to_close_for_a_configuration_that_cannot_agree() {
    for (protected, acceptors) in [(false, &[][..]), (true, &[1][..])] {
        let mut a = A::default();
        let dead_end = closing(1, "x", protected, acceptors);
        assert!(a.handle(accept_at(0, b(1, 1), dead_end.clone())).is_err());
        assert!(a.endorse(b(1, 1), dead_end).is_err());
        assert_eq!(a, A::default());
    }
    // One acceptor that isn't protected, or two that are, can agree.
    let mut a = A::default();
    a.handle(accept_at(0, b(1, 1), closing(1, "x", false, &[1])))
        .unwrap();
    a.endorse(b(2, 1), closing(1, "y", true, &[1, 2])).unwrap();
}

#[test]
fn does_not_learn_a_newer_version_in_an_older_configuration() {
    let mut a = A::default();
    a.handle(prepare(1, b(1, 1))).unwrap();
    let learned = Chosen {
        value: value(4, 2, "x"),
        ballot: None,
    };
    assert!(a.learn(learned.clone(), ()));
    // The shape of the failure the property test found: going back to
    // configuration 0 would make configuration 1 current again, after its
    // promises were dropped.
    let older = Chosen {
        value: value(5, 0, "y"),
        ballot: None,
    };
    assert!(!a.learn(older, ()));
    assert_eq!(a.learned(), Some(&learned));
    assert!(matches!(
        a.handle(prepare(1, b(1, 1))).unwrap().0,
        Reply::Stale { .. }
    ));
}

#[test]
fn does_not_learn_a_value_whose_next_configuration_cannot_agree() {
    let mut a = A::default();
    let dead_end = Chosen {
        value: closing(1, "x", true, &[1]),
        ballot: Some(b(1, 1)),
    };
    assert!(!a.learn(dead_end, ()));
    assert_eq!(a.learned(), None);
}

#[test]
fn a_refused_endorsement_leaves_the_state_as_it_was() {
    let mut a = A::default();
    a.handle(prepare(0, b(5, 1))).unwrap();
    a.handle(accept_at(0, b(5, 1), value(1, 0, "x"))).unwrap();
    let before = a.clone();
    // Below the promise, too high, and another value at an accepted ballot,
    // in this configuration and in one the acceptor has no slot for.
    for (ballot, value) in [
        (b(4, 1), value(1, 0, "y")),
        (b(MAX_COUNTER_STEP + 6, 1), value(1, 0, "y")),
        (b(5, 1), value(1, 0, "y")),
        (b(MAX_COUNTER_STEP + 1, 1), value(1, 3, "y")),
    ] {
        assert!(a.endorse(ballot, value).is_err());
        assert_eq!(a, before);
    }
}

#[test]
fn refuses_endorsements_that_handle_would_refuse() {
    // Below the promise.
    let mut a = A::default();
    a.handle(prepare(0, b(5, 1))).unwrap();
    assert!(a.endorse(b(4, 1), value(1, 0, "x")).is_err());
    a.endorse(b(5, 1), value(1, 0, "x")).unwrap();

    // Too far above the promise.
    let mut a = A::default();
    assert!(
        a.endorse(b(MAX_COUNTER_STEP + 1, 1), value(1, 0, "x"))
            .is_err()
    );
    a.endorse(b(MAX_COUNTER_STEP, 1), value(1, 0, "x")).unwrap();

    // In a configuration older than the learned value.
    let learned = Chosen {
        value: value(4, 1, "y"),
        ballot: None,
    };
    let mut a = A::with_learned(learned, ());
    assert!(a.endorse(b(1, 1), value(1, 0, "x")).is_err());
    a.endorse(b(1, 1), value(5, 1, "x")).unwrap();
}

#[test]
fn handle_proven_keeps_the_proof_with_the_accepted_value() {
    let mut a = Proven::default();
    // A promise keeps no proof.
    let (reply, changed) = a.handle_proven(prepare(0, b(1, 1)), 7).unwrap();
    assert!(changed);
    assert!(matches!(reply, Reply::Promise { .. }));
    assert_eq!(a.accepted_proof(0), None);

    let (reply, changed) = a
        .handle_proven(accept_at(0, b(1, 1), value(1, 0, "x")), 8)
        .unwrap();
    assert!(changed);
    assert!(matches!(reply, Reply::Accepted { .. }));
    assert_eq!(a.accepted_proof(0), Some(&8));

    // A refused request keeps the earlier proof.
    let before = a.clone();
    assert!(
        a.handle_proven(accept_at(0, b(1, 1), value(1, 0, "y")), 9)
            .is_err()
    );
    assert_eq!(a, before);
    let (reply, _) = a
        .handle_proven(accept_at(0, b(0, 1), value(1, 0, "y")), 9)
        .unwrap();
    assert!(matches!(reply, Reply::Rejected { .. }));
    assert_eq!(a.accepted_proof(0), Some(&8));
}

#[test]
fn learning_drops_endorsements_and_proofs_of_old_configurations() {
    let mut a = Proven::default();
    a.endorse(b(1, 1), value(1, 0, "x")).unwrap();
    a.handle_proven(accept_at(0, b(1, 1), value(1, 0, "x")), 5)
        .unwrap();
    a.endorse(b(1, 1), value(2, 1, "y")).unwrap();
    a.handle_proven(accept_at(1, b(1, 1), value(2, 1, "y")), 6)
        .unwrap();
    assert_eq!(a.proof(), None);

    let learned = Chosen {
        value: value(2, 1, "y"),
        ballot: Some(b(1, 1)),
    };
    assert!(a.learn(learned.clone(), 7));
    assert_eq!(a.learned(), Some(&learned));
    assert_eq!(a.proof(), Some(&7));
    assert_eq!(a.accepted_proof(0), None);
    assert_eq!(a.accepted_proof(1), Some(&6));
    assert_eq!(a.verified.keys().collect::<Vec<_>>(), [&1]);
}

mod properties {
    use std::collections::BTreeMap;

    use proptest::prelude::*;

    use super::*;
    use crate::strategies::{self, Node, Value};

    /// Something that can happen to an acceptor, from any node.
    #[derive(Clone, Debug)]
    enum Op {
        Handle(Request<Node, Value>),
        Endorse(Ballot<Node>, Agreed<Node, Value>),
        Learn(Chosen<Node, Value>),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => strategies::request().prop_map(Op::Handle),
            2 => (strategies::ballot(), strategies::agreed())
                .prop_map(|(b, v)| Op::Endorse(b, v)),
            1 => strategies::chosen().prop_map(Op::Learn),
        ]
    }

    type ByBallot = BTreeMap<(u64, Ballot<Node>), Agreed<Node, Value>>;

    /// Records `value` at `ballot` in `config`, and checks that it is the
    /// value recorded there before, if any.
    fn one_value_for_each_ballot(
        seen: &mut ByBallot,
        config: u64,
        ballot: &Ballot<Node>,
        value: &Agreed<Node, Value>,
    ) -> Result<(), TestCaseError> {
        let earlier = seen
            .entry((config, ballot.clone()))
            .or_insert(value.clone());
        prop_assert_eq!(&*earlier, value, "two values at {:?} in {}", ballot, config);
        Ok(())
    }

    proptest! {
        /// Whatever it is sent, an acceptor never panics, and keeps the
        /// promises that make agreement safe.
        #[test]
        fn an_acceptor_keeps_its_promises(
            start_with_genesis in any::<bool>(),
            ops in prop::collection::vec(op(), 0..40),
        ) {
            let mut a = if start_with_genesis {
                Acceptor::genesis(Chosen::genesis(0, 0), ())
            } else {
                Acceptor::default()
            };
            let mut accepted = ByBallot::new();
            let mut endorsed = ByBallot::new();
            for op in ops {
                let before = a.clone();
                let refused = match op {
                    Op::Handle(req) => a.handle(req).is_err(),
                    Op::Endorse(ballot, value) => a.endorse(ballot, value).is_err(),
                    Op::Learn(chosen) => {
                        a.learn(chosen, ());
                        false
                    }
                };
                if refused {
                    prop_assert_eq!(&a, &before, "a refusal changed the state");
                }
                let version = |a: &Acceptor<Node, Value>| a.learned().map(|c| c.value.version);
                prop_assert!(version(&a) >= version(&before), "the learned value went back");

                for (config, slot) in &a.slots {
                    if let Some(old) = before.slots.get(config) {
                        prop_assert!(slot.promised >= old.promised, "a promise went down");
                    }
                    if let Some((ballot, value)) = &slot.accepted {
                        prop_assert!(
                            slot.promised.as_ref() >= Some(ballot),
                            "accepted above the promise"
                        );
                        one_value_for_each_ballot(&mut accepted, *config, ballot, value)?;
                    }
                }
                for (config, (ballot, value)) in &a.verified {
                    one_value_for_each_ballot(&mut endorsed, *config, ballot, value)?;
                }
            }
        }
    }
}
