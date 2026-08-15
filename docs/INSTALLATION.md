# Installation and test builds

Swagri currently provides two Windows x64 applications.

- **Swagri Agent** is the lightweight, headless peer for computers that donate
  resources. It contains no GUI.
- **Swagri Debugger** is the desktop test console. Its package also contains an
  agent, so it can start and control a local peer, discover nearby agents,
  test connections and tasks with buttons, compare versions, display host CPU
  and memory, and provide an optional technical console.

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

## Updating test agents through the swarm

Version 0.3 introduces signed, chunked peer-to-peer Agent updates. Install 0.3
manually once on every device; earlier agents do not yet understand the update
protocol. Later Agent versions can be received from a connected newer Agent.

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

P2P updates replace the lightweight Agent only. Debugger itself is updated with
`Swagri-Debugger-Setup-x64.exe` because it owns and supervises the bundled Agent.

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
