# Roadmap

The roadmap is evidence-driven. A phase should not grow substantially until its
core claims are covered by tests and measurements.

## Phase 0 — Repository foundation

- [x] Define the Swarm Grid vision and non-goals.
- [x] Record the initial architecture.
- [x] Create a Rust workspace.
- [x] Add contribution, security, CI, and issue-management foundations.
- [ ] Select an open-source license.

## Phase 1 — Two-node MVP

Goal: prove that two independent computers behave as equal Swagri nodes.

- [x] Persistent node identity.
- [x] QUIC transport.
- [x] Local mDNS discovery.
- [x] Explicit address dialing fallback.
- [x] Bidirectional typed request/response.
- [x] Allowlisted built-in tasks.
- [x] Execution outside the network event loop.
- [ ] Test on two physical Windows/Linux/macOS devices.
- [ ] Measure local execution against remote execution.
- [ ] Document firewall and LAN troubleshooting.

Exit criterion: repeated two-device tests can send work in both directions,
survive a peer restart, reject invalid work, and produce reproducible timing
data.

## Phase 2 — Local cloud

- [x] Baseline CPU/memory capability and dynamic resource advertisement;
- [x] Cached CPU calibration and an operator-visible effective-capacity score;
- [x] Trusted P2P updates for both headless Agent and desktop Debugger;
- trust allowlists for the user's devices;
- task queue, cancellation, deadlines, and retry policy;
- [x] runtime contribution pause and advertised resource budgets;
- hard OS-enforced resource quotas;
- [x] local-first placement for bounded CPU benchmark and matrix tasks;
- [x] operator-visible in-memory task lifecycle and result history;
- [x] persistent local SQLite history for task lifecycle and results;
- thermal/load-aware chunk assignment;
- broader SQLite event and resource-measurement store;
- benchmark suite for scheduler decisions.

Exit criterion: a group of 3–5 heterogeneous trusted devices demonstrates that
adaptive placement can outperform a fixed placement policy for at least one
well-characterized workload without regressing unsuitable workloads.

## Phase 3 — Divisible workloads

- task graphs and independent chunks;
- [x] bounded independent chunks for the first matrix workload;
- [x] deterministic matrix result aggregation;
- content-addressed inputs and cached artifacts;
- retry and reassignment after disconnects;
- redundant execution for critical results;
- [x] local control-stream progress for matrix chunks;
- network streaming progress and partial results for general workloads;
- initial Wasm execution backend.

## Phase 4 — Wider swarm

- rendezvous and/or DHT research;
- NAT traversal and relay fallback;
- scoped peer reputation;
- verified capability claims;
- privacy-aware placement;
- defense against abusive task submission;
- service-traffic and coordination-cost measurements.

## Phase 5 — Adaptive swarm AI experiments

- Python SDK over the stable execution API;
- ONNX and local-model runtime adapters;
- model and data locality signals;
- multi-agent task graphs;
- response comparison and aggregation;
- privacy boundaries for sensitive agent stages;
- experiments comparing centralized and adaptive placement.

## Research questions

- Which task properties predict a benefit from remote execution?
- When does data transfer dominate compute gain?
- How quickly should a scheduler react to thermal or load changes?
- Which state should be shared without creating excessive coordination traffic?
- How can a node prove or estimate capability without trusting every claim?
- When is redundant execution worth its cost?
- Which swarm-AI workflows benefit from heterogeneous edge devices?
