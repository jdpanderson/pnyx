//! [CASPaxos] in Rust: agreement on one value, changed by compare-and-set,
//! with no leader and no log.
//!
//! A cluster agrees on one value of type `V`. To change it, a [`Proposer`]
//! reads the current value from a quorum of [`Acceptor`]s, computes the new
//! value from it, and has a quorum accept the new value. Any node can propose
//! a change at any time; when two proposers compete, one of them retries.
//!
//! The set of acceptors can change while the cluster runs. A change can close
//! the current [`Config`] and name the acceptors of the next one (see
//! [`Change::Close`]). Every agreed value ([`Agreed`]) has a version and the
//! configuration it was agreed in.
//!
//! # Layers
//!
//! - **State machines.** [`Acceptor`] and [`Proposer`] do no I/O: the caller
//!   delivers requests and replies. This is why they can be model-checked,
//!   and why they work with any network, runtime and storage.
//! - **`propose`** (feature `tokio`) runs one change over a `Transport` that
//!   you implement, with timeouts and retries.
//! - **`store`** (feature `store`) keeps an acceptor's state on disk, and saves
//!   each change before the reply is sent.
//!
//! Both features are on by default.
//!
//! # Example
//!
//! This example drives the state machines by hand. It starts a cluster with
//! one acceptor, adds two more, and then changes the value. A real caller
//! sends each request over the network, and saves the acceptor's state
//! before it sends the reply.
//!
//! ```
//! use std::collections::{BTreeMap, BTreeSet, VecDeque};
//!
//! use pnyx::{Acceptor, Agreed, Change, Chosen, Proposer, Step};
//!
//! type Acceptors = BTreeMap<u8, Acceptor<u8, String>>;
//!
//! /// Runs one change, and delivers each request in order.
//! fn run(
//!     proposer: &mut Proposer<u8, String>,
//!     acceptors: &mut Acceptors,
//!     change: impl Fn(&Agreed<u8, String>) -> Change<u8, String>,
//! ) -> Chosen<u8, String> {
//!     let mut in_flight = VecDeque::new();
//!     let mut step = proposer.begin();
//!     loop {
//!         match step {
//!             Step::Send(requests) => in_flight.extend(requests),
//!             Step::Wait => {}
//!             Step::Read(current) => {
//!                 step = proposer.decide(change(&current));
//!                 continue;
//!             }
//!             Step::Retry { .. } => {
//!                 step = proposer.begin();
//!                 continue;
//!             }
//!             Step::Done(chosen) => return chosen,
//!             Step::OutOfBallots => panic!("no ballots are left"),
//!         }
//!         let (to, request) = in_flight.pop_front().expect("a request is in flight");
//!         let acceptor = acceptors.get_mut(&to).expect("a known acceptor");
//!         // If `changed` is true, a real acceptor saves its state here.
//!         let (reply, _changed) = acceptor.handle(request).expect("a valid request");
//!         step = proposer.receive(to, reply);
//!     }
//! }
//!
//! // Node 1 starts the cluster as its only acceptor.
//! let first = Chosen::genesis(1, "hello".to_string());
//! let mut acceptors = Acceptors::from([
//!     (1, Acceptor::genesis(first.clone(), ())),
//!     (2, Acceptor::default()),
//!     (3, Acceptor::default()),
//! ]);
//! let mut proposer = Proposer::new(1, first);
//!
//! // Close configuration 0, and name nodes 1, 2 and 3 as the acceptors of
//! // configuration 1.
//! run(&mut proposer, &mut acceptors, |current| {
//!     let next = current.config.successor(BTreeSet::from([1, 2, 3]), false);
//!     Change::Close(current.value.clone(), next)
//! });
//!
//! // Change the value. Now a quorum of the three acceptors must accept it.
//! let chosen = run(&mut proposer, &mut acceptors, |current| {
//!     Change::Set(format!("{}, world", current.value))
//! });
//! assert_eq!(chosen.state(), "hello, world");
//! assert_eq!(chosen.value.config.number, 1);
//! ```
//!
//! # Faults
//!
//! pnyx is safe when nodes crash and restart, and when messages are lost,
//! delayed, repeated or reordered, as long as:
//!
//! - each acceptor saves its state before it sends a reply (see
//!   [`Acceptor::handle`]), and
//! - every node follows the protocol.
//!
//! Proposers that compete can delay each other's changes, but can't make the
//! cluster agree on two values. The tests check the state machines with the
//! [Stateright] model checker, with crashes and changes of configuration.
//!
//! # Byzantine faults
//!
//! pnyx alone does not handle nodes that lie. It has the parts of a design
//! for one faulty acceptor that must live in the acceptor's saved state, so
//! that a layer above it can check signed evidence:
//!
//! - [`Config::protected`] uses larger quorums, so that any two quorums share
//!   at least two acceptors. With at most one faulty acceptor, any two
//!   quorums then share an honest one.
//! - [`Acceptor::endorse`] saves a promise and a candidate value before the
//!   acceptor signs that it has checked the candidate. An acceptor never
//!   endorses two values at one ballot, even after a restart.
//! - [`Acceptor::handle_proven`] saves a proof with an accepted value, and
//!   [`Acceptor::accepted_proof`] returns it. The proof type `P` is the
//!   caller's; pnyx only keeps it.
//! - A proposer changes only a value that it knows is agreed. If it reads a
//!   value that may not be agreed, it writes it back first. So each change
//!   builds on a value that the acceptors hold, and they can check the change.
//!
//! The layer above pnyx must sign and check every message. It must also check
//! that each claim is backed by a quorum ([`Config::quorum`]) of distinct
//! acceptors of a configuration it trusts. Before it signs that a candidate
//! is checked, it calls `endorse`. It accepts a value only with proof that a
//! quorum checked it, through `handle_proven`.
//!
//! The model checker does not check these parts, and they have not been
//! audited.
//!
//! [CASPaxos]: https://arxiv.org/abs/1802.07000
//! [Stateright]: https://crates.io/crates/stateright

#![cfg_attr(docsrs, feature(doc_cfg))]

mod acceptor;
#[cfg(feature = "tokio")]
mod driver;
mod proposer;
#[cfg(feature = "store")]
pub mod store;

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub use acceptor::{Acceptor, InvalidRequest, MAX_COUNTER_STEP};
#[cfg(feature = "tokio")]
pub use driver::{Error, Options, RequestError, Transport, propose};
pub use proposer::{Change, Proposer, Step};

/// A ballot: a counter and the proposer's node ID, so no two proposers use the
/// same ballot. Ordered by counter, then by node ID.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Ballot<N> {
    /// Goes up by one with each round of a proposer.
    pub counter: u64,
    /// The proposer's node ID.
    pub node: N,
}

/// A set of acceptors, with its configuration number.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(bound(deserialize = "N: Ord + Deserialize<'de>"))]
pub struct Config<N> {
    /// Goes up by one with each configuration.
    pub number: u64,
    /// Use quorums of which any two share at least two acceptors (see
    /// [`quorum`]). This is one part of handling a faulty acceptor; see
    /// *Byzantine faults* in the [crate documentation](crate). With four or
    /// more acceptors, a quorum can still form when one of them fails.
    pub protected: bool,
    /// The acceptors of this configuration.
    pub acceptors: BTreeSet<N>,
}

impl<N> Config<N> {
    /// The next configuration, with these acceptors.
    pub fn successor(&self, acceptors: BTreeSet<N>, protected: bool) -> Self {
        Self {
            number: self.number + 1,
            acceptors,
            protected,
        }
    }

    /// The number of acceptors that make a quorum in this configuration.
    pub fn quorum(&self) -> usize {
        quorum(self.acceptors.len(), self.protected)
    }

    /// Whether a value can ever be agreed in this configuration: a quorum
    /// is no larger than the set of acceptors. This is false with no
    /// acceptors, and for a protected configuration with one.
    ///
    /// Every change is agreed in the current configuration, including the
    /// one that closes it, so a cluster could never leave such a
    /// configuration. pnyx refuses to enter one.
    pub fn can_agree(&self) -> bool {
        self.quorum() <= self.acceptors.len()
    }
}

/// The number of acceptors, out of `n`, that make a quorum.
///
/// This is a majority, `n / 2 + 1`. If `protected` is true, it is
/// `(n + 1) / 2 + 1`, so that any two quorums share at least two acceptors.
pub fn quorum(n: usize, protected: bool) -> usize {
    (n + usize::from(protected)) / 2 + 1
}

/// The value that acceptors store: the user's value, its version, and the
/// configuration it was agreed in.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(bound(deserialize = "N: Ord + Deserialize<'de>, V: Deserialize<'de>"))]
pub struct Agreed<N, V> {
    /// Goes up by one with each change, so each agreed value has its own
    /// version.
    pub version: u64,
    /// The configuration this value belongs to.
    pub config: Config<N>,
    /// Set when this value closes `config`: the acceptors of the next
    /// configuration. Nothing else can be agreed in `config` after it.
    pub next: Option<Config<N>>,
    /// The value itself.
    pub value: V,
}

impl<N, V> Agreed<N, V> {
    /// The configuration used for new rounds: the successor once this value
    /// closes its own configuration, otherwise the one that agreed it.
    pub fn active_config(&self) -> &Config<N> {
        self.next.as_ref().unwrap_or(&self.config)
    }

    /// The acceptors used for new rounds.
    pub fn acceptors(&self) -> &BTreeSet<N> {
        &self.active_config().acceptors
    }
}

impl<N: Clone + Ord, V: Clone> Agreed<N, V> {
    /// The first value of a cluster, with `acceptor` as its only acceptor.
    pub fn genesis(acceptor: N, value: V) -> Self {
        Agreed {
            version: 0,
            config: Config {
                protected: false,
                number: 0,
                acceptors: BTreeSet::from([acceptor]),
            },
            next: None,
            value,
        }
    }

    /// The first value of the next configuration, if this value closes its
    /// own. Every proposer derives the same value from the same closing value.
    pub fn open(&self) -> Option<Self> {
        let next = self.next.as_ref()?;
        Some(Agreed {
            version: self.version + 1,
            config: next.clone(),
            next: None,
            value: self.value.clone(),
        })
    }
}

/// An agreed value, with the ballot it was accepted at in its configuration.
///
/// The ballot is `None` for the first value of a configuration that no
/// acceptor holds yet (see [`Agreed::open`]).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(bound(deserialize = "N: Ord + Deserialize<'de>, V: Deserialize<'de>"))]
pub struct Chosen<N, V> {
    /// The agreed value.
    pub value: Agreed<N, V>,
    /// The ballot at which a quorum accepted it, if any did yet.
    pub ballot: Option<Ballot<N>>,
}

impl<N, V> Chosen<N, V> {
    /// The agreed value itself (`value.value`), without its version and
    /// configuration.
    pub fn state(&self) -> &V {
        &self.value.value
    }
}

impl<N: Clone + Ord, V: Clone> Chosen<N, V> {
    /// The first value of a cluster, accepted by its only acceptor at ballot
    /// zero. See [`Acceptor::genesis`].
    pub fn genesis(acceptor: N, value: V) -> Self {
        Chosen {
            ballot: Some(Ballot {
                counter: 0,
                node: acceptor.clone(),
            }),
            value: Agreed::genesis(acceptor, value),
        }
    }
}

/// A request from a proposer to an acceptor.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(bound(deserialize = "N: Ord + Deserialize<'de>, V: Deserialize<'de>"))]
pub enum Request<N, V> {
    /// Promise to accept nothing below `ballot` in configuration `config`.
    /// `have` is the ballot of the value the proposer already holds for this
    /// configuration; the acceptor leaves that value out of its answer.
    Prepare {
        /// The configuration number.
        config: u64,
        /// The ballot of the new round.
        ballot: Ballot<N>,
        /// The ballot of the value the proposer holds, if any.
        have: Option<Ballot<N>>,
    },
    /// Accept `value` at `ballot` in configuration `config`.
    Accept {
        /// The configuration number.
        config: u64,
        /// The ballot of the round.
        ballot: Ballot<N>,
        /// The value to accept.
        value: Agreed<N, V>,
    },
}

/// A value an acceptor has accepted, with its ballot. The value is `None` if
/// it's the one the proposer said it has.
pub type Accepted<N, V> = (Ballot<N>, Option<Agreed<N, V>>);

/// An acceptor's answer to a [`Request`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(bound(deserialize = "N: Ord + Deserialize<'de>, V: Deserialize<'de>"))]
pub enum Reply<N, V> {
    /// The promise, with the last value accepted in this configuration.
    Promise {
        /// The configuration number of the request.
        config: u64,
        /// The ballot of the request.
        ballot: Ballot<N>,
        /// The last value accepted in this configuration, if any.
        accepted: Option<Accepted<N, V>>,
    },
    /// The value in the request is accepted.
    Accepted {
        /// The configuration number of the request.
        config: u64,
        /// The ballot of the request.
        ballot: Ballot<N>,
    },
    /// Refused, because the acceptor has promised at least `promised`.
    /// This lower bound is limited to [`MAX_COUNTER_STEP`] above the request,
    /// allowing large gaps to be recovered over several bounded rounds.
    Rejected {
        /// The configuration number of the request.
        config: u64,
        /// The ballot of the request.
        ballot: Ballot<N>,
        /// A lower bound on the ballot the acceptor has promised.
        promised: Ballot<N>,
    },
    /// The configuration is closed. `learned` is an agreed value from a later
    /// one.
    Stale {
        /// The latest agreed value the acceptor has learned.
        learned: Chosen<N, V>,
    },
    /// Refused, because the ballot's counter is above `limit`, the highest
    /// counter the acceptor takes now (see [`MAX_COUNTER_STEP`]).
    TooHigh {
        /// The configuration number of the request.
        config: u64,
        /// The ballot of the request.
        ballot: Ballot<N>,
        /// The highest ballot counter the acceptor takes now.
        limit: u64,
    },
}

#[cfg(test)]
mod strategies;
#[cfg(test)]
mod tests;
