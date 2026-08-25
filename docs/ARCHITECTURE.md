# Architecture

This document describes the intended direction and clearly separates it from
the features implemented in the current prototype.

## Design principles

1. **Local first.** Avoid network coordination when one device is sufficient.
2. **Smallest useful area.** Recruit only the resources a task can use.
3. **Adaptive decisions.** Treat capabilities and availability as changing
   signals, not permanent labels.
4. **Temporary coordination.** A coordinator is a role for a task, not a
   mandatory global server.
5. **Explicit trust.** Encryption identifies a peer; it does not make the peer
   trustworthy.
6. **Typed execution.** A node accepts declared task types and resource limits,
   not implicit shell access.
7. **Measured utility.** Scheduling policy must be evaluated against local
   execution and network overhead.

## Current MVP

```text
┌──────────────────┐        QUIC + CBOR        ┌──────────────────┐
│ Swagri node A    │◄─────────────────────────►│ Swagri node B    │
│                  │                           │                  │
│ CLI              │                           │ CLI              │
│ request/response │                           │ request/response │
│ allowlisted exec │                           │ allowlisted exec │
└──────────────────┘                           └──────────────────┘
```

Every node has the same role and can initiate or execute requests. The MVP uses:

- QUIC as the encrypted, multiplexed transport;
- mDNS for local peer discovery;
- explicit multiaddresses as a fallback;
- libp2p PeerId values derived from persistent Ed25519 identities;
- CBOR messages on versioned protocol `/swagri/task/1`;
- separately signed Agent and Debugger manifests and chunks on `/swagri/update/1`;
- trusted artifact inventories, manifests, and blocks on `/swagri/artifact/1`;
- small, allowlisted task kinds;
- aggregate resource snapshots in `NodeInfo` responses;
- cached one-time CPU calibration and five-second dynamic sampling;
- local-first placement for bounded built-in CPU benchmark and matrix work;
- runtime contribution pause/resume with zero advertised capacity;
- Debugger-visible lifecycle events for local, outbound, and inbound tasks;
- request limits, execution limits, deadlines, and structured failures.

The prototype has no global discovery, relay, reputation, payment, arbitrary
code execution, or general distributed scheduler. Version 0.7 uses resource
scores for two constrained decisions: `auto-benchmark` and `auto-matrix` remain
local unless a connected compatible peer with a resource observation no older
than 20 seconds exceeds the local effective CPU score by at least 20%. If the
operator pauses local contribution, the local score becomes zero and a smart
task must use an eligible remote peer or fail explicitly.

## Workspace boundaries

### `swagri-core`

Owns stable task, result, capability, and failure types. It has no dependency on
libp2p so these types can later be reused by other transports and SDKs.

### `swagri-executor`

Validates and executes the allowlisted built-in tasks. Networking must not have
direct access to shell execution or dynamic native libraries.

### `swagri-storage`

Owns the local content-addressed store. It has no networking dependency. Files
are represented by a small manifest and immutable 256 KiB blocks; both complete
artifacts and individual blocks use SHA-256 identities. Imports use atomic
same-directory writes, identical blocks occupy disk once, and exports happen
only after every block and the complete artifact digest pass verification.

### `swagri-node`

Owns peer identity, discovery, transport, request/response coordination, and
the interactive CLI. Since 0.14.1-alpha it builds both an `rlib` and Android
`cdylib`; the CLI and Android foreground host feed commands into the same event
loop instead of maintaining separate protocol implementations.

### `android`

Owns only Android lifecycle and presentation concerns: foreground-service
activation, notification, mDNS multicast access, battery/thermal/network
sampling, user settings, and JNI calls. It must not reimplement trust,
transport, task validation, or artifact integrity in Kotlin.

## Request lifecycle

```text
user command
  -> validate local request
  -> select explicit peer, or local-first CPU placement for a smart task
  -> open request substream
  -> validate request on receiver
  -> execute with limits
  -> return typed result or failure
  -> record duration
```

Inbound work runs outside the networking event loop. A slow computation must
not stop peer discovery, heartbeats, or other responses.

Debugger correlates lifecycle events by typed request ID. It persists the
latest 1,000 completed records in a local SQLite database and restores the
newest 100 into the task panel on startup. Tasks left running when Debugger
closes are marked as interrupted on the next start.

Version 0.10 adds a bounded lifecycle for the first divisible workload:

```text
distributed matrix command
  -> validate matrix and chunk bounds
  -> build a bounded row-chunk queue
  -> select fresh protocol-compatible local/remote workers
  -> keep at most one chunk in flight per worker
  -> requeue a failed remote chunk onto another healthy worker, up to 3 attempts
  -> report completed chunks as truthful progress
  -> XOR the order-independent partial checksums
  -> finish one parent swarm task
```

This remains deliberately workload-specific. It proves concurrent assignment,
aggregation, and bounded reassignment without pretending that arbitrary programs
are safely divisible. Version 0.11 removes a worker from the current job after
its remote request fails, gives the exact chunk to another already-selected
healthy worker, and fails explicitly when no replacement remains or three
attempts are exhausted. General-workload retry, cancellation, and a task graph
remain future work. Other running tasks show elapsed wall time rather
than an invented percentage. The database is Debugger-local and stores task
metadata/results only; resource measurement persistence remains a later step.

Version 0.11.1 exposes two one-shot diagnostics through the local Agent stdin:
fail or delay the next inbound matrix chunk. They are not network requests and
cannot be armed by a remote peer. Non-matrix tasks do not consume them, and the
atomic state resets as soon as one inbound chunk takes it. This makes retry and
disconnect tests repeatable on two physical devices without weakening task
authorization.

## Content-addressed artifact storage

Version 0.12.0 deliberately separates storage correctness from network
distribution:

```text
local file
  -> stream in 256 KiB blocks (no whole-file RAM copy)
  -> SHA-256 every block and the complete byte stream
  -> check the node's disk quota before committing
  -> atomically place new immutable blocks in blocks/sha256/
  -> atomically publish manifests/sha256/<artifact-id>.json
  -> verify every block before reconstruction
```

The Agent defaults to offering at most 5% of the physical disk containing its
artifact directory (`--artifact-storage-percent`). This is a policy ceiling,
not reserved space: an empty cache consumes almost nothing. Import, verification,
and export use blocking workers so hashing a large video cannot stall QUIC,
mDNS, task progress, or resource polling.

Version 0.12.1 adds the first artifact wire protocol. Only Peer IDs in the
owner's persisted trust list can request or serve inventories, manifests, and
blocks. A receiver validates bounded manifest structure, finds already-present
blocks, requests missing blocks sequentially from the selected peer, verifies
each SHA-256 digest before committing it, and publishes the manifest only after
the reconstructed whole-file digest matches. If the connection drops, verified
blocks remain in CAS and the next fetch skips them.

Version 0.13.0 can combine inventories from explicitly trusted peers, select up
to eight providers for one content ID, and keep up to four block requests in
flight. Providers are selected round-robin. A request failure or disconnect
removes that provider from the active set, requeues its unfinished block, and
continues with healthy providers; every accepted block and the complete artifact
still pass the same SHA-256 boundary as a single-source transfer.

Replication or erasure coding
must be a separate durability policy: content addressing detects missing or
corrupt data but does not by itself preserve a file when its only node leaves.

## Identity and trust

The MVP persists an Ed25519 keypair and derives a stable PeerId from it. This
provides authenticated identity at the transport layer. A later trust policy
will distinguish:

- the user's own devices;
- explicitly trusted devices;
- verified external peers;
- unknown peers.

Sensitive tasks must not leave their allowed trust boundary. Trust policy and
resource capability are separate scheduler inputs.

Update trust is implemented separately from task authorization. The owner
enrolls a specific Peer ID, and manifests must verify against the public key
that derives that ID. A dedicated updater performs replacement only after the
complete executable matches its signed manifest, retains the previous binary,
and rolls back after an activation or health-check failure.

Agent and Debugger use different signature domains. This prevents a valid
Agent manifest from being replayed as a GUI update. Agent updates may run
automatically for an already trusted peer; Debugger replacement always requires
an explicit GUI action because it closes and restarts the control application.

## Resource snapshots and scoring

Version 0.4 and later advertise a deliberately small aggregate snapshot:

```text
static:  OS, architecture, CPU model, physical/logical cores, total memory
dynamic: smoothed host/Agent CPU, free memory, Agent memory, active tasks
policy:  owner CPU and memory contribution limits
score:   cached CPU calibration and effective available CPU score
```

The CPU calibration is a bounded 200 ms single-thread calculation, cached by a
hardware fingerprint and reused on later starts. Dynamic data is sampled every
five seconds by default. Agents exchange only totals and percentages—not
process names, files, user activity, or other private host details.

The initial effective CPU score is:

```text
calibrated strength × min(host free CPU, policy CPU remaining) / 100
```

Advertised allocatable memory is the smaller of currently available RAM and the
remaining owner-configured memory budget. The operator can pause contribution
at runtime: the Agent advertises zero effective CPU and allocatable memory and
rejects new compute requests while continuing to answer resource/version
queries. Already-running work is allowed to finish. These limits are scheduler
signals, not OS-enforced hard quotas yet. Android 0.14.1-alpha appends battery
percentage, charging state, simplified thermal status, and unmetered-network
state as optional backward-compatible fields. Mobile contribution works
without external power at 50% battery or above; below 50%, charging is
required. Unmetered Wi-Fi and a temperature below severe thermal status remain
mandatory. Desktop temperature,
accelerators, network-quality measurements, and workload-specific benchmarks
remain planned inputs.

## Android lifecycle boundary

Android is treated as an intermittent edge node, not an always-on daemon. The
owner explicitly starts a foreground contribution session, Android displays a
persistent notification, and the initial host bounds one session below six
hours. The Rust node retains verified artifact blocks and the existing matrix
retry logic tolerates lifecycle-driven disappearance. P2P self-update serving
is disabled on Android: a later signed APK flow must still hand installation to
the operating system and user.

## Adaptive scheduling direction

Version 0.7 extends the first measurable scheduler slice to two CPU-only,
zero-input built-in tasks: a synthetic benchmark and deterministic square
matrix multiplication. It compares the local score with fresh connected-peer
scores, filters by task protocol capability, applies a 20% remote-gain margin
as a coarse allowance for coordination cost, emits the decision for Debugger,
and executes outside the network event loop. This is not yet a general
scheduler: it has no measured latency model, queue, cancellation, retry, task
splitting, or data-locality input.

The scheduler should expand its candidate area in stages:

1. current device;
2. the user's own or trusted LAN devices;
3. verified nearby peers;
4. wider regional or global peers.

Expansion is justified only while expected improvement is positive.

## Execution backends

The long-term execution interface should support independent adapters:

```text
Executor
├── BuiltinExecutor       implemented first
├── WasmExecutor          portable sandboxed modules
├── OnnxExecutor          model inference
├── LlamaCppExecutor      local language models
├── PythonExecutor        controlled AI/research workflows
└── ContainerExecutor     isolated heavyweight jobs
```

WebAssembly is a promising portable sandbox, but it is not a universal solution
for GPU workloads. Each backend must declare capabilities, isolation properties,
and resource accounting.

## Planned protocol evolution

The request/response protocol is explicitly versioned. Breaking wire changes
must use a new protocol identifier and may be supported alongside the old
version during migration.

Future protocol families are expected for:

- capability advertisement;
- streaming task output;
- cancellation and deadlines;
- content-addressed artifacts;
- task graphs and result aggregation;
- scheduler observations and measurements.
