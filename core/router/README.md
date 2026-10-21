# aws-emulator-router

The emulator binary. It owns no service logic: it constructs the single shared
`InMemoryEngine` per tenant and mounts every `service-*` crate on one endpoint,
keyed by each service's SigV4 signing name (`ROUTE_KEY`).

```bash
cargo run -p aws-emulator-router
```

Binds the emulator API and its `GET /__admin/health` readiness endpoint on
`:9297` (self-signed HTTPS); override with `EMULATOR_DATA_PORT` or `-p`.
Options (after `--` with `cargo run`): `-h` help, `-v` version, `-b <ip>` / `-p <port>`
(override the variables above), `-a on|off` SigV4 request authentication with this run's
keys (default on), `-t on|off` usage telemetry for this run, `--idle-timeout 900` exit after 900 seconds
without an API call.

## How a service gets mounted

`src/lib.rs` is **re-rendered on every service add** — edits there are overwritten. `src/main.rs` is emitted once and must not be edited — only `tests/` is editable here. For each service it does one thing:

- `router.register_service(<service>::EmulatedService::new(engine.clone()))` —
  construct the service from the single shared engine and mount it. Each operation
  receives the engine through its constructor and reaches the shared store through
  `self.engine`.

`MultiServiceRouter` then picks the service whose `can_handle` claims each request
(seeded from the model's SigV4 signing name), and the service's protocol names the
operation.

## Tests

`router/tests/` is for CROSS-service behaviour (e.g. one service invoking
another). Single-service tests belong in that service's own crate, where they
run without rebuilding the rest of the workspace.

Put a router suite at `router/tests/all/<name>.rs` and declare it in
`router/tests/all.rs`. That is the router's only test target: every target links
all services, so a file directly under `router/tests/` fails the build.
