Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# Bound Entity$Type and Entity$Identifier at the modelled 255 characters in StartChangeSet

_Filed: 2026-10-28T16:06:26Z_
_Operation: StartChangeSet | Class: validation_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Entity$Type` and `Entity$Identifier` both carry `skip_validation`, and the crate states that the handler judges them instead (`is_entity_type_reference` / `is_entity_identifier`). Both modelled members are `@length(min: 1, max: 255)`, but neither predicate bounds the length:

* `is_entity_type_reference` accepts `<EntityType>@<version>` where `is_entity_type_version` admits an unbounded run of alphanumerics and dots. So `Entity.Type = "AmiProduct@" + "1"*1000` is accepted, stored verbatim on the change record, and reported back by `DescribeChangeSet` in `ChangeSummary$Entity$Type` — a value 1011 characters long, where the modelled `EntityType` is `@length(min: 1, max: 255)`. The emulator therefore persists and serves an out-of-contract value (a strict client would fail deserializing its own change set).
* `is_entity_identifier` likewise bounds neither alternative, so an `Entity.Identifier` of 300 well-formed characters passes the shape check and falls through to the store lookup, answering `ResourceNotFoundException` (404) for a value the model's `@length` forbids — while the handler's own comment says the identifier is judged "BEFORE the entity is looked up, so a malformed identifier is a refusal and only a well-formed one can be reported missing".

Every other hand-written predicate in this crate folds the modelled length into the published pattern it checks (and says so in its doc); these two are the exception.

## Evidence

Smithy (smithy_query shape=Entity,Identifier):
- `"Entity": { "Type": {"target": "EntityType", "required": true, "length": {"min": 1, "max": 255}, "pattern": "^[a-zA-Z]+$"}, "Identifier": {"target": "Identifier", "length": {"min": 1, "max": 255}, "pattern": "^[\\w\\-@]+$"} }`

Boundary validation switched off, with the handler named as the replacement:
- `src/ops/types/shared_types.rs:89-98`: "`skip_validation` names both members. `Type` is an entity type REFERENCE … `Identifier` has two published forms wider than the modelled `^[\w\-@]+$`. Both are judged by the handler (`crate::state::is_entity_type_reference` / `is_entity_identifier`), which answers the modelled `ValidationException` and its 422." … `skip_validation = ["Type", "Identifier"]`

The replacement checks carry no length bound:
- `src/state.rs:3121-3128`: `pub fn is_entity_type_reference(reference: &str) -> bool { match reference.split_once('@') { Some((entity_type, version)) => { is_entity_type(entity_type) && is_entity_type_version(version) } None => is_entity_type(reference), } }`
- `src/state.rs:3134-3140`: `fn is_entity_type_version(version: &str) -> bool { !version.is_empty() && version.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') && !version.starts_with('.') && …` — unbounded.
- `src/state.rs:3162-3176`: `pub fn is_entity_identifier(value: &str) -> bool { … match value.split_once('@') { … None => is_entity_id(value), } }`, and `is_entity_id` (`src/state.rs:3388`) bounds no length either.

The convention this breaks, stated in the crate itself:
- `src/state.rs:3299-3302`: "The syntax the service documents for `ChangeSetName`: the modelled `@pattern` with the modelled `@length(min: 1, max: 100)` folded into it" + `is_change_set_name` enforcing `(1..=CHANGE_SET_NAME_MAX)` at `src/state.rs:3316`; same for `is_client_request_token` (`src/state.rs:3344`) and `is_change_name` (`src/state.rs:3246`).

The over-long value is stored and served:
- `src/ops/start_change_set.rs:636`: `entity_type: change.entity.r#type.clone(),`
- `src/state.rs:679-682`: `entity: Some(Entity { r#type: self.entity_type.clone(), identifier: self.entity_identifier.clone(), }),` (the `ChangeSummary` `DescribeChangeSet` reports at `src/ops/describe_change_set.rs:217`).
- The tests treat that bound as the contract for reported ids: `tests/all/change_set_lifecycle.rs:45-62` `fn is_resource_id(value: &str) -> bool { !value.is_empty() && value.len() <= 255 && …}` / `fn is_identifier(value: &str) -> bool { !value.is_empty() && value.len() <= 255 && …}` — but no test submits an `Entity.Type`/`Entity.Identifier` past 255.

## Steps to Reproduce

1. POST /StartChangeSet with Catalog=AWSMarketplace and ChangeSet=[{"ChangeType":"CreateProduct","Entity":{"Type":"AmiProduct@" + "1"*1000},"Details":"{\"ProductTitle\":\"x\"}"}] — observe 200 instead of the modelled ValidationException for a member whose `@length` max is 255.
2. GET /DescribeChangeSet?catalog=AWSMarketplace&changeSetId=<id> and observe `ChangeSet[0].Entity.Type` reported as the 1011-character string, past `EntityType`'s `@length(min: 1, max: 255)`.
3. POST /StartChangeSet with Entity={"Type":"AmiProduct","Identifier":"a".repeat(300)} — observe 404 ResourceNotFoundException (the store lookup) for a value the modelled `Identifier` `@length` forbids, where the handler's stated rule is that a malformed identifier is refused before any lookup.
