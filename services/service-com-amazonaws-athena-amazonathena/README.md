# service-com-amazonaws-athena-amazonathena

Emulator implementation for the `com-amazonaws-athena-amazonathena` service. This crate is
self-contained: building it does NOT rebuild the other services.

```bash
cargo build -p service-com-amazonaws-athena-amazonathena
cargo test  -p service-com-amazonaws-athena-amazonathena
```

## The SQL engine, and its memory-safety posture

Athena is an HTTP control plane in front of a query engine: `StartQueryExecution` takes
SQL and `GetQueryResults` hands back the rows that SQL produced. So this crate runs
submitted queries on a REAL engine in-process — `rusqlite` (SQLite), declared in
`Cargo.toml` with the `bundled` feature — and the rationale is recorded here so it is
visible with the code rather than only in a review:

* **Why an engine crate at all.** The alternative is a canned or hand-written result set,
  which this project forbids for execution planes (never match on source text, never
  synthesise output that looks computed). The contract for an execution plane is a real
  interpreter or engine, bounded by a wall-clock timeout, a step budget and an output cap,
  run in-process — which is exactly how `src/state/query_engine.rs` drives it.
* **What `bundled` means.** `libsqlite3-sys` compiles the SQLite C amalgamation at build
  time and links it in, so the process gains a native (unsafe-FFI, C) component. The
  requirement "no crate may use `unsafe`" is about the crates in THIS workspace: this
  crate contains no `unsafe` block, function or impl and keeps the workspace's
  `unsafe_code = "forbid"` lint. The native component is a third-party dependency, and it
  is the same engine this workspace already links through `vendor/emulator-telemetry`,
  which depends on `rusqlite` with the same `bundled` feature — so this introduces no new
  native library to the emulator process, only a second user of one already linked.
* **What bounds it.** Caller-supplied SQL reaches the engine only through
  `Database::open`, which installs, BEFORE any request is served: a default-deny
  authorizer (every host-reaching action — `ATTACH`/`DETACH`, `VACUUM INTO`, `PRAGMA`,
  extension loading, a virtual-table module, and any action a future engine version adds
  — is refused), an in-memory connection opened without `SQLITE_OPEN_URI`,
  `temp_store = MEMORY`, `max_page_count` on `main` and `temp`, `hard_heap_limit`,
  defensive mode, untrusted schema, `SQLITE_LIMIT_ATTACHED = 0`, and length, SQL-length,
  column, expression-depth and compound-select limits. One run is additionally bounded by
  a wall-clock deadline and a step budget enforced from the engine's progress handler, and
  the retained result set is capped in rows, cells and bytes. `load_extension` is not an
  enabled feature of the dependency, so it is not reachable from this crate at all.
  `tests/all/query_execution_lifecycle.rs` asserts that the SQL which would reach the host
  ends the query FAILED, carrying a reason with no host path in it.
* **Where it runs, and what it holds.** The engine is synchronous and CPU-bound, so a
  query runs on a blocking pool (`tokio::task::spawn_blocking`) and never on an async
  worker, and the service's state lock is released before the engine is asked for
  anything: one caller's slow query serialises nothing but its own database. The queries
  of ONE database run one at a time on that database's own connection, and the runtime's
  blocking pool is itself bounded, so the number of engine runs in flight is bounded
  without a second mechanism. There is one database per catalog and database name a query
  addresses, bounded by `state::MAX_QUERY_DATABASES`, and the number of retained
  executions by `state::MAX_QUERY_EXECUTIONS` — each holds a result set, so both are
  needed for the per-query caps to bound anything.
* **What it does not do yet.** Registering what a `CREATE TABLE` defines in the Glue Data
  Catalog is not part of this slice: Athena's metadata reads project Glue (see
  `src/state.rs`), and wiring DDL through to it belongs with the DDL operations' own
  issues. Until then a table defined through a query is visible to later queries in the
  same database and not to `ListTableMetadata`. A failed query's `StateChangeReason` is
  also the engine's OWN diagnostic (with anything path-shaped redacted) or this
  emulator's wording for a bound it stopped the run at, so it does not match the wording
  Athena's own engine would use for the same failure — a message-fidelity gap for its own
  issue, not a reason to invent a message no engine produced.
* **Alternatives considered.** A pure-Rust SQL engine would remove the native component,
  at the cost of a different SQL dialect and far less exercised code on the same
  untrusted-input path; the engine already linked by the workspace was preferred. If the
  workspace's curated `[workspace.dependencies]` is to own this dependency, the root
  manifest is the place for it — this crate declares it directly only because the root
  manifest is not editable from here.
* **This confinement harness exists three times, and should exist once.** The same
  construction — the open flags without `SQLITE_OPEN_URI`, the pragma batch, the
  `set_db_config` pair, the `SQLITE_LIMIT_*` set, the denied-function list, the
  default-deny authorizer, the progress-handler deadline and step budget, and the
  path-scrubbing of an engine message — is also in
  `services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs` and
  `services/service-com-amazonaws-rdsdata-rdsdataservice/src/state.rs`. Three copies of
  one security-relevant rule can drift, so the harness belongs in `services/common`
  (which this crate already depends on for its page tokens), consumed by all three. That
  hoist is NOT part of this change: it rewrites two other services' data planes, so it
  needs its own maintainability change where all three crates' suites run against the
  shared home.

## Layout

```
service-com-amazonaws-athena-amazonathena/
├── build.rs              # calls emulator_core_build_support::generate(...)
├── smithy-model/
│   └── com-amazonaws-athena-amazonathena.json      # the Smithy model — drives routing + wire shapes
├── OPERATIONS.md         # every operation's contract, re-rendered from the model
├── tests/
│   └── state_roundtrip.rs  # compiled example of the state API
└── src/
    ├── lib.rs            # #[smithy_service] decl (generated — do not hand-edit)
    ├── service.rs        # impl Service — can_handle + routing/refusal overrides (yours to edit)
    ├── state.rs          # ComAmazonawsAthenaAmazonathenaState — this service's in-memory state
    ├── scaffolded.rs     # Scaffolded<E> — the 501 stubs return, and `validation` for a 400
    └── ops/              # ONE FILE PER OPERATION
```

## Implementing an operation

Each file in `src/ops/` owns exactly one operation and declares which one
via an `@op` header comment. `build.rs` globs this directory and generates
both the module list and the operation set, so adding a file is all that
is needed — there is no central registry to update.

Every generated stub starts out **scaffolded, not implemented**: it is fully typed
and registered, and it answers **HTTP 501 `NotImplemented`** until you write it. It
deliberately does not return an empty success — a hollow `200` with a zeroed body is
indistinguishable from a real implementation to a client, and to any check that only
asserts the call succeeded.

Each operation is a struct that owns the shared engine (constructor-injected) and
implements `smithy_server_runtime::Operation`. The framework constructs one per
service via `OpName::new(engine)` and drives it through the centralized request walk:
deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.

To implement an operation, replace the `Err(..)` in `handle` with real behaviour and
delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line at the top of the file. That
marker is how the still-stubbed operations are counted:

```sh
grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' */src/ops/
```

```rust
//! @op CreateThing

impl<Eng: Send + Sync + 'static> smithy_server_runtime::Operation for CreateThing<Eng> {
    type Input = CreateThingInput;
    type Output = CreateThingOutput;
    type Error = CreateThingError;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Scaffolded: `Err(crate::scaffolded::Scaffolded::not_implemented("CreateThing"))`.
        // Modeled failures: `Err(CreateThingErrorModeled::Variant(..).into())`.
        /* ... */
    }
}
```

## State

A handler reaches this service's store through the engine held on `self`
(constructor-injected). The framework builds ONE shared engine and hands it to every
operation of this crate through its `new(engine)` constructor, so every handler in
this crate sees the same store:

```rust
async fn handle(
    &self,
    input: Self::Input,
    ctx: &smithy_server_runtime::OperationContext<'_>,
) -> Result<Self::Output, Self::Error> {
    let Some(state) = crate::state::service_state(&*self.engine) else {
        return Err(/* mounted on an engine that holds no state: a modelled server error */);
    };
    // A LOCKABLE handle, not the value — read and mutate through the guard:
    state.lock().your_field = /* ... */;
}
```

> **Never write `engine::InMemoryEngine::new()` inside a handler.** `engine` is the
> name of a CRATE as well as of the field, so that line compiles — and silently
> gives the request a private, empty store that is dropped when it returns. The write
> looks like it succeeded and the next call sees nothing. Reach the shared store
> through `self.engine`, never a fresh engine.

State lives in the SINGLE shared engine, keyed by type. Reach it through
`crate::state::service_state`, which `src/state.rs` generates.

Do not add a second engine crate — the router and every service must share
one `InMemoryEngine` type or the engine the router injects cannot be handed to this
service's operations.
