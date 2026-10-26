# service-com-amazonaws-backupgateway-backuponpremises-v20210101

Emulator implementation for the `com-amazonaws-backupgateway-backuponpremises-v20210101` service. This crate is
self-contained: building it does NOT rebuild the other services.

```bash
cargo build -p service-com-amazonaws-backupgateway-backuponpremises-v20210101
cargo test  -p service-com-amazonaws-backupgateway-backuponpremises-v20210101
```

## Layout

```
service-com-amazonaws-backupgateway-backuponpremises-v20210101/
├── build.rs              # calls emulator_core_build_support::generate(...)
├── smithy-model/
│   └── com-amazonaws-backupgateway-backuponpremises-v20210101.json      # the Smithy model — drives routing + wire shapes
├── OPERATIONS.md         # every operation's contract, re-rendered from the model
├── tests/
│   └── state_roundtrip.rs  # compiled example of the state API
└── src/
    ├── lib.rs            # #[smithy_service] decl (generated — do not hand-edit)
    ├── service.rs        # impl Service — can_handle + routing/refusal overrides (yours to edit)
    ├── state.rs          # ComAmazonawsBackupgatewayBackuponpremisesV20210101State — this service's in-memory state
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

## The activation handshake (how to get an `ActivationKey`)

`CreateGateway` takes an `ActivationKey` and no operation of this model issues one: on AWS
the key is minted when the gateway appliance a customer deployed makes its activation
handshake — an unsigned HTTP request, made outside the SDK, that names the region the key
is for and is answered with the key itself.

This emulator serves that handshake (`src/service/activation.rs`), because it stands in for
the appliance as much as for the service, and `CreateGateway` REDEEMS the key against the
activations it issued. So the way to create a gateway is two requests:

```sh
# 1. the handshake: an unsigned GET of the endpoint root, answered with the key
key=$(curl -sk "https://localhost:9297/?activationRegion=us-west-2&gatewayType=BACKUP_VM&no_redirect")

# 2. the create, which redeems that activation
aws backup-gateway create-gateway --activation-key "$key" \
    --gateway-display-name my-gateway --gateway-type BACKUP_VM
```

A key nothing issued, one a create already redeemed, or one issued for another region is
refused with HTTP 400 `ValidationException` / `ErrorCode` `ActivationKeyNotFound`, which is
what AWS answers a key it has no activation record for.
