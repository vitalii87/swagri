# Security policy

Swagri is pre-release experimental software and has not received a security
audit. Do not expose the current prototype to the public internet or use it to
process sensitive data.

## Current security boundaries

- Network transport is encrypted, but encryption does not establish trust.
- mDNS discovery can reveal untrusted devices on the same network.
- The MVP accepts only compiled-in, typed tasks with input limits.
- Shell commands, arbitrary binaries, and downloaded native code are not valid
  task types.
- Identity files are sensitive because they represent a stable node identity.
- P2P Agent updates require an explicitly trusted Peer ID. The sender signs the
  manifest with its persistent Ed25519 identity; the receiver also verifies the
  target, version, size, and SHA-256 before a separate updater swaps binaries.
- Update trust is device-owner trust, not official publisher signing. A trusted
  compromised device can therefore distribute a malicious Agent. Use the
  feature only between devices you control until release-key signing is added.
- Task execution does not yet implement a peer allowlist; run it only on a
  trusted or isolated LAN.

## Reporting a vulnerability

Do not open a public issue for a vulnerability that could put users or devices
at risk. Contact the repository owner privately through GitHub and include:

- affected commit or version;
- reproduction steps;
- expected impact;
- suggested mitigation, if known.

Public disclosure should wait until a fix or safe mitigation is available.
