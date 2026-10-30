Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5f1c460e5544105d602e680f733bf81495110ec8

# Enforce EntityId's modelled @length(1..255) on BatchDescribeEntities entries

_Filed: 2026-10-28T16:06:26Z_
_Operation: BatchDescribeEntities | Class: validation_

> _Note: this issue was identified against the code base at commit `5f1c460e5544105d602e680f733bf81495110ec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`EntityRequest$EntityId` carries `skip_validation`, so the boundary validator does NOT apply the modelled `EntityId` constraints to it; the handler replaces that check with `crate::state::is_entity_id`, which spells out only the `@pattern` and places NO bound on the length. The modelled `EntityId` shape is `@length(min: 1, max: 255)` as well as `@pattern`, and that same shape is the KEY of the `EntityDetails` and `Errors` maps this operation answers with.

Consequence: an entry whose `EntityId` is longer than 255 characters (but matches the character shape) is accepted, looked up, and reported back as a key of the `Errors` map — a map key the model bounds at 255 — so the 200 this call answers carries a response value outside the modelled constraint. The same id handed to `DescribeEntity` is refused as a client error, because `DescribeEntityInput` does not skip validation of `EntityId` and the boundary applies `ResourceId`'s `@length(min: 1, max: 255)` there. One read and a batch of them therefore disagree about the same id, which is exactly what the batch handler's own comment says they must not do ("the same two answers, under the same two codes, a single read of that entity gives").

The crate's other `skip_validation` members all fold the modelled length into their hand-written predicate; `is_entity_id` is the one that does not.

## Evidence

Smithy (smithy_query shape=EntityId / EntityRequest / Errors):
- `"EntityId": {"type": "string", "length": {"min": 1, "max": 255}, "pattern": "^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$"}`
- `"EntityRequest": { "EntityId": {"target": "EntityId", "required": true, "length": {"min": 1, "max": 255}, "pattern": "^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$"} }`
- `"Errors": {"type": "map", "key": {"target": "EntityId", "length": {"min": 1, "max": 255}, "pattern": "^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$"}, "value": "BatchDescribeErrorDetail"}` (same for `EntityDetails`)

The boundary is switched off for the member, and the handler is named as the replacement:
- `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/types/batch_describe_entities_types.rs:20-27`: "`skip_validation` names `Catalog` (see `CancelChangeSetInput`) and `EntityId`: a batch read reports a malformed entry PER ENTRY in `Errors`, so an `EntityId` outside the published form must reach the handler rather than failing the whole call at the boundary." … `skip_validation = ["Catalog", "EntityId"]`

The handler's only check is the pattern:
- `src/ops/batch_describe_entities.rs:188`: `if !crate::state::is_entity_id(&request.entity_id) {`
- `src/state.rs:3388-3397`: `pub fn is_entity_id(value: &str) -> bool { let mut middle = value.chars(); … first.is_ascii_alphanumeric() && last.is_ascii_alphanumeric() && !middle.as_str().is_empty() && middle.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '-')) }` — no length bound.

The sibling predicates for every other `skip_validation` member DO fold the modelled length in:
- `src/state.rs:3315-3316`: `pub fn is_change_set_name(value: &str) -> bool { (1..=CHANGE_SET_NAME_MAX).contains(&value.chars().count()) && …`
- `src/state.rs:3344-3346`: `pub fn is_client_request_token(value: &str) -> bool { (1..=CLIENT_REQUEST_TOKEN_MAX).contains(&value.chars().count()) && …`
- `src/state.rs:3246-3248`: `pub fn is_change_name(value: &str) -> bool { (1..=CHANGE_NAME_MAX).contains(&value.chars().count()) && …`
- `src/state.rs:3016`: `if value.is_empty() || value.chars().count() > CATALOG_NAME_MAX { return false; }` (`is_catalog_name`)

The single-read path refuses the same value, because its boundary validation is NOT skipped:
- `src/ops/types/describe_entity_types.rs:19`: `skip_validation = ["Catalog"]`

The crate's own tests treat the 255 as part of the contract but exercise only the bound, never past it:
- `tests/all/entity_reads.rs:252-255`: "… including at the published length bound (`ResourceId` and `EntityId` are both `@length(min: 1, max: 255)`)." / `let longest = "a".repeat(255);`
- `tests/all/request_validation.rs:904-941` batch table rows cover only `MALFORMED` and `MISSING` ids, so no test pins an over-long id.

## Steps to Reproduce

1. POST /BatchDescribeEntities with body {"EntityRequestList": [{"Catalog": "AWSMarketplace", "EntityId": "a" + "b"*300 + "c"}]} (302 characters, matching EntityId's pattern).
2. Observe 200 with an `Errors` map keyed by the 302-character id — a key whose modelled shape `EntityId` is `@length(min: 1, max: 255)`; expected instead a per-entry `ValidationException` entry (or no such key at all).
3. GET /DescribeEntity?catalog=AWSMarketplace&entityId=<the same 302-character id> and observe a client ValidationException from the boundary, proving the two read paths disagree about the same id.
