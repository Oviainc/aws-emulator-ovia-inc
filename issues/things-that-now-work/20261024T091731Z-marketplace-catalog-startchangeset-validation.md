Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 671706971a680016eadecf6b3438021fa34d8a56

# Unsupported ChangeType is accepted and the change set always succeeds

_Filed: 2026-10-24T09:17:31Z_
_Operation: StartChangeSet | Class: validation_

> _Note: this issue was identified against the code base at commit `671706971a680016eadecf6b3438021fa34d8a56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Change$ChangeType` is documented as "single string values that describe your intention for the change. Each change type is unique for each EntityType provided in the change's scope" — the service has a fixed, per-entity-type set (CreateProduct, AddDeliveryOptions, ReleaseProduct, UpdateInformation, ChangeVisibility, ... for AmiProduct; a different set for ContainerProduct, Offer, ResaleAuthorization).

The emulator never looks at the value beyond the model's `^[A-Z][\w]*$` pattern: `src/ops/start_change_set.rs` stores `change.change_type` verbatim and `src/state.rs::apply_changes` merges the change's `Details` into the entity regardless of the change type. Consequences:
- `ChangeType="Nonsense"` (or a change type that does not exist for the named EntityType, e.g. `AddDeliveryOptions` on an `Offer`) is accepted with HTTP 200 and then reported as SUCCEEDED.
- A change that cannot apply still creates/updates an entity: a change with no `Entity.Identifier` ALWAYS mints a new entity id when Intent=APPLY, whatever its change type.
- No change set can ever reach `FAILED`: `ChangeSetRecord.failure_code`/`failure_description` are hardcoded to `None` at creation and `ChangeSummary.error_detail_list` is hardcoded `None` in `ChangeRecord::to_summary`, so the modeled `FailureCode` (CLIENT_ERROR | SERVER_FAULT), `FailureDescription` and `ErrorDetailList` are unreachable.

Expected: an unsupported ChangeType for the named entity type is refused with `ValidationException` (HTTP 422, `ValidationExceptionField` naming the ChangeType/EntityType), or at minimum the change set reaches `Status=FAILED` with a `FailureCode` and a populated `ChangeSummary.ErrorDetailList`. Actual: HTTP 200, then SUCCEEDED with the details merged into an entity.

## Evidence

Spec/model: `Change$ChangeType` doc (smithy_query on shape `Change`) states change types are a per-EntityType set; `ValidationExceptionField` carries `ChangeType`/`EntityType`/`EntityId` members precisely to report a bad change; `ChangeStatus` includes FAILED, `DescribeChangeSetResponse$FailureCode`/`FailureDescription` and `ChangeSummary$ErrorDetailList` are modeled outputs. Handler: `src/ops/start_change_set.rs` performs no ChangeType check (only pattern-level boundary validation) and writes `failure_code: None, failure_description: None`; `src/state.rs::apply_changes` ignores `change.change_type` entirely; `ChangeRecord::to_summary` sets `error_detail_list: None` with the comment "No change this emulator applies fails". Tests: `tests/all/change_set_lifecycle.rs` asserts only success paths (`a_started_change_set_is_reported_from_what_was_stored`, `a_change_against_an_existing_entity_updates_it`); no test asserts a refusal or a FAILED change set for a bad change type.

## Steps to Reproduce

1. POST /StartChangeSet with Catalog=AWSMarketplace and ChangeSet=[{"ChangeType":"Nonsense","Entity":{"Type":"AmiProduct@1.0"},"Details":"{\"ProductTitle\":\"t\"}"}]
2. Observe HTTP 200 with a ChangeSetId; expected a ValidationException (422) for a change type the entity type has no such change for
3. GET /DescribeChangeSet?catalog=AWSMarketplace&changeSetId=<id> after a moment — observe Status=SUCCEEDED with no FailureCode and ChangeSummary without ErrorDetailList, and a new entity now visible in ListEntities/DescribeEntity
