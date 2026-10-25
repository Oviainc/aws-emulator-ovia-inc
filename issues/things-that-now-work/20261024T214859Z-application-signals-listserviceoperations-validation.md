Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4af528f360b03ab2b05e9ad5a75be6ebefdc29e9

# Refuse an entity read whose KeyAttributes omit the documented minimum identity (Name/Environment, or ResourceType/Identifier)

_Filed: 2026-10-24T21:48:59Z_
_Operation: ListServiceOperations | Class: validation_

> _Note: this issue was identified against the code base at commit `4af528f360b03ab2b05e9ad5a75be6ebefdc29e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The four entity reads that take `KeyAttributes` as the entity to report on (`ListServiceOperations`, and the same member on `GetService`, `ListServiceDependencies`, `ListServiceDependents`) document a minimum identity: "You must specify at least the Type, Name, and Environment attributes", with the member's own per-type list making `Name` applicable to `Service`/`RemoteService`/`AWS::Service` and `ResourceType`/`Identifier` applicable to `Resource`/`AWS::Resource`.

Expected: a request whose `KeyAttributes` names only `Type` (e.g. `{"Type":"Service"}`), or `Type` + `Name` with no `Environment`, is refused with the modelled `ValidationException` (HTTP 400) naming `keyAttributes`, because the map identifies no service.

Actual: the emulator enforces only the FIRST half of that sentence. `crate::state::entity_identity_refusal` checks that `Type` is present and is one of the five documented entity types, and nothing else; a map carrying only `Type` is accepted and answered HTTP 200 — `ListServiceOperations`/`ListServiceDependencies`/`ListServiceDependents` return an empty list and `GetService` returns a `Service` whose `KeyAttributes` is the caller's own one-entry map echoed back, as though a service with that identity had been discovered.

The already-fixed issue `application-signals-getservice-validation` covered only the missing-`Type` half of the rule, so the `Name`/`Environment` (and `ResourceType`/`Identifier`) part of the documented minimum is still unenforced on every one of these reads. Note the rule belongs to this member only: `ListServiceLevelObjectives.KeyAttributes` is an optional filter and `ListEntityEvents.Entity` carries no such "must specify" sentence, so neither should be tightened.

## Evidence

Spec (Smithy model): `ListServiceOperationsInput$KeyAttributes` doc — "Use this field to specify which service you want to retrieve information for. You must specify at least the Type, Name, and Environment attributes." (identical text on `GetServiceInput`, `ListServiceDependenciesInput`, `ListServiceDependentsInput`), followed by the per-type applicability list (`Name` only for `Service`/`RemoteService`/`AWS::Service`; `ResourceType`/`Identifier` only for `Resource`/`AWS::Resource`).

Handler: `src/ops/list_service_operations.rs` calls `crate::state::entity_identity_refusal(&input.key_attributes)` and serves the request when it returns `None`. `src/state.rs` `entity_identity_refusal_at` (the shared rule, also used by `get_service.rs`, `list_service_dependencies.rs`, `list_service_dependents.rs`) matches only on `attributes.get("Type")`: absent -> "Member must contain the entity type attribute 'Type'", not in `ENTITY_TYPES` -> enum-set clause, `Some(_)` -> `return None`. No clause mentions `Name`, `Environment`, `ResourceType` or `Identifier`, and its doc comment states the choice explicitly ("Only `Type` is required here").

Test coverage: `tests/all/entity_identity.rs` (`an_identity_that_names_a_documented_entity_type_is_served`) pins the lenient behaviour with the row "the entity type on its own", `json!({ "Type": "Service" })`, asserting HTTP 200 from all four reads — so no test asserts the documented minimum identity; it is asserted to be accepted.

## Steps to Reproduce

1. Create an SLO so the store holds a discovered service: POST /slo with {"Name":"checkout-latency","SliConfig":{"SliMetricConfig":{"KeyAttributes":{"Type":"Service","Name":"checkout","Environment":"eks:prod/default"},"MetricType":"LATENCY"}}}
2. Call POST /service-operations?StartTime=<now-3600>&EndTime=<now+3600> with body {"KeyAttributes":{"Type":"Service"}} — the emulator answers HTTP 200 with an empty ServiceOperations list instead of a 400 ValidationException naming keyAttributes
3. Call the same route with body {"KeyAttributes":{"Type":"Service","Name":"checkout"}} (no Environment) — again HTTP 200
4. Repeat both bodies against POST /service-dependencies, POST /service-dependents and POST /service (GetService); GetService additionally answers 200 with Service.KeyAttributes equal to the one-entry map the caller sent
5. Repeat with a resource identity missing its applicable attributes, e.g. {"KeyAttributes":{"Type":"AWS::Resource"}} with no ResourceType/Identifier — also accepted
