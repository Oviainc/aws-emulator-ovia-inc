# service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice

Emulator implementation for the `com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice` service. This crate is
self-contained: building it does NOT rebuild the other services.

```bash
cargo build -p service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice
cargo test  -p service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice
```

## Layout

```
service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/
├── build.rs              # calls emulator_core_build_support::generate(...)
├── smithy-model/
│   └── com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice.json      # the Smithy model — drives routing + wire shapes
├── OPERATIONS.md         # every operation's contract, re-rendered from the model
├── tests/
│   └── state_roundtrip.rs  # compiled example of the state API
└── src/
    ├── lib.rs            # #[smithy_service] decl (generated — do not hand-edit)
    ├── service.rs        # impl Service — can_handle + routing/refusal overrides (yours to edit)
    ├── state.rs          # ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState — this service's in-memory state
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

## Job resolution

This service is a DATA plane: it serves the rollouts of an AgenticRFT job that is
already running, and its model binds no operation that creates, lists or deletes
one. The `JobArn` every operation carries is therefore the ARN the caller's own job
execution was handed, and the runtime keeps no registry of jobs to resolve it
against: every value the member's modelled constraints admit (`@length` 1..256 and
the ARN `@pattern`, which allow any partition, region, account and job name) names
the job the call is served for, and a trajectory is keyed by the
`(JobArn, TrajectoryId)` pair. A value that misses those constraints is still
refused `400 ValidationException` by the front door (`Input arn … is malformed`).

What a sampling call IS refused on is state this service owns: a trajectory
`CompleteRollout` has already sealed, and the emulator's own store bounds
(`state::MAX_TURNS_PER_TRAJECTORY`, `MAX_TRAJECTORIES`, `MAX_TURN_BODY_BYTES`,
`MAX_RETAINED_BODY_BYTES`).
