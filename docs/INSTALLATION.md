# Installation and test builds

Swagri provides two Windows x64 applications and an experimental Android arm64
test Agent.

- **Swagri Agent** is the lightweight, headless peer for computers that donate
  resources. It contains no GUI.
- **Swagri Debugger** is the desktop test console. Its package also contains an
  agent, so it can start and control a local peer, discover nearby agents,
  test connections and tasks with buttons, compare versions, display local and
  peer resource capacity, and provide an optional technical console.

## Ready-made packages

Each packaging run produces four files:

| File | Purpose |
| --- | --- |
| `Swagri-Agent-Setup-x64.exe` | Per-user installer for a lightweight agent |
| `Swagri-Agent-Portable-x64.zip` | Installer-free lightweight agent |
| `Swagri-Debugger-Setup-x64.exe` | Per-user GUI installer, including an agent |
| `Swagri-Debugger-Portable-x64.zip` | Installer-free GUI and agent pair |

The installers do not require administrator privileges. They add Start Menu
shortcuts; the Debugger also adds a desktop shortcut. Portable packages can be
unzipped anywhere and run directly.

For development branches, open the repository's **Actions**, choose **Windows
packages**, run the workflow, and download its `swagri-windows-x64` artifact.
Tagged versions can later publish the same files as release assets.

## Android 0.14.0-alpha test APK

The **Android APK** workflow produces `app-debug.apk` in the
`swagri-android-arm64` artifact. The first build supports 64-bit ARM devices on
Android 10 or newer and is intended for private LAN testing, not Play Store
distribution.

1. Download the workflow artifact and transfer `app-debug.apk` to the phone or
   tablet.
2. Permit installation from the selected file-manager source and install it.
3. Connect Android and the Windows devices to the same private Wi-Fi network.
4. Open Swagri, accept notification permission, keep the default conservative
   limits, and tap **Start agent**.
5. Android shows a persistent notification while its P2P session is active.
   Tap **Stop agent** or the notification's **Stop** action to end it.
6. Tap **Find / refresh**. If multicast is blocked by the router, paste the
   Windows Agent's complete `/ip4/.../udp/.../quic-v1/p2p/...` address into the
   peer field and tap **Connect**.

Mobile contribution pauses automatically unless the device is charging, at
least 50% charged, connected to unmetered Wi-Fi, and below severe Android
thermal status. The node remains available for identity/resource diagnostics
while contribution is paused. Android update serving is disabled; new APKs
require normal Android installation confirmation.

To build locally, install JDK 17, Android SDK platform/build-tools 35, NDK
27.2.12479018, Gradle 8.10.2, the Rust `aarch64-linux-android` target, and
`cargo-ndk`, then run from `android/`:

```console
gradle :app:assembleDebug
```

## Test with two computers

1. Install or unpack Swagri Debugger on the first Windows computer.
2. Install or unpack Swagri Agent on the second computer.
3. Start both on the same trusted local network. Windows Firewall may ask for
   permission for private networks because Swagri uses QUIC and mDNS.
4. In Debugger, click **Start agent**. On the headless computer, start:

   ```console
   swagri-agent.exe --name second-computer
   ```

5. Run `peers` on each console. Copy a peer ID and test:

   ```text
   echo <peer-id> hello from Swagri
   sum <peer-id> 1 2 3
   benchmark <peer-id> 1000000
   ```

The Agent stores its identity in `%LOCALAPPDATA%\Swagri\identity.key`. Debugger
uses `%LOCALAPPDATA%\Swagri\debugger.key`. Keep these files if the devices
should retain their peer identities.

## Resource collection and placement

The Agent samples aggregate CPU and memory data every five seconds and exposes
it to connected peers through `resources <peer-id>` (the `info` command is an
alias). A short CPU calibration runs only when the hardware cache is missing or
no longer matches the device. Debugger shows calibrated strength, current host
load, free and potentially allocatable RAM, Agent usage, active tasks, and an
effective-capacity recommendation.

Use the Debugger's advanced settings before starting its Agent, or configure a
headless Agent directly:

```console
swagri-agent.exe --name second-computer --max-cpu-percent 60 --max-memory-percent 40
```

These percentages currently control what the Agent advertises as available to
future scheduling. They are not yet hard operating-system quotas. Swagri sends
no process list, filenames, or user-activity details in the snapshot.

Version 0.10 provides **Smart CPU test**, **Smart Matrix task**, and the first
**Distributed Matrix 768×768** task in Debugger.
The equivalent console commands are:

```text
auto-benchmark 1000000
auto-matrix 320
distributed-matrix 768 96
```

The Agent compares its current effective CPU score with resource observations
received from connected peers. It executes locally unless the strongest fresh,
compatible peer is at least 20% better, which provides a simple allowance for
network and coordination overhead. Debugger shows the chosen device, both
scores, the required threshold, completion time, and the matrix checksum.
The distributed command creates eight 96-row chunks, gives at most one chunk at
a time to each eligible Agent, reuses a worker when it finishes, and XOR-folds
the partial checksums into one deterministic result. Ordinary tasks are not yet
automatically divided, migrated, retried, or cancelled.

### Force a smart task onto the other computer

This is the recommended two-Debugger placement test for version 0.10:

1. Start version 0.10 on both computers and wait until each lists the other as
   connected with resource data.
2. On computer A, click **Block resources of this PC**. Its local effective
   strength becomes zero and the status says that contribution is paused.
3. On computer A, click **Smart Matrix task**. The scheduler must select
   computer B and show a remote placement decision.
4. Wait for a successful result containing `matrix 320x320`, a checksum, and
   the execution time.
5. Click **Allow resources of this PC** to resume normal placement.

The pause is intentionally safe and lightweight: it blocks only *new Swagri
compute tasks*. It does not reserve CPU in Windows, stop other applications, or
interrupt a task already running. If no compatible remote Agent is available,
the smart task fails visibly instead of silently running on the paused PC.

### Test a task split across the swarm

1. Allow Swagri resources on both computers and refresh resources/versions.
2. On either Debugger click **Distributed Matrix 768×768**.
3. In **Swarm tasks**, watch the parent job progress from `0/8` to `8/8` and
   inspect the child rows to see exactly which named computer executed each
   chunk.
4. Block local Swagri resources and run it again. Every new child chunk should
   show the other computer as executor; Windows and non-Swagri programs remain
   unaffected.

Both Agents must be version 0.10 or newer because matrix chunks use protocol
version 4.

Version 0.10.1 fixes the initial 0.10.0 test build losing queued chunks while
all workers were busy. It also marks every active Debugger task as interrupted
immediately when its managed Agent stops, so elapsed timers cannot continue for
work that is no longer running. Use 0.10.1 or newer for distributed-matrix
testing.

Version 0.11 adds bounded recovery for remote matrix chunks. If a remote request
fails or a remote Agent rejects the chunk, the failed attempt remains visible in
the task history and the coordinator queues a new attempt on another healthy
worker. The parent task keeps its completed-chunk count, performs at most three
attempts per chunk, and fails explicitly instead of hanging when no healthy
replacement remains. To test reassignment, use at least three Agents (or leave
the coordinator's local resources enabled), start a distributed matrix, and
stop one remote Agent while it owns a chunk.

Version 0.11.1 makes the same test deterministic with two Debuggers:

1. Start both Agents, allow Swagri resources on both, and refresh resources.
2. On computer B click **Збій наступного chunk**. This arms only its next
   inbound matrix chunk and cannot be triggered remotely.
3. On computer A click **Distributed Matrix 768×768**.
4. Computer A should retain a failed child attempt from B, queue a row with a
   `retry-2` task ID on its healthy local worker, and complete the parent at
   `8/8` with the usual deterministic checksum.
5. To inspect an in-flight chunk or test a real process stop, arm **Затримати
   chunk на 5 с** on computer B before starting the matrix on A.

Both diagnostic controls are one-shot. Echo, resource polling, and other task
types do not consume them. They affect only matrix chunks received from another
Agent, not work coordinated locally on the same computer.

## Test the v0.12 local artifact store

Start Debugger and expand **Files of the swarm (CAS)**. Click **Add file** and
choose any test file, including a video. Agent hashes it on a background worker,
splits it into immutable 256 KiB blocks, and displays its `sha256:` content ID,
size, block count, used storage, and quota. Select the row and click **Verify
integrity** to reread and hash every block.

Importing the same file twice should keep the same content ID and should not
duplicate its blocks. Files with repeated identical 256 KiB regions also reuse
those blocks. Headless commands are:

```text
artifact-status
artifact-import C:\data\video.mp4
artifact-list
artifact-verify sha256:<content-id>
artifact-export sha256:<content-id> C:\data\restored-video.mp4
```

The default quota is 5% of the physical disk that contains the artifact store.
Change it with `--artifact-storage-percent`, from 0.1 to 25. Version 0.12.0 is
the local integrity foundation; version 0.12.1 adds resumable trusted-peer
block transfer on top of it.

### Test trusted P2P artifact transfer in v0.12.1

1. Run Debugger 0.12.1 on both computers and connect the Agents.
2. On each Debugger select the other Agent and click **Trust peer for files**.
   Trust is deliberately mutual: trusting B on A does not silently make A
   trusted on B.
3. On computer A click **Add file** and wait for its verified local CAS row.
4. On computer B select A, click **Files of selected peer**, select the remote
   row, then click **Resume selected file**.
5. B downloads only blocks it does not already have, verifies every SHA-256,
   verifies the complete artifact, and then adds it to its local table.

To test resume, use a sufficiently large file, stop A during transfer, reconnect,
and repeat the inventory/fetch action. B retains verified blocks from the first
attempt and reports them as reused. Version 0.12.1 downloads from one selected
trusted Agent.

### Test multi-provider artifact transfer in v0.13.0

1. Run Debugger 0.13.0 on three computers, connect the Agents, and establish
   explicit mutual trust between the receiving computer and both providers.
2. Add the same file on provider A and provider B. Its Content ID must match on
   both machines.
3. On the receiver click **Files of all trusted peers**. Select either row for
   the shared Content ID and click **Resume from available sources**.
4. The receiver requests up to four blocks concurrently and rotates them across
   the available providers. The log records the provider used for each progress
   event and reports if a source drops out.
5. Stop one provider during a sufficiently large transfer. The remaining
   provider continues; verified blocks are not discarded. The final artifact is
   published only after its complete SHA-256 digest passes.

The technical terminal in Debugger 0.13.0 timestamps every Agent, GUI, and
command entry to milliseconds. It provides level/source filters, text search,
follow/pause scrolling, copy, export to `.log`, and bounded clearing. ANSI
terminal control sequences are removed before display and export.

## Task activity and history

Version 0.10 provides a persistent-on-screen **Swarm tasks** panel. It lists work
initiated on this Debugger, work executed locally, and work received from
another Agent. Each row shows the lifecycle state, task description, executor,
direction, live elapsed time or final duration, and the typed result/error.
Matrix results therefore remain visible with their checksum after transient
status messages change.

The newest 100 rows are displayed and up to 1,000 completed rows are retained
in `%LOCALAPPDATA%\Swagri\debugger-tasks.sqlite3`. The database is local to the
Debugger and adds no swarm traffic. If Debugger closes while a task is running,
that row is marked as interrupted on the next start. **Clear completed** removes
finished rows from both the panel and SQLite. Swagri still does not claim a
percentage for workloads that expose no progress units; filtering,
cancellation and general-workload progress are later scheduler steps. The
distributed matrix is the first exception: its known chunk count provides
truthful progress without inventing a percentage.

## Updating Agent and Debugger through the swarm

Version 0.3 introduced signed, chunked peer-to-peer Agent updates. Install 0.3
manually once on every device; earlier agents do not yet understand the update
protocol. Version 0.4 can then be received from a connected newer Agent, making
0.3 → 0.4 the first practical upgrade test for the mechanism.

In Debugger, select a newer peer and click **Trust and update through P2P**. The
local Agent persists that exact Peer ID, requests its signed manifest, downloads
the executable in 256 KiB chunks, and verifies the platform, version, size,
Ed25519 signature, and SHA-256 before replacement. `swagri-updater` keeps the
previous executable and restores it when activation or the health check fails.

After the first explicit trust decision, **Automatically update from already
trusted agents** may be enabled. A newly discovered peer never becomes trusted
just because it is on the same LAN. Headless nodes provide equivalent commands:

```text
trust <peer-id>
update <peer-id>
untrust <peer-id>
```

For unattended headless operation, start the Agent with
`--update-policy automatic`; only peers already present in the trust file are
eligible. `--update-policy disabled` turns receiving updates off.

Before version 0.5, P2P updates replaced only the lightweight Agent and the
Debugger required `Swagri-Debugger-Setup-x64.exe`.

Version 0.5 adds full Debugger P2P updates. On both test computers, first use
the **Update Agent through P2P** button. After the local Agent is current,
select the same trusted newer peer and click **Update Debugger through P2P**.
The Agent downloads the peer's sibling Debugger binary in chunks using a
Debugger-specific signature domain. The GUI then stops its local Agent,
launches the updater, closes, retains `swagri-debugger.previous.exe`, and starts
the new GUI. A failed activation restores the previous executable.

The source must be a Debugger installation or Debugger portable directory that
contains `swagri-debugger.exe` and its `swagri-debugger.version` marker. A
headless Agent package intentionally cannot distribute a GUI it does not have.

### One-time 0.4 → 0.5 transition

Debugger 0.4 does not contain the new GUI-update command, so install Debugger
0.5 manually on at least the source computer and then once on the other test
Debugger, or use the installer fallback on both. Beginning with 0.5, later
Debugger versions can update each other entirely through P2P. Agent 0.4 can
still update itself to 0.5 through the existing P2P mechanism.

Once Debugger 0.5 or newer is installed, version 0.11.1 can be distributed as a
complete Debugger-to-Debugger P2P upgrade test.

## Build packages locally

Install Rust stable, Microsoft C++ Build Tools, and NSIS, then run:

```powershell
cargo test --workspace --all-targets
cargo build --release -p swagri-agent -p swagri-debugger -p swagri-updater
.\scripts\package-windows.ps1
```

The four packages are written to `dist\`.

## Can Swagri work without installers?

Yes. Both programs are self-contained portable executables, so installers are
already optional. In later stages agents can also be distributed through OS
package managers, containers, machine images, device-management systems, or be
embedded into another application.

An installer remains useful when Swagri needs to register a background service,
configure automatic startup, request firewall rules, manage updates, and offer
clean uninstallation. The intended model is therefore **installer optional**:
portable execution for experiments and managed installation for persistent
nodes.
