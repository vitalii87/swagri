# Swagri

**Swagri** (*Swarm Grid*) is an experimental adaptive execution layer for
heterogeneous, independent, and intermittently available devices.

Instead of treating every task as a reason to activate the largest possible
cluster, Swagri starts with the smallest useful execution area and expands only
when additional resources are expected to help.

> **Local first. Expand when useful. Rebalance when conditions change.**

## Why Swagri?

Computing capacity is scattered across desktops, laptops, home servers, mobile
devices, accelerators, and edge hardware. Much of it is idle, but using it well
is harder than merely connecting the devices. Network cost, data locality,
temperature, energy, reliability, trust, and current user activity can make a
nominally faster node the worse choice.

Swagri explores whether a changing group of devices can behave as one adaptive
computing fabric:

- a task creates a temporary area of computation;
- suitable nearby or trusted nodes join first;
- the area expands only when expected benefit exceeds coordination cost;
- work shifts when a node slows down, overheats, disconnects, or becomes busy;
- the temporary group dissolves after the task is complete.

The long-term vision includes adaptive multi-agent and swarm-AI workloads, but
the execution layer is deliberately general-purpose. Potential applications
also include media processing, simulation, rendering, compilation, edge
computing, sensor processing, and other divisible workloads.

## Project status

Swagri is an engineering experiment in its **bootstrap/MVP phase**. The current
goal is intentionally small:

> Two independent computers can discover or dial each other, exchange typed
> requests in both directions, execute a small allowlisted task, and return a
> structured result.

The first prototype includes a lightweight headless Agent and a desktop
Debugger for test networks. It does **not** execute shell commands, downloaded
binaries, or arbitrary remote code.

## Technology

- Rust workspace
- Tokio asynchronous runtime
- rust-libp2p
- QUIC transport
- mDNS discovery on local networks
- CBOR request/response messages
- persistent Ed25519 node identities
- trusted-peer, signed and chunked P2P Agent updates with rollback
- native Rust desktop Debugger with live logs, commands, and host metrics

See [Architecture](docs/ARCHITECTURE.md) for the design boundaries and
[Roadmap](docs/ROADMAP.md) for the staged research plan. Ready-made and portable
Windows packages are described in [Installation](docs/INSTALLATION.md).

## Quick start

### Prerequisites

- Rust stable toolchain (Rust 1.95 or newer)
- two terminals, or two computers on the same local network

### Run two local nodes

In the first terminal:

```console
cargo run -p swagri-agent -- --name alpha --identity .swagri/alpha.key
```

In the second terminal:

```console
cargo run -p swagri-agent -- --name beta --identity .swagri/beta.key
```

The nodes should discover each other through mDNS. Type `peers` to list known
peers, then submit a task using the peer ID printed by the other node:

```text
echo <peer-id> hello from alpha
sum <peer-id> 1 2 3.5
sha256 <peer-id> Swagri
benchmark <peer-id> 1000000
```

Use `help` for all commands. If mDNS is unavailable, start a node with an
explicit `--dial <multiaddr>` copied from the other node's listen output.

The desktop Debugger provides buttons for discovery, connection testing,
allowlisted sample tasks, version comparison, trusted P2P Agent updates,
firewall help, and manual Debugger update installation. Agents automatically
attempt a QUIC connection after mDNS
discovery; advanced users can also use `connect`, `dial`, and `info` commands.

## Workspace

```text
crates/
├── swagri-core/       Protocol-neutral task and result types
├── swagri-executor/   Allowlisted local task execution
├── swagri-node/       Lightweight P2P Agent implementation
├── swagri-debugger/   Desktop test console and host telemetry
└── swagri-updater/    Atomic Agent replacement and rollback helper
```

## Safety

Swagri is experimental software. Do not expose the prototype to untrusted
networks or use it for sensitive data. Read [SECURITY.md](SECURITY.md) before
testing across machines.

## Contributing

The project values measured results over architectural assumptions. See
[CONTRIBUTING.md](CONTRIBUTING.md) for development commands and the research
workflow.

## License

No open-source license has been selected yet. Until one is added, the source is
provided for evaluation only and all rights remain with the copyright holder.
