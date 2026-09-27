# zedproxy

A tiny local reverse proxy that fixes a [Zed](https://zed.dev) bug: context
compaction requests sent to OpenAI-compatible providers carry
`"reasoning_effort": "none"`, which most non-OpenAI providers reject, so
compaction always fails. zedproxy rewrites that one field before forwarding.

Upstream bug: [zed-industries/zed#60411](https://github.com/zed-industries/zed/issues/60411) —
"OpenAI-compatible provider sends `reasoning_effort: "none"` on non-thinking requests
(compaction/summaries), breaking backends that don't accept `"none"`".

```
Zed ──http://localhost:4000/v1──▶ zedproxy ──https://api.isoquant.ai/v1──▶ provider
```

Everything passes through untouched — request and response bodies, headers,
query strings — with exactly one exception: on `/v1/chat/completions`, if the
request body has top-level `"reasoning_effort": "none"`, it is replaced with
the configured value. Malformed JSON and all other paths are forwarded as-is.

## Usage

```
zedproxy
```

That's the whole interface. Configuration is environment variables:

| Variable | Default | Meaning |
|---|---|---|
| `ZEDPROXY_UPSTREAM_URL` | `https://api.isoquant.ai/v1` | Upstream base URL. Path part is only used to build the match pattern. |
| `ZEDPROXY_REASONING_EFFORT` | `high` | Value substituted for `"none"`. |

The proxy listens on `127.0.0.1:4000`.

### Zed side

Point the provider's `api_url` at the proxy:

```json
"Isoquant": {
  "api_url": "http://localhost:4000/v1",
  ...
}
```

## Development

Uses [devenv](https://devenv.sh)

```
devenv shell -- cargo test
devenv shell -- cargo clippy --all-targets
devenv shell -- cargo build
```

- `cargo run` — run the proxy from source.
- `examples/mock_upstream.rs` — a mock OpenAI-compatible upstream for testing
  the rewrite without hitting a real provider.

## Packaging

`pkgs/zedproxy.nix` builds the binary as a Nix derivation via
`pkgs.rustPlatform.buildRustPackage` with `cargoLock` integrity checking
against `Cargo.lock`.

Once published, it can be built directly from this repository — e.g. from a
NixOS configuration:

```nix
zedproxy = pkgs.callPackage (pkgs.fetchFromGitHub {
  owner = "YOURUSER";
  repo = "zedproxy";
  rev = "v0.1.0";
  hash = ""; # fill from build error
}) {};
```
