# Upstream dependency tracking

A pin is a **snapshot**, not a freeze. Upgrade when an upstream moves and it
buys us something. This file is the map: crates we build on, and the data
sources we refresh from.

_Last checked: 2026-10-22._ Cross-repo rollout tracker: [cf-connectrpc-middleware#4](https://github.com/joeblew999/cf-connectrpc-middleware/issues/4).

## Crates

| Dep | We pin | Latest published | Source | Notes |
|---|---|---|---|---|
| `connectrpc` | **0.9.1** (`json`) | 0.9.1 (2026-09-21) | [anthropics/connect-rust](https://github.com/anthropics/connect-rust) | `default-features = false` + `features = ["json"]`. Without `json` (opt-in since 0.8) every JSON request is **HTTP 415**. |
| `connectrpc-build` | **0.9** | 0.9.0 (2026-08-25) | anthropics/connect-rust | build.rs codegen; moves with `connectrpc`. |
| `buffa` / `buffa-types` | **0.9** (lock 0.9.2) | 0.9.2 (2026-09-03) | [anthropics/buffa](https://github.com/anthropics/buffa) | Lockstep with `connectrpc`. |
| `worker` / `worker-macros` | **0.8** (lock 0.8.7) | 0.8.7 (2026-09-25) | [cloudflare/workers-rs](https://github.com/cloudflare/workers-rs) | Needs `rust-version` ≥ 1.91. `wrangler.toml` installs `worker-build@^0.8` (= 0.8.7), which always passes `--force-enable-abort-handler`, so the lock needs **wasm-bindgen ≥ 0.2.125** (on 0.2.129). An older lock fails `wrangler deploy`. |
| `tower` | `0.5` | 0.5.3 | [tower-rs/tower](https://github.com/tower-rs/tower) | |

## Data sources (runtime upstreams)

| Source | URL | Used for |
|---|---|---|
| where.durableobjects.live ([connyay](https://github.com/connyay)) | `https://where.durableobjects.live/api/v3/data.json` | Edge → DO-region latency measurements; refreshed weekly (Mon 03:00 UTC) into KV. |
| Cloudflare status | `https://www.cloudflarestatus.com/api/v2/components.json` | Colo list / names. |

If either changes shape, `src/refresh.rs` is the only place that parses them (native-testable).

## Relationships

- **Nothing links against us.** Consumers vendor `proto/locator/v1/` and call over HTTP (`examples/rust-client` uses `ureq`; `examples/ts-client` uses fetch), so our crate versions never enter anyone else's `Cargo.lock`.
- We track the same connectrpc line as [cf-connectrpc-middleware](https://github.com/joeblew999/cf-connectrpc-middleware) only to share one set of gotchas. There's no ordering constraint, so a bump here can land any time.
- Alternative for consumers who'd rather not depend on a service: connyay's static crate [cf-colo-hint](https://github.com/connyay/cf-colo-hint).

## Upgrade checklist

1. Bump the pins, then `cargo update` (wasm-bindgen must stay ≥ 0.2.125).
2. `mise run cargo:check`, clippy `-D warnings` (wasm32), `mise run cargo:test`.
3. `worker-build --release` (the real deploy path).
4. Deploy, then sanity-check `ListColos` / `GetLocationHint` with a JSON request (catches a dropped `json` feature).
