# RODE extension API

Shared, data-only Rust contracts for independently built RODE extensions.
The crate depends only on serde, serde_json and anyhow. It contains no editor,
egui, SSH, host service or domain implementation.

Protocol 1 remains compatible with existing providers. Protocol 2 adds native
interactive OHLC/volume charts, selectable action groups, inline inputs with
Enter submission, links, and host-owned, field-merged settings. Declare
`protocol: 2` in an explicit desktop panel contribution (or `panel_version: 2`
in a legacy API-1 panel manifest) to use it. The included Stocks
executable and separately installed providers use the same host and contracts.

`panel::Request` and `panel::Reply` carry one request at a time per provider.
`server::serve` implements bounded serial JSON-line transport and validates
requests before domain dispatch, then reply identities and negotiated view
capabilities before publishing them. Version 1 retains its original request
shape; version 2 always includes its settings object, including when empty.
Lifecycle action IDs (`open`, `tick`, `settings`) are reserved and cannot submit
form values. Domain failures should return a View with
`error: true`; malformed framing terminates the process. Nothing connects to a
service just because the executable is installed or its manifest discovered.

Host and provider versions are separate from application release versions. A
protocol-1 provider continues working with a host supporting protocol 2. New
protocol-2 capabilities require a compatible host once; compatible provider
updates do not rebuild the editor. Before publishing, SDK users can vendor this
small crate as a versioned snapshot without depending on the RODE source tree.

`manifest::Manifest` validates both legacy API-1 manifests and explicit
`contributions`. A contribution declares its `kind` and `host`: native `panel`
providers run on `desktop`; one-shot `project-tool` adapters run on `project`.
One extension can provide both with separate literal executables and process
lifetimes. Mixing legacy and explicit forms or changing a kind's host is rejected.
The editor owns execution, policy, cancellation and buffer-version guards.

`tool::Request`, `tool::Reply`, `tool::Diagnostic` and `tool::Invocation` are the
shared one-shot project-tool contracts. Hosts re-export these types rather than
maintaining another wire schema; providers need only this small SDK. Tools return
buffer edits and diagnostics and must not write behind the editor's save guards.
