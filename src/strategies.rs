//! Generators of protocol values for the property tests. The domains are
//! small, so that generated values often meet: the same node, ballot and
//! configuration come up again and again. Ballot counters also include the
//! extremes around [`MAX_COUNTER_STEP`] and `u64::MAX`.

use proptest::prelude::*;

use crate::{Agreed, Ballot, Chosen, Config, MAX_COUNTER_STEP, Reply, Request};

pub(crate) type Node = u8;
pub(crate) type Value = u8;

pub(crate) fn node() -> impl Strategy<Value = Node> {
    0u8..5
}

pub(crate) fn counter() -> impl Strategy<Value = u64> {
    prop_oneof![
        8 => 0u64..6,
        1 => Just(MAX_COUNTER_STEP),
        1 => Just(MAX_COUNTER_STEP + 1),
        1 => Just(u64::MAX - 1),
        1 => Just(u64::MAX),
    ]
}

pub(crate) fn ballot() -> impl Strategy<Value = Ballot<Node>> {
    (counter(), node()).prop_map(|(counter, node)| Ballot { counter, node })
}

pub(crate) fn config_number() -> impl Strategy<Value = u64> {
    0u64..3
}

pub(crate) fn config() -> impl Strategy<Value = Config<Node>> {
    (
        config_number(),
        any::<bool>(),
        prop::collection::btree_set(node(), 0..5),
    )
        .prop_map(|(number, protected, acceptors)| Config {
            number,
            protected,
            acceptors,
        })
}

pub(crate) fn agreed() -> impl Strategy<Value = Agreed<Node, Value>> {
    (0u64..6, config(), prop::option::of(config()), 0u8..3).prop_map(
        |(version, config, next, value)| Agreed {
            version,
            config,
            next,
            value,
        },
    )
}

pub(crate) fn chosen() -> impl Strategy<Value = Chosen<Node, Value>> {
    (agreed(), prop::option::of(ballot())).prop_map(|(value, ballot)| Chosen { value, ballot })
}

pub(crate) fn request() -> impl Strategy<Value = Request<Node, Value>> {
    prop_oneof![
        (config_number(), ballot(), prop::option::of(ballot())).prop_map(
            |(config, ballot, have)| Request::Prepare {
                config,
                ballot,
                have
            }
        ),
        (config_number(), ballot(), agreed()).prop_map(|(config, ballot, value)| Request::Accept {
            config,
            ballot,
            value
        }),
    ]
}

pub(crate) fn reply() -> impl Strategy<Value = Reply<Node, Value>> {
    prop_oneof![
        (
            config_number(),
            ballot(),
            prop::option::of((ballot(), prop::option::of(agreed()))),
        )
            .prop_map(|(config, ballot, accepted)| Reply::Promise {
                config,
                ballot,
                accepted,
            }),
        (config_number(), ballot()).prop_map(|(config, ballot)| Reply::Accepted { config, ballot }),
        (config_number(), ballot(), ballot()).prop_map(|(config, ballot, promised)| {
            Reply::Rejected {
                config,
                ballot,
                promised,
            }
        }),
        chosen().prop_map(|learned| Reply::Stale { learned }),
        (config_number(), ballot(), counter()).prop_map(|(config, ballot, limit)| {
            Reply::TooHigh {
                config,
                ballot,
                limit,
            }
        }),
    ]
}
