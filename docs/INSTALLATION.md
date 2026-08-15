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

## Updating test agents

Version 0.2 agents exchange their versions after connecting. Debugger highlights
when the selected peer is newer. Open **Updates** and use **Update from
installer...** with an official `Swagri-Debugger-Setup-x64.exe`; Debugger stops
its bundled agent, launches the installer, and closes so both executables can be
replaced.

Automatic peer-to-peer executable transfer remains disabled until Swagri has a
signed update feed and trusted-peer enrollment. A peer may announce that a
newer version exists, but an unauthenticated peer is never allowed to silently
replace an executable. This boundary is intentional for the current LAN alpha.

## Build packages locally

Install Rust stable, Microsoft C++ Build Tools, and NSIS, then run:

```powershell
cargo test --workspace --all-targets
cargo build --release -p swagri-agent -p swagri-debugger
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
