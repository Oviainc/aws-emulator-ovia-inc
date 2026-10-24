# service-com-amazonaws-groundstation-groundstation

Emulator implementation for the `com-amazonaws-groundstation-groundstation` service. This crate is
self-contained: building it does NOT rebuild the other services.

```bash
cargo build -p service-com-amazonaws-groundstation-groundstation
cargo test  -p service-com-amazonaws-groundstation-groundstation
```

## Layout

```
service-com-amazonaws-groundstation-groundstation/
├── build.rs              # calls emulator_core_build_support::generate(...)
├── smithy-model/
│   └── com-amazonaws-groundstation-groundstation.json      # the Smithy model — drives routing + wire shapes
├── OPERATIONS.md         # every operation's contract, re-rendered from the model
├── tests/
│   └── state_roundtrip.rs  # compiled example of the state API
└── src/
    ├── lib.rs            # #[smithy_service] decl (generated — do not hand-edit)
    ├── service.rs        # impl Service — can_handle + routing/refusal overrides (yours to edit)
    ├── state.rs          # ComAmazonawsGroundstationGroundstationState — this service's in-memory state
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

## Operator settings

| Variable | What it does |
|---|---|
| `GROUNDSTATION_APPROVED_UPLINK_BANDS_MHZ` | The uplink frequency bands this emulated account is approved to transmit on, as comma-separated `<low>-<high>` pairs in MHz (`2025-2120` approves the whole band the service operates). Read ONCE, when this service's store is first created, so setting it after the first request has no effect. |
| `GROUNDSTATION_RESERVED_MINUTES` | The reserved-minutes pricing plan this account holds, as comma-separated entries: a bare minute count is a standing monthly commitment (`500`) and `<year>-<month>=<minutes>` is one calendar month's own allocation (`500,2026-02=750`). Read once, with the same timing. |
| `GROUNDSTATION_SATELLITES` | The satellites this account is on-boarded to, as comma-separated entries: a bare NORAD catalogue number (`25994`) or `<norad>=<ground station>+<ground station>…` naming the ground stations the satellite is on-boarded to (`25994=Ohio 1+Oregon 1,27424`). Read once, with the same timing. |

Both settings stand for something that happens OUTSIDE the Ground Station API, which is
why they are settings rather than operations:

- Uplink approval is granted by AWS after spectrum coordination — the model has no
  operation that grants one and none that reports one — so an account starts with none
  and `CreateConfig`/`UpdateConfig` refuse every `antennaUplinkConfig` with
  `InvalidParameterException` ("The specified Uplink Frequency has not been approved."),
  which is what AWS answers for an unapproved account.
- A reserved-minutes commitment is bought from AWS, not through this API: the model has
  no operation that signs an account up and none that reports the signup other than
  `GetMinuteUsage` itself. So an account starts on on-demand pricing and `GetMinuteUsage`
  answers `isReservedMinutesCustomer: false` with a zero allocation until a plan is listed
  here; the minutes it reports as scheduled and upcoming are computed from the contacts
  the account actually reserved either way.

- A satellite is on-boarded to an account by AWS, after it has reviewed the spacecraft's
  licensing and configuration: the model declares no operation that onboards one and none
  that reports one other than `ListSatellites`/`GetSatellite` themselves. So an account
  starts on-boarded to no satellite and `ListSatellites` answers an empty list until an
  onboarding is listed here; each entry is minted a `satelliteId` and a region-less
  `satelliteArn` when the store is created.

`src/state.rs`'s `APPROVED_UPLINK_BANDS_ENV`, `RESERVED_MINUTES_ENV` and `SATELLITES_ENV`
are the three readers, and all land in this service's own state, where a test seeds them
directly.

An `s3RecordingConfig`'s recording bucket needs NO setting: Ground Station resolves the
bucket the `bucketArn` names against the emulator's own S3 (`HeadBucket` over the
in-process callback), so a bucket created through the emulator's S3 is accepted and only
a `bucketArn` naming no bucket is refused with `InvalidParameterException` ("Unable to
create new S3 endpoint config - bucket does not exist."). With no S3 mounted to ask,
nothing is known about the bucket and the config is created.
