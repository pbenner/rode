# RODE Stocks

An independent Rust executable supplying Stocks through the common RODE panel
API. Market requests, symbol parsing, watchlists and the bounded 12-series cache
live here. RODE renders the provider's chart data with its theme, keyboard and
window services. The GUI has no dependency on Stocks domain or market-networking code.

Build only this extension from a RODE source distribution:

```sh
cargo build --locked --release -p rode-stocks
```

For a source installation, update the executable independently:

```sh
cargo install --locked --path crates/rode-stocks --force
```

Use `--root PREFIX` when updating a source installation under a custom prefix.
Disable/re-enable Stocks after replacing its binary to restart only that provider.
For packaged installations, update the executable in the package installation
prefix, or install a separate explicit manifest pointing to a user-owned build.

RODE discovers its included `rode-stocks` executable beside the editor, then on
PATH. Its existing `rode.stocks` preference ID and saved watchlist are retained.
Debian and `cargo xtask install` distributions include this executable. It can
also be used by an installed manifest with `panel: true`, `panel_version: 2`,
`icon: "chart"`, program `/absolute/path/rode-stocks` and args `["--stdio"]`.

Without `--stdio`, the executable prints help and performs no requests. With
`--stdio --demo` it uses clearly labeled synthetic data and no network, useful
for demos and native UI checks. Production uses the same bounded, read-only,
unofficial public Yahoo Finance endpoints as before; availability and delays
remain subject to that provider. There are no API keys or project-host requests.
