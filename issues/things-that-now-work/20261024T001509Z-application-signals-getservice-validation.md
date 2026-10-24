Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2c9ffd59ad5f58010cf4df26da4b1c3c27b76a2c

# GetService accepts KeyAttributes that name no entity type instead of refusing them

_Filed: 2026-10-24T00:15:09Z_
_Operation: GetService | Class: validation_

> _Note: this issue was identified against the code base at commit `2c9ffd59ad5f58010cf4df26da4b1c3c27b76a2c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetService` validates `KeyAttributes` only against the model's map bounds (`length 1..4`, key pattern `^[a-zA-Z]{1,50}$`), which the framework boundary enforces. The handler adds no rule of its own, so a request whose `KeyAttributes` omits `Type` entirely, or carries a `Type` value outside the documented set, is answered HTTP 200 with a `Service` whose `KeyAttributes` are the caller's own map echoed back.

Expected: the member documents the entity-identity rule — "You must specify at least the `Type`, `Name`, and `Environment` attributes", with `Type` designating the kind of object and `Name`/`Identifier`/`ResourceType` applicable only for particular `Type` values. A `KeyAttributes` map that names no `Type`, or whose `Type` is not one of the documented entity types (`Service`, `RemoteService`, `AWS::Service`, `Resource`, `AWS::Resource`), identifies no entity and is refused by the real service with the modelled `ValidationException` (HTTP 400).
Actual: HTTP 200 with the nonsense identity echoed as a real service entity (e.g. `{"Colour":"blue"}` comes back as `Service.KeyAttributes`), so a client that mis-spelled an attribute key gets a success instead of a refusal.

The same `KeyAttributes` member, with the identical documentation, is `@required` input on `ListServiceOperations`, `ListServiceDependencies` and `ListServiceDependents`, whose handlers pass it straight to the topology projection as well — the rule belongs beside the existing shared helpers in `crate::state` so all four reads agree.

## Evidence

Spec: `smithy_query(query="doc", shape="GetServiceInput", member="KeyAttributes")` — "You must specify at least the Type, Name, and Environment attributes", followed by the per-`Type` applicability of `ResourceType`, `Name` and `Identifier`; `ValidationException` (HTTP 400) is one of only two errors the operation declares. `smithy_query(query="shape", shape="Attributes")` shows the model expresses only the map bounds, so the entity rule cannot be enforced at the boundary.
Handler: `src/ops/get_service.rs:170-203` — the only check is `crate::state::time_range_refusal`; `input.key_attributes` goes straight to `observed_service`, which in `src/state.rs:954-975` starts from `key_attributes.clone()` and reports it as the entity identity when no stored SLO widens it.
Tests: `tests/all/get_service.rs:271-316` covers only the modelled map bounds (absent map, 0 entries, 5 entries); `tests/all/get_service.rs:326-412` asserts 200 for identities that all carry a valid `Type` (`Service`, `AWS::Resource`), so a `Type`-presence/value rule contradicts no existing assertion.

## Steps to Reproduce

1. POST /service?StartTime=<now-3600>&EndTime=<now+3600> with body {"KeyAttributes":{"Colour":"blue"}}.
2. Observe HTTP 200 with Service.KeyAttributes = {"Colour":"blue"}; expected HTTP 400 ValidationException because the map names no entity Type.
3. Repeat with {"KeyAttributes":{"Type":"NotAnEntityType","Name":"checkout","Environment":"prod"}} and observe HTTP 200 for a Type value outside the documented set.
4. Send the same two bodies to POST /service-operations, /service-dependencies and /service-dependents (with the required query time range) and observe the same unvalidated 200s.
