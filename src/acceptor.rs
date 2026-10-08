//! The acceptor: stores promises and accepted values, one slot for each
//! configuration.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Agreed, Ballot, Chosen, Reply, Request};

/// What an acceptor stores for one configuration.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(bound(deserialize = "N: Ord + Deserialize<'de>, V: Deserialize<'de>"))]
struct Slot<N, V> {
    promised: Option<Ballot<N>>,
    accepted: Option<(Ballot<N>, Agreed<N, V>)>,
}

impl<N, V> Default for Slot<N, V> {
    fn default() -> Self {
        Slot {
            promised: None,
            accepted: None,
        }
    }
}

/// An acceptor's state.
///
/// Each change to it must be saved before the reply is sent: an acceptor
/// that forgets a promise or an accepted value after a restart can make the
/// cluster agree on two values. The `store` module (feature `store`) does
/// this.
///
/// `P` is a proof that the caller keeps with each accepted and each learned
/// value, so that both are saved together, such as a signed certificate.
/// pnyx doesn't look at it; use `()` if there is none. See *Byzantine faults*
/// in the [crate documentation](crate).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(bound(
    deserialize = "N: Ord + Deserialize<'de>, V: Deserialize<'de>, P: Deserialize<'de>"
))]
pub struct Acceptor<N, V, P = ()> {
    slots: BTreeMap<u64, Slot<N, V>>,
    /// Last verification endorsement in each configuration, saved before signing.
    verified: BTreeMap<u64, (Ballot<N>, Agreed<N, V>)>,
    /// Evidence authorizing the accepted value, saved atomically with that vote.
    accepted_proofs: BTreeMap<u64, P>,
    /// The latest agreed value this acceptor has learned, with its proof.
    /// Slots of earlier configurations are dropped, and requests for them get
    /// this value.
    learned: Option<(Chosen<N, V>, P)>,
}

impl<N, V, P> Default for Acceptor<N, V, P> {
    fn default() -> Self {
        Acceptor {
            slots: BTreeMap::new(),
            verified: BTreeMap::new(),
            accepted_proofs: BTreeMap::new(),
            learned: None,
        }
    }
}

/// How far above its promise in a configuration an acceptor lets a ballot
/// counter go. Proposers add one for each round, so they stay far below this.
/// It stops a faulty member from using up all the counters with one request:
/// with 64-bit counters, that takes about 2^44 requests.
pub const MAX_COUNTER_STEP: u64 = 1 << 20;

/// A request that no proposer following the protocol sends.
#[derive(Debug, thiserror::Error)]
#[error("invalid request: {0}")]
pub struct InvalidRequest(&'static str);

impl<N: Clone + Ord, V: Clone + PartialEq, P: Clone> Acceptor<N, V, P> {
    /// The only acceptor of a new cluster, holding its first value.
    ///
    /// # Panics
    ///
    /// If `first.ballot` is `None`. [`Chosen::genesis`] makes a value with a
    /// ballot.
    pub fn genesis(first: Chosen<N, V>, proof: P) -> Self {
        let ballot = first.ballot.clone().expect("genesis has a ballot");
        let slot = Slot {
            promised: Some(ballot.clone()),
            accepted: Some((ballot, first.value.clone())),
        };
        Acceptor {
            verified: BTreeMap::new(),
            accepted_proofs: BTreeMap::from([(first.value.config.number, proof.clone())]),
            slots: BTreeMap::from([(first.value.config.number, slot)]),
            learned: Some((first, proof)),
        }
    }

    /// An acceptor that has promised nothing and knows the agreed value
    /// `chosen`, such as that of a node joining a cluster. It answers requests
    /// for older configurations as stale: the node may have been an acceptor
    /// in one of them before, and lost that state when it left.
    pub fn with_learned(chosen: Chosen<N, V>, proof: P) -> Self {
        Acceptor {
            slots: BTreeMap::new(),
            verified: BTreeMap::new(),
            accepted_proofs: BTreeMap::new(),
            learned: Some((chosen, proof)),
        }
    }

    /// The latest agreed value this acceptor has learned.
    pub fn learned(&self) -> Option<&Chosen<N, V>> {
        self.learned.as_ref().map(|(c, _)| c)
    }

    /// The proof of the learned value.
    pub fn proof(&self) -> Option<&P> {
        self.learned.as_ref().map(|(_, p)| p)
    }

    /// The proof saved with the value accepted in configuration `config`
    /// (see [`Acceptor::handle_proven`]).
    pub fn accepted_proof(&self, config: u64) -> Option<&P> {
        self.accepted_proofs.get(&config)
    }

    /// Records that this acceptor has checked `value` as the candidate at
    /// `ballot`. Call it after checking the round's evidence, and save the
    /// new state before signing the endorsement. It also promises `ballot`.
    ///
    /// The acceptor never endorses two values at one ballot. A higher ballot
    /// can endorse another value, so that a value from a round that stopped
    /// can be replaced.
    ///
    /// # Errors
    ///
    /// If the configuration of `value` is older than the learned value, if
    /// `ballot` is below the promise or too far above it (see
    /// [`MAX_COUNTER_STEP`]), if another value was endorsed or accepted at
    /// `ballot`, or if `value` closes its configuration for one in which no
    /// value can be agreed.
    pub fn endorse(
        &mut self,
        ballot: Ballot<N>,
        value: Agreed<N, V>,
    ) -> Result<(), InvalidRequest> {
        if closes_for_a_dead_end(&value) {
            return Err(InvalidRequest(DEAD_END));
        }
        let config = value.config.number;
        if self
            .learned()
            .is_some_and(|c| c.value.config.number > config)
        {
            return Err(InvalidRequest("verification in a retired configuration"));
        }
        // Check without changing the state, so that a refusal leaves it as
        // it was.
        let slot = self.slots.get(&config);
        let promised = slot.and_then(|s| s.promised.as_ref());
        if promised.is_some_and(|p| p > &ballot) {
            return Err(InvalidRequest("verification below the promise"));
        }
        if ballot.counter
            > promised
                .map_or(0, |p| p.counter)
                .saturating_add(MAX_COUNTER_STEP)
        {
            return Err(InvalidRequest("verification ballot too high"));
        }
        let conflicts = |(b, v): &(Ballot<N>, Agreed<N, V>)| b == &ballot && v != &value;
        if self.verified.get(&config).is_some_and(conflicts)
            || slot
                .and_then(|s| s.accepted.as_ref())
                .is_some_and(conflicts)
        {
            return Err(InvalidRequest("conflicting verification at one ballot"));
        }
        self.slots.entry(config).or_default().promised = Some(ballot.clone());
        self.verified.insert(config, (ballot, value));
        Ok(())
    }

    /// Like [`Acceptor::handle`], but if the value in an accept request is
    /// accepted, `proof` is saved with it. [`Acceptor::accepted_proof`]
    /// returns it.
    ///
    /// # Errors
    ///
    /// As for [`Acceptor::handle`].
    pub fn handle_proven(
        &mut self,
        req: Request<N, V>,
        proof: P,
    ) -> Result<(Reply<N, V>, bool), InvalidRequest> {
        let (reply, changed) = self.handle(req)?;
        if let Reply::Accepted { config, .. } = &reply {
            self.accepted_proofs.insert(*config, proof);
            return Ok((reply, true));
        }
        Ok((reply, changed))
    }

    /// The value this acceptor accepted in `config` at `ballot`, if any. A
    /// proposer that has had it agreed says so, and the acceptor can then
    /// learn it without fetching it.
    pub fn accepted(&self, config: u64, ballot: &Ballot<N>) -> Option<&Agreed<N, V>> {
        match &self.slots.get(&config)?.accepted {
            Some((b, v)) if b == ballot => Some(v),
            _ => None,
        }
    }

    /// The values this acceptor has accepted, one at most for each
    /// configuration it keeps.
    pub fn accepted_values(&self) -> impl Iterator<Item = &Agreed<N, V>> {
        self.slots
            .values()
            .filter_map(|s| s.accepted.as_ref().map(|(_, v)| v))
    }

    /// Answers a request. The second value is true if the state changed, and
    /// must then be saved before the reply is sent.
    ///
    /// # Errors
    ///
    /// If the request is one that no proposer following the protocol sends:
    /// an accept request with a value from another configuration, with a
    /// value that closes its configuration for one in which no value can be
    /// agreed, or with another value at a ballot that this acceptor accepted
    /// a value at.
    pub fn handle(&mut self, req: Request<N, V>) -> Result<(Reply<N, V>, bool), InvalidRequest> {
        let config = match &req {
            Request::Prepare { config, .. } | Request::Accept { config, .. } => *config,
        };
        if let Some((learned, _)) = &self.learned
            && learned.value.config.number > config
        {
            let learned = learned.clone();
            return Ok((Reply::Stale { learned }, false));
        }
        let ballot = match &req {
            Request::Prepare { ballot, .. } | Request::Accept { ballot, .. } => ballot,
        };
        let promised = self.slots.get(&config).and_then(|s| s.promised.as_ref());
        let limit = promised
            .map_or(0, |p| p.counter)
            .saturating_add(MAX_COUNTER_STEP);
        if ballot.counter > limit {
            let ballot = ballot.clone();
            return Ok((
                Reply::TooHigh {
                    config,
                    ballot,
                    limit,
                },
                false,
            ));
        }
        let slot = self.slots.entry(config).or_default();
        let fresh = slot.promised.is_none();
        match req {
            Request::Prepare { ballot, have, .. } => {
                // Each ballot is promised at most once, so at most one round
                // with a ballot reaches a majority, even if a proposer that
                // restarted uses the ballot again.
                if let Some(promised) = &slot.promised
                    && ballot <= *promised
                {
                    let promised = promised.clone();
                    return Ok((reject(config, ballot, promised), false));
                }
                slot.promised = Some(ballot.clone());
                let accepted = slot.accepted.as_ref().map(|(b, v)| {
                    let value = (have.as_ref() != Some(b)).then(|| v.clone());
                    (b.clone(), value)
                });
                let reply = Reply::Promise {
                    config,
                    ballot,
                    accepted,
                };
                Ok((reply, true))
            }
            Request::Accept { ballot, value, .. } => {
                let invalid = if value.config.number != config {
                    Some("value is from another configuration")
                } else if closes_for_a_dead_end(&value) {
                    Some(DEAD_END)
                } else {
                    None
                };
                if let Some(reason) = invalid {
                    if fresh {
                        self.slots.remove(&config);
                    }
                    return Err(InvalidRequest(reason));
                }
                if let Some(promised) = &slot.promised
                    && ballot < *promised
                {
                    let promised = promised.clone();
                    return Ok((reject(config, ballot, promised), false));
                }
                let changed = match &slot.accepted {
                    Some((b, v)) if *b == ballot => {
                        if *v != value {
                            return Err(InvalidRequest("another value at an accepted ballot"));
                        }
                        false
                    }
                    _ => true,
                };
                slot.promised = Some(ballot.clone());
                slot.accepted = Some((ballot.clone(), value));
                Ok((Reply::Accepted { config, ballot }, changed))
            }
        }
    }

    /// Records an agreed value, and drops the slots of earlier
    /// configurations. Returns true if the state changed.
    ///
    /// These values are ignored, because no agreed value looks like them, so
    /// only a faulty node sends one:
    ///
    /// - a value whose configuration for new rounds can't agree on anything
    ///   (see [`Config::can_agree`](crate::Config::can_agree)), and
    /// - a value with a newer version but an older configuration than the
    ///   learned value. Learning it would bring back configurations that
    ///   this acceptor has already dropped.
    pub fn learn(&mut self, chosen: Chosen<N, V>, proof: P) -> bool {
        if !chosen.value.active_config().can_agree() {
            return false;
        }
        if let Some((learned, _)) = &self.learned
            && (learned.value.version >= chosen.value.version
                || learned.value.config.number > chosen.value.config.number)
        {
            return false;
        }
        let config = chosen.value.config.number;
        self.slots = self.slots.split_off(&config);
        self.verified = self.verified.split_off(&config);
        self.accepted_proofs = self.accepted_proofs.split_off(&config);
        self.learned = Some((chosen, proof));
        true
    }
}

const DEAD_END: &str = "no value can be agreed in the next configuration";

/// Whether `value` closes its configuration for one in which no value can be
/// agreed. The cluster could never leave that configuration.
fn closes_for_a_dead_end<N, V>(value: &Agreed<N, V>) -> bool {
    value.next.as_ref().is_some_and(|next| !next.can_agree())
}

fn reject<N, V>(config: u64, ballot: Ballot<N>, mut promised: Ballot<N>) -> Reply<N, V> {
    // A bounded lower bound lets a restarted proposer catch up over several
    // rounds without accepting an arbitrarily large counter from one reply.
    promised.counter = promised
        .counter
        .min(ballot.counter.saturating_add(MAX_COUNTER_STEP));
    Reply::Rejected {
        config,
        ballot,
        promised,
    }
}

#[cfg(test)]
mod tests;
