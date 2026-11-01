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
execution was handed, and it is resolved against the SHARED job directory the
SageMaker control plane writes when it mints a job
(`service_common::agentic_rft_jobs::JobDirectory`, reached from this crate through
`state::job_directory` and `state::job_exists`): a create in that control plane is
the only thing that makes a job addressable here, so its directory is the only place
an ARN can be resolved. Which partition, Region and account an ARN was addressed
through identifies no job, so a job the control plane created resolves however a
caller addresses it, and a trajectory is keyed by the job that resolution identifies
together with the request's `TrajectoryId` (`state::trajectory_key`). The same holds
for what a served turn REPORTS: the per-job values of its completion envelope — the
turn's `id` and the `model` that served it — are derived from the job that resolution
identifies (`state::job_identity`) and never from the ARN string one call spelled it
with, so every turn of one rollout, and every response body retained for it, names
one job (`tests/all/served_turn_reports_the_job.rs`).

A `JobArn` the member's own modelled constraints admit (`@length` 1..256 and the ARN
`@pattern`, which allow any partition, region, account and job name) but that no
create ever minted names no job, and EVERY operation of this data plane refuses such
a call rather than serving it:

- `Sample` and `SampleWithResponseStream` are refused `400 ValidationException`
  carrying `state::job_not_found_message`, before any turn is captured, because the
  turn can be routed to no model session (`state::serve_sampling_turn`);
- `CompleteRollout` and `UpdateReward` are refused `404 ResourceNotFoundException`
  carrying `state::trajectory_not_found_message`, because a trajectory lives inside
  the job its key names, so the pair names no trajectory of it either
  (`state::unresolved_job_trajectory_message`, reached through
  `state::serve_rollout_trajectory`).

A value that misses the modelled constraints never gets that far: it is refused
`400 ValidationException` by the front door (`Input arn … is malformed`). Whatever a
refusal says, it has to fit the member that carries it — every error shape of this
model declares one `Message` member targeting `FailureReason`, `@length` max 1024 — so
no single value a clause quotes passes that maximum less one clause's own prose
(`service::LARGEST_QUOTED_VALUE_BYTES`), the clauses of a sentence that states several
are held to what its prose leaves them together (`service::refusal_message`), and a
value longer than its room keeps the shortened rendering naming its byte count.

Beside job resolution, a sampling call is refused on state this service owns: a
trajectory `CompleteRollout` has already sealed, and the emulator's own store bounds
(`state::MAX_TURNS_PER_TRAJECTORY`, `MAX_TRAJECTORIES`, `MAX_TURN_BODY_BYTES`,
`MAX_RETAINED_BODY_BYTES`).

This section states the rule `src/state.rs` implements, and
`tests/all/job_resolution_is_documented_once.rs` reads it back and drives it over the
wire on every operation, so the two statements of it cannot drift apart again.
