# RODE RSA dependency security patch

Based on the published `russh` 0.65.1 source, upstream revision
`b33092aad1e51c66258a12e8b83fc473a68d9d34` from
<https://github.com/warp-tech/russh>. The upstream license and copyright notices
are retained. `Cargo.toml.orig` is the unmodified published upstream manifest.
`LICENSE` contains the Apache-2.0 text from the repository's checksum-pinned
upstream license snapshot in `packaging/licenses/russh-LICENSE.txt`.

The normalized Cargo manifest moves RSA from the exact `0.10.0-rc.18` pin to
`0.10.0`, explicitly enables its `pkcs1v15` signature support, and moves PKCS#1
from the exact `0.8.0-rc.4` pin to `0.8.0`. Key-format code uses the stable
`rsa::PrivateKey` API. PKCS#8 RSA decoding delegates to the existing PKCS#1 DER
decoder instead of duplicating component conversion. The decompressed-packet
limit is compiled only for compression-enabled builds or tests, avoiding a dead
code warning in builds without compression.

Serialized private-key data is omitted from the key-format fixture module,
the inline key module's fixture tests and the no-RSA-feature signing fixture.
Their fixture-dependent tests are omitted from this vendored copy. The key
module documentation describes runtime key loading instead of embedding a key.
Other library implementation and tests are unchanged. RODE's generated-key SSH,
tunnel and key-format regressions cover the application integration without
storing serialized private keys in the repository.
Three upstream README lines have trailing whitespace removed.
The registry cache marker and package-local lockfile are omitted; the repository
lockfile selects the complete dependency graph.

The old RSA version is affected by
[RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html).
The registry release and current upstream main still require the prerelease as
of 2026-11-10, preventing a lockfile-only upgrade. The workspace patch keeps RSA
SSH identities and host keys supported while selecting the patched dependency.
Replace this local patch with an upstream release once it permits the fixed RSA
version. Do not suppress the advisory or downgrade authentication support.

Validate with the native SSH client, tunnel and SSH integration tests, the
standalone helper build, and a fresh `cargo audit`.
