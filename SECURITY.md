# Security Policy

## Supported Versions

Security fixes ship in a new release on the latest minor line; there are no
long-term-support branches. Upgrade to the
[latest release](https://github.com/avihut/daft/releases/latest) to receive
them.

## Reporting a Vulnerability

If you discover a security vulnerability in daft, please report it privately —
not in a public issue — through either channel:

- **GitHub private vulnerability reporting** (preferred):
  [Report a vulnerability](https://github.com/avihut/daft/security/advisories/new).
  It keeps the report, the discussion, and the eventual advisory in one place.
- **Email**: **security@avihu.dev**

Please include:

- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Any suggested fixes (optional)

## Response Timeline

- **Acknowledgment**: Within 48 hours
- **Initial assessment**: Within 1 week
- **Fix timeline**: Depends on severity, typically within 30 days

## Scope

This policy covers vulnerabilities in:

- The daft binary and its commands
- The hooks system trust model
- Installation scripts

## Out of Scope

- Vulnerabilities in dependencies (report to upstream maintainers)
- Social engineering attacks
- Issues requiring physical access to your machine

## Disclosure

We follow coordinated disclosure. Once a fix is released, we'll credit reporters
(unless anonymity is preferred) in the release notes.
