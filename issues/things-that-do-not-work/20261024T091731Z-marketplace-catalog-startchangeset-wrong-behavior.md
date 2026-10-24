Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 671706971a680016eadecf6b3438021fa34d8a56

# Resolve the $<ChangeName>.Entity.Identifier reference inside a multi-change change set

_Filed: 2026-10-24T09:17:31Z_
_Operation: StartChangeSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `671706971a680016eadecf6b3438021fa34d8a56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AWS Marketplace Catalog lets one change in a change set act on the entity that ANOTHER change of the same change set creates, by naming that change: `Entity.Identifier = "$<ChangeName>.Entity.Identifier"` (the documented pattern for e.g. `CreateProduct` + `AddDeliveryOptions`/`ReleaseProduct` in a single StartChangeSet call).

The emulator accepts that syntax but can never resolve it:

1. `src/ops/start_change_set.rs` validates the identifier with `crate::state::is_entity_identifier`, which explicitly admits `$<ChangeName>.Entity.Identifier` (see `ENTITY_IDENTIFIER_PATTERN` in `src/state.rs`). It then calls `crate::state::entity_id_of(identifier)` (`identifier.split('@').next()`), which for a change reference returns the WHOLE literal string, and looks it up with `store.resolve_identifier(identifier)` -> `entities.get("$Foo.Entity.Identifier")` -> `None` -> `ResourceNotFoundException` (HTTP 404). So every change set that uses the documented reference form is refused, instead of applying the second change to the entity the first change creates.
2. A second, compounding symptom: `Change` carries no `skip_validation` (`src/ops/types/start_change_set_types.rs`), so the boundary enforces the model's `ChangeName` `@pattern` `^[a-zA-Z]$` literally — only a SINGLE letter is accepted, and a realistic change name (`"CreateNewProduct"`) is refused with 422. Every other member whose published syntax is wider than the model's pattern (`Catalog`, `Entity$Type`, `Entity$Identifier`) is already relaxed in this crate and judged by the handler; `ChangeName` is not, so no change name a real caller sends (or that the reference form could point at) can be submitted at all.

Expected: a change set whose second change names `$<ChangeName>.Entity.Identifier` resolves that reference to the entity id reserved for the referenced change, applies both changes to the same entity, and DescribeChangeSet/DescribeEntity report one entity with the merged details. Actual: 404 ResourceNotFoundException for the reference, and 422 for any multi-letter ChangeName.

## Evidence

Spec: `Change$ChangeName` is modeled ("Optional name for the change") and `Entity$Identifier`'s published form includes the change reference — the crate's own `src/state.rs` documents it as `(^\$[a-zA-Z]{1,72}\.Entity\.Identifier$)` and `tests/all/request_validation.rs:60-64` restates it, so the emulator has adopted the form but implemented no resolution. Handler: `src/ops/start_change_set.rs` (identifier branch calling `entity_id_of` then `store.resolve_identifier`, returning `ResourceNotFoundException`) and `src/state.rs::entity_id_of` which only strips `@<revision>`. Boundary: `src/ops/types/start_change_set_types.rs` lists `skip_validation = ["Catalog", "ChangeSetName", "ClientRequestToken"]` — `Change`/`ChangeName` is absent. Tests: grep of `tests/` for `Entity.Identifier` finds only the pattern constant in `request_validation.rs`; no test exercises a change set that references another change, so nothing asserts the correct behavior.

## Steps to Reproduce

1. POST /StartChangeSet with Catalog=AWSMarketplace and ChangeSet=[{"ChangeType":"CreateProduct","ChangeName":"CreateNewProduct","Entity":{"Type":"AmiProduct@1.0"},"Details":"{\"ProductTitle\":\"t\"}"},{"ChangeType":"AddDeliveryOptions","Entity":{"Type":"AmiProduct@1.0","Identifier":"$CreateNewProduct.Entity.Identifier"},"Details":"{}"}]
2. Observe HTTP 422 ValidationException because ChangeName="CreateNewProduct" fails the literally-enforced `^[a-zA-Z]$` pattern
3. Retry with ChangeName="C" and Identifier="$C.Entity.Identifier" — observe HTTP 404 ResourceNotFoundException naming `$C.Entity.Identifier` as a missing entity
4. Expected: 200 with a ChangeSetId; after the change set succeeds, DescribeChangeSet shows both changes against one entity and DescribeEntity returns the merged details for the single created entity
