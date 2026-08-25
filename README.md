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
Debugger for test networks. It exchanges lightweight resource snapshots and
ranks connected devices by calibrated CPU strength adjusted for current load
and the owner's contribution limits. Version 0.10 adds the first genuinely
divisible workload: a deterministic matrix can be queued as bounded row chunks,
executed concurrently by the available local and remote Agents, and aggregated
into one result with truthful chunk progress. Debugger retains both the parent
job and every executor-visible chunk in its bounded SQLite task history. Version
0.11 requeues a failed remote matrix chunk and reassigns it to another healthy
worker, while bounding retries and failing explicitly if no worker remains.
Version 0.11.1 adds local one-shot fault and delay controls so this recovery can
be reproduced safely from two Debuggers without changing the wire protocol.
Version 0.12.0 adds the local storage foundation for large inputs: files are
split into immutable 256 KiB blocks, addressed and verified with SHA-256,
deduplicated, and held under a default 5% physical-disk quota. Block exchange
between peers is the next protocol step; this version intentionally proves the
local integrity boundary first. Swagri does **not** yet split, migrate, retry,
or cancel general workloads. Version 0.12.1 adds an explicitly trusted P2P
artifact protocol: a node can inspect a peer's bounded inventory, fetch a
manifest, download only missing blocks, retain verified blocks across an
interruption, and publish the artifact only after its complete digest matches.
Version 0.13.0 discovers matching content IDs in the inventories of trusted
Agents and fetches up to four missing blocks concurrently, rotating requests
across as many as eight providers. A failed provider is removed without
discarding verified blocks or stopping healthy sources. Durability replication
remains a future step. Version 0.14.1-alpha begins the heterogeneous mobile
stage: the Agent is now also a reusable Rust library, and an Android arm64 app
can run the same identity, QUIC, typed-task, resource, trust, and artifact
protocol inside an explicitly started foreground session. Mobile snapshots add
battery, charging, thermal, and unmetered-network signals; conservative policy
pauses contribution outside safe conditions. Swagri does not execute shell
commands, downloaded binaries, or arbitrary remote code.

## Technology

- Rust workspace
- Tokio asynchronous runtime
- rust-libp2p
- QUIC transport
- mDNS discovery on local networks
- CBOR request/response messages
- persistent Ed25519 node identities
- trusted-peer, signed and chunked P2P Agent and Debugger updates with rollback
- cached CPU calibration plus low-frequency CPU/RAM resource snapshots
- local-first placement for bounded CPU benchmark and matrix workloads
- bounded multi-Agent matrix chunk scheduling, progress, and result aggregation
- bounded retry and healthy-worker reassignment for failed remote matrix chunks
- local one-shot failure and delay injection for reproducible recovery tests
- content-addressed 256 KiB artifact blocks with SHA-256 integrity, deduplication,
  atomic writes, reconstruction, and a disk quota
- trusted peer inventory and resumable verified artifact block downloads
- bounded four-block parallel downloads with round-robin multi-provider failover
- runtime pause/resume of a device's Swagri contribution
- durable local SQLite lifecycle and result history for swarm tasks
- native Rust desktop Debugger with timestamped searchable/exportable logs,
  resource comparison, and controls
- Android arm64 test Agent with a Kotlin control surface, foreground lifecycle,
  mobile resource policy, peer controls, artifact import, and timestamped logs

See [Architecture](docs/ARCHITECTURE.md) for the design boundaries and
[Roadmap](docs/ROADMAP.md) for the staged research plan. Windows packages and
the Android test APK are described in [Installation](docs/INSTALLATION.md).

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
auto-benchmark 1000000
matrix <peer-id> 192
auto-matrix 320
distributed-matrix 768 96
artifact-import C:\data\video.mp4
artifact-list
artifact-verify sha256:<content-id>
artifact-peer-list <trusted-peer-id>
artifact-fetch <trusted-peer-id> sha256:<content-id>
artifact-fetch-many sha256:<content-id> <trusted-peer-id> <trusted-peer-id> ...
pause-resources
resume-resources
resources <peer-id>
```

Use `help` for all commands. If mDNS is unavailable, start a node with an
explicit `--dial <multiaddr>` copied from the other node's listen output.

The desktop Debugger provides buttons for discovery, connection testing,
allowlisted sample tasks, smart CPU tests, a divisible multi-Agent matrix test
with visible placement and progress, a local contribution pause, device-resource comparison,
version comparison, trusted P2P Agent and Debugger updates, firewall help, and
manual installer fallback. Agents
automatically attempt a QUIC connection after mDNS discovery; advanced users
can also use `connect`, `dial`, and `info` commands.

## Workspace

```text
crates/
├── swagri-core/       Protocol-neutral task and result types
├── swagri-executor/   Allowlisted local task execution
├── swagri-node/       Lightweight P2P Agent implementation
├── swagri-debugger/   Desktop test console and host telemetry
└── swagri-updater/    Atomic Agent replacement and rollback helper
android/               Kotlin UI and foreground host for the Rust Agent library
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
