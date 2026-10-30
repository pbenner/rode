# Codex runtime notices

Unchanged upstream notice texts for the native Codex version pinned in `../codex.json`. Each manifest entry records its source and SHA-256. The Codex archive and extracted executable have separate size/hash pins for Linux x86_64 and aarch64.

The snapshots include the upstream Apache 2.0 LICENSE, NOTICE, license documentation and checked-in third-party notices. Ratatui's MIT LICENSE comes from the checksum-verified `ratatui 0.30.2` Cargo archive referenced by Codex's lockfile, supplementing the attribution in Codex's NOTICE. Platform-specific optional components mentioned by upstream notices are not added as separate executables to RODE's Linux package.

The builder ships these complete texts with Codex and assembles `CODEX-NOTICES.gz`. They retain their upstream terms; RODE's MIT license does not replace them. Review archive provenance, binary hashes, notices and protocol compatibility when updating the pin. No VS Code extension files or user credentials are copied into this bundle.
