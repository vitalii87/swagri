# Installation and test builds

Swagri currently provides two Windows x64 applications.

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

Version 0.9 provides **Smart CPU test** and **Smart Matrix task** in Debugger.
The equivalent console commands are:

```text
auto-benchmark 1000000
auto-matrix 320
```

The Agent compares its current effective CPU score with resource observations
received from connected peers. It executes locally unless the strongest fresh,
compatible peer is at least 20% better, which provides a simple allowance for
network and coordination overhead. Debugger shows the chosen device, both
scores, the required threshold, completion time, and the matrix checksum.
Ordinary tasks are not yet automatically queued, divided, migrated, retried,
or cancelled.

### Force a smart task onto the other computer

This is the recommended two-Debugger test for version 0.9:

1. Start version 0.9 on both computers and wait until each lists the other as
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

## Task activity and history

Version 0.9 provides a persistent-on-screen **Swarm tasks** panel. It lists work
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
cancellation, and streamed percentage progress are later scheduler steps.

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

Once Debugger 0.5 or newer is installed, version 0.9 can be distributed as a
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
