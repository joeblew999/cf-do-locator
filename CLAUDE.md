# CLAUDE

README has the intent. This file is for AI agents and the next human picking up the repo.

## What this is

A **Cloudflare Worker** that exposes a ConnectRPC `LocatorService` over a KV-stored snapshot, refreshed weekly from upstream measurement data. This is the service, not a library — consumers vendor `proto/locator/v1/` and codegen their own clients.

Not a library crate, despite `cdylib + rlib`. The `rlib` is for native unit tests (parse + hysteresis); production is the wasm32 cdylib bundled by `worker-build`.

## Version pins

No version coupling with other repos: consumers vendor the proto and call over HTTP, so these pins never enter anyone else's `Cargo.lock`. We still track the same connectrpc line as [cf-connectrpc-middleware](https://github.com/joeblew999/cf-connectrpc-middleware) to keep one set of gotchas.

| Dep                | Version  |
| ------------------ | -------- |
| `worker`           | `0.8` (features: `http`) |
| `worker-macros`    | `0.8`    |
| `connectrpc`       | `0.9`    |
| `connectrpc-build` | `0.9`    |
| `buffa`            | `0.9`    |
| `buffa-types`      | `0.9`    |
| `tower`            | `0.5`    |
| `http`             | `1`      |
| `http-body`        | `1`      |
| `http-body-util`   | `0.1`    |
| `serde` / `serde_json` | `1`  |
| edition            | `2024`   |
| rust-version       | `1.91` (worker 0.8.7 needs it) |

Build target: **`wasm32-unknown-unknown`** (not `wasm32-wasip1`).

`connectrpc` needs `features = ["json"]` (it's opt-in since 0.8 and we set `default-features = false`); without it every JSON request is HTTP 415. `wrangler.toml` installs `worker-build@^0.8`, and worker-build 0.8.7 needs wasm-bindgen ≥ 0.2.125 in `Cargo.lock`, so run `cargo update` when you bump.

## Critical workers-rs gotchas

1. **`.into_send()` before `.await`** — every KV / Fetch / D1 call. `connectrpc`'s service trait requires `+ Send` on the returned future; worker futures hold `JsValue` which is `!Send`. Pattern:

   ```rust
   use worker::send::IntoSendFuture;

   let v = kv.get(key).json::<T>().into_send().await?;
   let body = fetch(...).send().into_send().await?.bytes().into_send().await?;
   kv.put(k, body)?.execute().into_send().await?;
   ```

   Forgetting this is a 100-line compiler error that doesn't say "into_send" anywhere. See [[feedback-workers-rs-into-send]] in memory.

2. **`worker::console_log!` / `console_error!` panic natively.** They call into `wasm_bindgen` which traps on `cargo test` non-wasm targets. Either gate logs with `#[cfg(target_arch = "wasm32")]`, or — better — return structured stats from pure functions and log only at the wasm-only call site. `refresh::apply_hysteresis` follows the latter pattern.

3. **`#[event(scheduled)]` doesn't auto-handle `Result<()>`.** The macro warns "unused Result". Make the handler return `()` and log errors inside:

   ```rust
   #[event(scheduled)]
   async fn scheduled(_e: ScheduledEvent, env: Env, _c: ScheduleContext) {
       if let Err(e) = refresh::run(&env).await {
           worker::console_error!("refresh failed: {e}");
       }
   }
   ```

4. **Don't re-read KV blobs per request.** Reading + parsing the 344-colo snapshot on every RPC was ~6 ms of a ~9 ms request (switching `.json()` to `.text()` + serde_json made no measurable difference). `load_snapshot` caches it per isolate; do the same for any other hot KV read.

5. **Read CF production CPU profiles by sample count, not by time.** The profiling API's sample values are wall-clock gaps between samples, and an idle isolate takes no samples, so idle time lands on whatever frame runs first after an `await` (e.g. `reader.releaseLock()`). Ground truth for per-request CPU is GraphQL `workersInvocationsAdaptive` `cpuTime` quantiles.

## Architecture

```
upstream (CF + connyay) ──► scheduled handler ──► KV snapshot ──► RPC handlers ──► consumers
                            (weekly, in-Worker)   ("snapshot" key)  (4 RPCs)        (cache once per isolate)
```

KV is canonical. Anyone can blow away the local checkout — the live state is in KV. The repo holds the **logic**, not the data.

## Module layout

```
src/lib.rs          fetch + scheduled event handlers; wires LocatorServer into ConnectRpcService
src/service.rs      LocatorService impl — every RPC: load_snapshot() (isolate-cached, 60 s TTL) then map to pb
src/refresh.rs      download upstream, merge, hysteresis, KV put. Native-testable.
src/snapshot.rs     KV blob shape (Snapshot, ColoEntry) + proto conversions
src/state.rs        AppState = { kv, snapshot_key }
proto/locator/v1/locator.proto  service contract
examples/           consumer-side patterns (README only — no compiled examples by design)
```

## When changing the proto

1. Edit `proto/locator/v1/locator.proto`.
2. `cargo check --target wasm32-unknown-unknown` regenerates the Rust bindings (via `build.rs` → `connectrpc-build`).
3. Update `src/service.rs` to satisfy the trait if the signature changed.
4. Bump `Cargo.toml` version on any breaking shape change so consumers know.
5. The schema is the contract — never let `service.rs` and `locator.proto` drift.

## When changing the refresh pipeline

1. Edit `src/refresh.rs`.
2. Cover the change with a `#[cfg(test)]` unit test that runs on native (no JsValue calls in the tested path). See `parses_iata_suffix`, `hysteresis_keeps_marginal_flips`.
3. `cargo test --lib` must pass before push.
4. Hysteresis constants live at the top of the file; don't smear them across the codebase.

## When wiring a new consumer

The funnel pattern is **mandatory**, not optional:

- One helper per consumer Worker — `createUserDO()`, `createTenantDO()`, etc.
- The colo source varies per DO type (see [[feedback-do-location-hint-permanent]] in memory).
- Cache `ListColos` at isolate scope; never RPC `GetLocationHint` per request.
- Log `{colo, hint, do_type, do_id, ts}` at every creation site.

`examples/{rust,ts}-client/README.md` shows the shape.

## Don't do these

- Don't add Python tooling. The repo is Rust + nushell + bash (Stack rule).
- Don't add a frontend. This is an API service; consumers handle UI.
- Don't store secrets in `wrangler.toml`. Everything goes through `fnox` keychain with `DO_LOCATOR_*` prefixes.
- Don't `cargo test` then push without `cargo check --target wasm32-unknown-unknown` — the wasm target catches errors native misses (and vice versa).
- Don't run `wrangler kv:namespace ...` (deprecated v3 syntax). Use `wrangler kv namespace ...` (v4).
