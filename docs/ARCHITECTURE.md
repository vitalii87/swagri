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

### `swagri-node`

Owns peer identity, discovery, transport, request/response coordination, and
the interactive CLI.

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

Version 0.8 emits a lightweight local control event when tracked work starts
and when it completes or fails. Debugger correlates those events by the typed
request ID and keeps the latest 100 records in memory. Running tasks show
elapsed wall time rather than an invented percentage: truthful percentage
progress requires a later streaming progress protocol and workload-specific
units. Both requester and executor can therefore observe the same remote task
from their own perspective without adding coordination traffic to the wire.

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
signals, not OS-enforced hard quotas yet. Temperature, accelerators, network
quality, energy state, and workload-specific benchmarks remain planned inputs.

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
