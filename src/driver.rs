//! Runs one change over a transport, with timeouts and retries.

use std::{future::Future, time::Duration};

use futures_util::{StreamExt, stream::FuturesUnordered};
use rand::RngExt;
use tokio::time::{Instant, sleep_until, timeout};

use crate::{Agreed, Change, Chosen, Proposer, Reply, Request, Step};

/// Sends requests to acceptors. The proposer's own node may be one of them.
pub trait Transport<N, V>: Sync {
    /// Why a request failed. [`Error::Timeout`] keeps the last one as its
    /// source.
    type Error: std::error::Error + 'static;

    /// Sends `req` to the acceptor `to` and returns its reply.
    fn call(
        &self,
        to: &N,
        req: Request<N, V>,
    ) -> impl Future<Output = Result<Reply<N, V>, Self::Error>> + Send;

    /// The latest agreed value the proposer's node has learned in another
    /// way, such as from another proposer. It's checked before each new
    /// round: a change that began before a later configuration was learned
    /// would otherwise keep asking acceptors that may be gone.
    fn learned(&self) -> Option<Chosen<N, V>> {
        None
    }
}

/// Timing for [`propose`].
///
/// New fields may be added, so start from [`Options::default`] and set the
/// fields to change.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Options {
    /// How long one request may take.
    pub request_timeout: Duration,
    /// How long the whole change may take.
    pub deadline: Duration,
    /// The longest random wait before a new round, when another proposer is
    /// active.
    pub max_retry_wait: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            request_timeout: Duration::from_secs(5),
            deadline: Duration::from_secs(30),
            max_retry_wait: Duration::from_millis(200),
        }
    }
}

/// Why [`propose`] failed. `E` is the transport's error type.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error<E> {
    /// No quorum of acceptors answered before the deadline in
    /// [`Options::deadline`].
    #[error("no quorum of acceptors answered in time")]
    Timeout {
        /// The last request that failed, if any did.
        #[source]
        last: Option<RequestError<E>>,
    },
    /// See [`Step::OutOfBallots`].
    #[error("no ballots are left in this configuration")]
    OutOfBallots,
}

/// Why one request to an acceptor failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RequestError<E> {
    /// The transport failed.
    #[error("the transport failed")]
    Transport(#[source] E),
    /// No reply came within [`Options::request_timeout`].
    #[error("the request timed out")]
    TimedOut,
}

/// Runs one change: reads the current value, calls `change` with it, and has
/// the result agreed. `change` may be called more than once, if a round must
/// be retried; the result of the last call is returned with the agreed value.
///
/// Requests go to all acceptors of the configuration at once. When another
/// proposer is active, the next round starts after a random wait of up to
/// [`Options::max_retry_wait`].
///
/// # Errors
///
/// [`Error::Timeout`] if the change isn't agreed by [`Options::deadline`].
/// The change may still be agreed later, if a request already sent is
/// accepted: read the value again to find out. [`Error::OutOfBallots`] if
/// no ballots are left.
///
/// # Panics
///
/// Only if [`Proposer`] says a change is done before it was decided, which
/// is a bug in pnyx.
///
/// # Example
///
/// A cluster in memory, which grows from one acceptor to three. A real
/// transport sends each request over the network, and the acceptor saves
/// its state before it replies.
///
/// ```
/// use std::{collections::BTreeSet, convert::Infallible, sync::Mutex};
///
/// use pnyx::{
///     Acceptor, Agreed, Change, Chosen, Options, Proposer, Reply, Request, Transport, propose,
/// };
///
/// struct Memory(Mutex<Vec<Acceptor<u8, u64>>>);
///
/// impl Transport<u8, u64> for Memory {
///     type Error = Infallible;
///
///     async fn call(&self, to: &u8, req: Request<u8, u64>) -> Result<Reply<u8, u64>, Infallible> {
///         let mut acceptors = self.0.lock().unwrap();
///         let acceptor = &mut acceptors[usize::from(*to)];
///         let (reply, _changed) = acceptor.handle(req).expect("a valid request");
///         Ok(reply)
///     }
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// // Node 0 starts the cluster as its only acceptor.
/// let first = Chosen::genesis(0, 0);
/// let acceptors = vec![
///     Acceptor::genesis(first.clone(), ()),
///     Acceptor::default(),
///     Acceptor::default(),
/// ];
/// let transport = Memory(Mutex::new(acceptors));
/// let mut proposer = Proposer::new(0, first);
/// let options = Options::default();
///
/// // Add nodes 1 and 2 as acceptors.
/// let grow = |current: &Agreed<u8, u64>| {
///     let next = current.config.successor(BTreeSet::from([0, 1, 2]), false);
///     (Change::Close(current.value, next), ())
/// };
/// propose(&mut proposer, &transport, &options, grow).await.unwrap();
///
/// // Each change now needs a quorum of the three acceptors.
/// let add = |current: &Agreed<u8, u64>| (Change::Set(current.value + 1), ());
/// for _ in 0..3 {
///     propose(&mut proposer, &transport, &options, add).await.unwrap();
/// }
/// assert_eq!(proposer.known().state(), &3);
/// # }
/// ```
pub async fn propose<N, V, T, R>(
    proposer: &mut Proposer<N, V>,
    transport: &T,
    options: &Options,
    mut change: impl FnMut(&Agreed<N, V>) -> (Change<N, V>, R),
) -> Result<(Chosen<N, V>, R), Error<T::Error>>
where
    N: Clone + Ord,
    V: Clone,
    T: Transport<N, V>,
{
    let deadline = Instant::now() + options.deadline;
    let mut last_error = None;
    let mut result = None;
    let mut pending = FuturesUnordered::new();
    let send = |pending: &mut FuturesUnordered<_>, requests: Vec<(N, Request<N, V>)>| {
        for (to, req) in requests {
            pending.push(async move {
                let reply = timeout(options.request_timeout, transport.call(&to, req)).await;
                (to, reply)
            });
        }
    };
    let begin = |proposer: &mut Proposer<N, V>| {
        if let Some(chosen) = transport.learned() {
            proposer.learn(chosen);
        }
        proposer.begin()
    };
    let mut step = begin(proposer);
    // When to start the next round, after a failed one.
    let mut retry_at = None;
    loop {
        if let Step::Read(value) = step {
            let (c, r) = change(&value);
            result = Some(r);
            step = proposer.decide(c);
        }
        match step {
            Step::Wait => {}
            Step::Send(requests) => send(&mut pending, requests),
            Step::Read(_) => unreachable!("decide() doesn't read"),
            Step::Done(chosen) => {
                let result = result.expect("a change is decided before it's done");
                return Ok((chosen, result));
            }
            Step::Retry { wait: false } => {
                step = begin(proposer);
                continue;
            }
            Step::Retry { wait: true } => {
                let max = options.max_retry_wait.as_millis() as u64;
                let wait = Duration::from_millis(rand::rng().random_range(0..=max));
                retry_at = Some(Instant::now() + wait);
            }
            Step::OutOfBallots => return Err(Error::OutOfBallots),
        }
        // Answers to earlier rounds are still received: a stale answer tells
        // the proposer about a later configuration.
        let (from, reply) = tokio::select! {
            Some(answer) = pending.next() => answer,
            _ = sleep_until(retry_at.unwrap_or(deadline)), if retry_at.is_some() => {
                retry_at = None;
                step = begin(proposer);
                continue;
            }
            _ = sleep_until(deadline) => {
                proposer.abort();
                return Err(Error::Timeout { last: last_error });
            }
        };
        step = match reply {
            Ok(Ok(reply)) => proposer.receive(from, reply),
            Ok(Err(e)) => {
                last_error = Some(RequestError::Transport(e));
                proposer.unreachable(from)
            }
            Err(_) => {
                last_error = Some(RequestError::TimedOut);
                proposer.unreachable(from)
            }
        };
    }
}
