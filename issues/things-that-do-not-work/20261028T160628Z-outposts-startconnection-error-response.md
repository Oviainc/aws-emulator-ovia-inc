Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: e39df06442d4f2a86a0fa0e3a1f1c22c399555b8

# StartConnection reports the emulator's own connection-store ceiling as a 500 InternalServerException, while the sibling creates report theirs as the 400 they model

_Filed: 2026-10-28T16:06:28Z_
_Operation: StartConnection | Class: error-response_

> _Note: this issue was identified against the code base at commit `e39df06442d4f2a86a0fa0e3a1f1c22c399555b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`MAX_RESOURCES` is the emulator's own in-memory entry ceiling, and this crate states one rule for refusing a create that would cross it: an operation that models no `ServiceQuotaExceededException` reports the cap "from the caller's side" through a client error it does model, never as a server fault. `CreateQuote` and `CreateRenewal` both do exactly that with a 400 `ValidationException` and the shared `store_full`-style wording. `StartConnection` grows the same kind of capped store and also models `ValidationException` (400), yet it answers `InternalServerException` (HTTP 500) — a server fault for a request the service will never accept, which SDK retry classifiers treat as retryable, so a client retries a call that can only fail again. Its own comment justifies the 500 with "This operation models no quota error", which is the identical premise `CreateRenewal`'s comment uses to reach the opposite conclusion; the crate's single statement of the cap policy (state.rs) enumerates the refusals of the three 402 creates, of `CreateQuote` and of `StartCapacityTask` and does not cover this path at all.

## Evidence

The outlier — services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/start_connection.rs:192-203:
```
// The store grows by one connection per call and lives for the life of the
// process, so the growth is capped. This operation models no quota error and
// the ceiling is the emulator's own rather than anything about the request,
// so past it the server error it does model is what the caller is told.
if store.connections.len() >= crate::state::MAX_RESOURCES {
    return Err(StartConnectionErrorModeled::InternalServerException(
        InternalServerException {
            message: Some(crate::state::store_full("connections")),
        },
    )
    .into());
```
`StartConnection` does model the client error: start_connection.rs:105-110 `pub enum StartConnectionErrorModeled { AccessDeniedException, InternalServerException, NotFoundException, ValidationException }`.

The sibling facing the identical premise — src/ops/create_renewal.rs:278-290:
```
// See `MAX_RESOURCES`: a renewal is never released, so what this contract
// grows is capped. `CreateRenewal` models no quota error, so the cap is
// reported as what it is from the caller's side — a request this service will
// not accept (400 `ValidationException`) — exactly as `CreateQuote` reports
// its own, and never as a 501 or an invented error code.
if store.renewals_held() >= crate::state::MAX_RESOURCES {
    return Err(CreateRenewalErrorModeled::ValidationException(
        ValidationException { message: Some(crate::state::store_full("renewal contracts")) },
```
and src/ops/create_quote.rs:238-253: "`CreateQuote` models no quota error, so the cap is reported as what it is from the caller's side — a request this service will not accept (400 `ValidationException`)".

The crate's one statement of the policy, src/state.rs:5756-5763, lists every refusal it knows about and does not include this one:
```
/// Past the cap the creating operation refuses the request rather than growing:
/// the three creates that model `ServiceQuotaExceededException` (HTTP 402) answer it
/// ..., `CreateQuote`, whose model declares no quota error, refuses the create as a
/// bad request (400 `ValidationException`) rather than inventing an error the model
/// does not carry, and `StartCapacityTask`, which models no quota error either,
/// answers the `ConflictException` (HTTP 409) it does model, worded by `store_full`
```
That a 5xx is the wrong envelope for "this can never succeed" is stated in src/scaffolded.rs:282-284: "Keep it 501 and not a 5xx server fault: SDK retry classifiers treat those as retryable, so a client would retry an operation that can never succeed."

No test pins the connection-store refusal: grep for `store_full` in tests/ returns nothing.

## Steps to Reproduce

1. Create a site and an Outpost, place an order and let it settle so the Outpost holds an installed asset (ListAssets returns an AssetId).
2. Call POST /connections with that AssetId and a valid 44-character ClientPublicKey until the store holds crate::state::MAX_RESOURCES connections.
3. Call POST /connections once more.
4. Observe HTTP 500 InternalServerException (a retryable server fault) carrying the store_full text, instead of the 400 ValidationException that CreateQuote and CreateRenewal answer for the same emulator-internal ceiling.
