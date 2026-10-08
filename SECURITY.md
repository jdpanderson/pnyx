# Security

## Reporting a vulnerability

Please report vulnerabilities privately, through
[GitHub's private vulnerability reporting](https://github.com/jdpanderson/pnyx/security/advisories/new).
Don't open a public issue.

Please include the version of pnyx, what an attacker can do, and how to
reproduce it if you can.

## Supported versions

Fixes go into the latest release.

## What counts

pnyx's job is to keep a cluster from agreeing on two values. These are
vulnerabilities:

- The cluster agrees on two values, or an agreed value is lost, while every
  node follows the protocol, even with crashes and lost, delayed, repeated
  or reordered messages.
- A request or a reply makes pnyx panic. (Limits on the size of messages
  belong to the transport.)
- `store` loads a state that was never saved, or loses a saved state, after a
  crash.
- The parts for a Byzantine layer (see *Byzantine faults* in the crate
  documentation) don't keep the promises they document.

pnyx alone does not handle nodes that lie. A node that doesn't follow the
protocol can stop the cluster from agreeing, or make it agree on two values,
unless a layer above pnyx checks signed evidence. That alone is not a
vulnerability in pnyx.
