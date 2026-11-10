# RODE RSA dependency security patch

Based on the published `ssh-key` 0.7.0-rc.11 source from
<https://github.com/RustCrypto/SSH>. The original Apache-2.0/MIT licenses and
copyright notices are retained. `Cargo.toml.orig` is the unmodified published
upstream manifest.

The normalized manifest requires RSA `0.10.0` with explicit `pkcs1v15` support,
replacing its removed optional `sha2` feature. RSA key conversions use the stable
`rsa::PrivateKey` and `rsa::PublicKey` names, and private-key encoding extracts
the integers from validated odd primes.

The upstream integration-test/fixture directory and its ten normalized Cargo
test targets are omitted. Those fixtures include serialized private keys that
trigger repository push protection even though they are public test data. The
one library signature test that includes an omitted key fixture is omitted too.
Private-key documentation examples accept caller-provided key input rather than
embedding serialized keys. Other library implementation and unit tests are
unchanged. RODE's `tests/ssh_client.rs` generates fresh keys for OpenSSH and
PKCS#8 round trips, encryption/password rejection and signature checks.
The registry cache marker and package lockfile are omitted; the repository
lockfile selects the dependency graph.

This accompanies the russh manifest patch in `../russh/PATCH.md`, allowing the
workspace to resolve the fixed RSA version for RUSTSEC-2023-0071 without losing
RSA SSH identities or host keys. Remove these local patches together when
compatible upstream releases support the fixed dependency.
