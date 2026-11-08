# Release checklist

Use this checklist for the exact commit intended for release. Local checks, optional-tool fixtures and package smoke tests provide different evidence; record which tools and host capabilities were actually exercised. A skipped or unavailable feature is not a passing test of its implementation.

## Source and documentation

- Check the diff for unrelated local files, debug code, obsolete launch paths and duplicated sources. Remove obsolete compatibility paths, their callers and fixtures together. Require the current session protocol and validate current persistence, attachment and shutdown behavior.
- Run `mise run check` with the [documented system dependencies](development.md). This covers formatting, Python/CI/packaging regressions, Rust API documentation, Clippy and ordinary workspace Rust tests.
- Run `scripts/uvx.sh --from mkdocs==1.6.1 mkdocs build --strict`. Review the published guides and repository-file links as well as warning-free generation.
- Follow changed operations through the [source map and ownership contracts](architecture.md). Document host identity, coordinate units, cancellation, version checks and resource lifetime where those contracts live. Keep public API documentation and the user-facing guide consistent with the final behavior.
- Review dependency and bundled-source notices. Run a fresh advisory audit as described in [Security review](security.md#dependency-audit); the dated audit in that guide is historical evidence, not an audit of every later lockfile.

Dependency scanners can flag build dependencies or public library entry points that ordinary application code does not mention. Verify call sites, Cargo build scripts, feature-gated targets and supported direct library clients before removing a candidate. The `cc` dependency in `rode-bwrap`, for example, compiles the vendored C sources in `build.rs`.

## Runtime coverage

Run **Actions → Check → Run workflow** with **full** enabled for the release commit. Review all fast and extended jobs, the documentation workflow and package validation. The `linux` aggregate covers the fast tier; its success alone does not establish extended coverage. A `full-ci` pull-request label selects all extended checks, but does not run the release package job.

The suite definitions and fixture variables are in [Development](development.md#ci-tiers-and-timings). Check the recorded logs for:

- Local projects and individual files; protected save/conflict behavior; large files; keyboard focus, themes, docking and multiple windows.
- Direct and ProxyJump SSH, failed connections, helper discovery/deployment and a host fixture without Python. Verify static-helper behavior on the oldest remote environment being claimed as supported.
- Agent controls, approvals, file-only/offline tool policies, history and persistent reattachment. Protocol fixtures do not establish compatibility with every external agent version or a live model service.
- Real installed Jupyter, debugger, formatter, PDF and other specialist tools using the documented fixture variables. Tool absence can skip ordinary tests; inspect the runtime job rather than relying on a test count alone.
- Actual bubblewrap and Landlock enforcement. Require enforcement through the documented test variables on capable hosts. Kernel ABI mask tests cover policy construction; they do not replace execution on each kernel family being supported.
- Persistent terminal/agent/runtime ownership across disconnect, reattachment and helper upgrades. Use the target site's scheduler and FUSE setup for claims about real Slurm allocations, GPUs or encrypted mounts; scheduler fixtures cannot establish that site's behavior.

Linux x86_64 is the locally validated platform. Treat arm64, particular desktop distributions and additional server/kernel combinations as separate validation targets. List remaining limitations in the release notes instead of inferring coverage from architecture selection code.

## Package and version

1. Set the intended version in `Cargo.toml`, review workspace package versions and keep `Cargo.lock` consistent. Package versions, helper API revisions and host/session protocol versions are separate contracts; change compatibility revisions only when their documented contracts require it.
2. Supply a real **DEB_MAINTAINER** identity before redistribution. Full CI uses an explicitly marked test identity; its package artifact is validation output.
3. Build on the oldest desktop distribution being supported. Run the [Debian package validation](packaging.md#validation), including extracted GUI startup and a real helper-owned shell. `--skip-build` requires the current standalone static helper or an explicit `--helper` path; it must not pick a stale executable.
4. If distributing the optional agent runtime, validate it separately and check that the core editor also works without it. Include complete notices and verify the final artifact checksums.
5. Tag only the validated commit. The package workflow requires a tag equal to `v` plus the root package version. Tagging triggers package construction; publishing a GitHub Release and installing packages remain explicit actions.

Record the commit, toolchain, build distribution, test results, checksums and unverified environments with the release notes. Preserve logs and screenshots needed to investigate a later regression.
