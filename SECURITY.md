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
- The prototype does not yet implement an allowlist; run it only on a trusted or
  isolated LAN.

## Reporting a vulnerability

Do not open a public issue for a vulnerability that could put users or devices
at risk. Contact the repository owner privately through GitHub and include:

- affected commit or version;
- reproduction steps;
- expected impact;
- suggested mitigation, if known.

Public disclosure should wait until a fix or safe mitigation is available.

