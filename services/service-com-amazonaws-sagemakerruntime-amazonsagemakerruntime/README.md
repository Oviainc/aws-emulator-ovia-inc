# service-com-amazonaws-sagemakerruntime-amazonsagemakerruntime

Emulator implementation for the `com-amazonaws-sagemakerruntime-amazonsagemakerruntime` service. This crate is
self-contained: building it does NOT rebuild the other services.

```bash
cargo build -p service-com-amazonaws-sagemakerruntime-amazonsagemakerruntime
cargo test  -p service-com-amazonaws-sagemakerruntime-amazonsagemakerruntime
```

## Layout

```
service-com-amazonaws-sagemakerruntime-amazonsagemakerruntime/
├── build.rs              # calls emulator_core_build_support::generate(...)
├── smithy-model/
│   └── com-amazonaws-sagemakerruntime-amazonsagemakerruntime.json      # the Smithy model — drives routing + wire shapes
├── OPERATIONS.md         # every operation's contract, re-rendered from the model
├── tests/
│   └── state_roundtrip.rs  # compiled example of the state API
└── src/
    ├── lib.rs            # #[smithy_service] decl (generated — do not hand-edit)
    ├── service.rs        # impl Service — can_handle + routing/refusal overrides (yours to edit)
    ├── state.rs          # ComAmazonawsSagemakerruntimeAmazonsagemakerruntimeState — this service's in-memory state
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

## Where this service's endpoints come from

SageMaker Runtime is a DATA PLANE: its model declares only `InvokeEndpoint`,
`InvokeEndpointAsync` and `InvokeEndpointWithResponseStream`, and no operation that
creates, updates or describes an endpoint — that belongs to the SageMaker control plane
(`services/service-com-amazonaws-sagemaker-sagemaker`). So this crate keeps NO endpoint
of its own: every invocation resolves the endpoint it names in the control plane's own
store, through the one shared engine, and a name that store does not hold is refused with
the modelled `ValidationError` (HTTP 400) rather than answered.

That question — "does the account host this endpoint, and is a deployment behind it?" —
is asked in ONE place, `crate::state::resolve_endpoint`, and each of the three operations
maps the answer onto its own modelled errors. It is also the only place that writes to the
control plane, and only to materialise the `Creating -> InService` transition the clock
already owed (the same write `DescribeEndpoint` makes).

What this crate's own state holds is what the data plane itself produced: the bounded log
of the invocations it answered (`EndpointInvocation`, with the request's non-sensitive
members and the outcome) and the bounded queue of the asynchronous submissions it accepted
(`AsyncInvocation`).

An in-process caller that needs an endpoint creates it through the control plane, exactly
as a client would, not through this crate:

```rust
// `CreateModel` -> `CreateEndpointConfig` -> `CreateEndpoint` on the control plane, then
// invoke through this service. `tests/all/endpoint_invocation.rs` does precisely this.
```

A real endpoint runs a customer-owned inference container — customer code in a container
image, which a generated emulator does not run. So NOTHING here computes a prediction:
an invocation that resolves is answered with the modelled `ModelError` (HTTP 424) saying
no container returned an inference, never a synthesised `Body`; and an accepted
asynchronous submission leaves `InProgress` for the modelled failure state once its
outcome falls due, never a `Completed` claiming an output object nothing wrote.

`InvokeEndpointWithResponseStream` is HAND-AUTHORED (the scaffold generator declines a
`@streaming` event-stream output — see `src/ops/SKIPPED_STUBS.md`) and is routed through
`Service::override_operation` in `src/service.rs`. It frames no event stream: a
`PayloadPart` carries the model's own output, and nothing produced one.
