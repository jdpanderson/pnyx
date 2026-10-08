//! A cluster of three nodes in one process, with each acceptor's state in
//! files. Two proposers add names to a list at the same time. Then the
//! acceptors are opened again from their files, as after a restart.
//!
//! Run it with `cargo run --example cluster`.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use pnyx::{
    Acceptor, Agreed, Change, Chosen, Options, Proposer, Reply, Request, Transport, propose,
    store::{self, Stored},
};

type Value = Vec<String>;
type Node = u8;

/// The acceptors, by node ID. A real transport sends each request over the
/// network to the node.
#[derive(Clone)]
struct Local(Arc<Vec<Mutex<Stored<Node, Value>>>>);

impl Transport<Node, Value> for Local {
    type Error = store::Error;

    async fn call(
        &self,
        to: &Node,
        req: Request<Node, Value>,
    ) -> Result<Reply<Node, Value>, store::Error> {
        let acceptors = self.0.clone();
        let to = usize::from(*to);
        // Saving the state waits for the disk, so it runs where blocking is
        // allowed.
        tokio::task::spawn_blocking(move || acceptors[to].lock().unwrap().handle(req))
            .await
            .expect("the acceptor doesn't panic")
    }
}

fn paths(dir: &Path) -> Vec<PathBuf> {
    (0..3).map(|n| dir.join(format!("acceptor-{n}"))).collect()
}

/// Adds `name` to the list.
fn add(name: &str) -> impl FnMut(&Agreed<Node, Value>) -> (Change<Node, Value>, ()) {
    move |current| {
        let mut names = current.value.clone();
        names.push(name.to_string());
        (Change::Set(names), ())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;

    // Node 0 starts the cluster as its only acceptor. Nodes 1 and 2 start
    // with empty acceptors.
    let first = Chosen::genesis(0, Vec::new());
    let mut acceptors = Vec::new();
    for (n, path) in paths(dir.path()).into_iter().enumerate() {
        let acceptor = if n == 0 {
            Stored::create(path, Acceptor::genesis(first.clone(), ()))?
        } else {
            Stored::open(path)?
        };
        acceptors.push(Mutex::new(acceptor));
    }
    let transport = Local(Arc::new(acceptors));
    let options = Options::default();

    // Make all three nodes acceptors.
    let mut alice = Proposer::new(0, first.clone());
    let grow = |current: &Agreed<Node, Value>| {
        let next = current.config.successor(BTreeSet::from([0, 1, 2]), false);
        (Change::Close(current.value.clone(), next), ())
    };
    propose(&mut alice, &transport, &options, grow).await?;

    // Two proposers on other nodes add names at the same time. When their
    // rounds meet, one of them retries with the other's change included.
    let mut bob = Proposer::new(1, first.clone());
    let mut carol = Proposer::new(2, first);
    let (b, c) = tokio::join!(
        propose(&mut bob, &transport, &options, add("bob")),
        propose(&mut carol, &transport, &options, add("carol")),
    );
    b?;
    c?;
    let (chosen, ()) = propose(&mut alice, &transport, &options, add("alice")).await?;
    println!("agreed: {:?}", chosen.state());

    // Open the acceptors again from their files.
    drop(transport);
    let reopened = paths(dir.path())
        .into_iter()
        .map(Stored::<Node, Value>::open)
        .collect::<Result<Vec<_>, _>>()?;
    let transport = Local(Arc::new(reopened.into_iter().map(Mutex::new).collect()));
    let mut dave = Proposer::new(0, chosen);
    let (chosen, ()) = propose(&mut dave, &transport, &options, add("dave")).await?;
    println!("after a restart: {:?}", chosen.state());
    Ok(())
}
