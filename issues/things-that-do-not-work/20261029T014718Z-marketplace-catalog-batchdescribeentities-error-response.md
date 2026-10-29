Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 1211f7611505016a47ab1bf8932fdc778b7fd399

# A per-entry failure is keyed by the raw submitted EntityId, so the Errors map carries a key (and a message) the modelled shapes cannot hold

_Filed: 2026-10-29T01:47:18Z_
_Operation: BatchDescribeEntities | Class: error-response_

> _Note: this issue was identified against the code base at commit `1211f7611505016a47ab1bf8932fdc778b7fd399`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`BatchDescribeEntitiesResponse$Errors` is modelled as a map whose KEY targets `EntityId` — `@length(min: 1, max: 255)` and `@pattern("^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$")`. The handler keys that map with the submitted entry value verbatim, including the value it has just decided is NOT an entity id, and `EntityRequest$EntityId` carries `skip_validation` so the boundary enforced neither the pattern nor the 1..255 length before the handler ran. Three concrete results, all 200 answers whose body is outside the modelled output shape:

1. an entry of `"bad entity id!"` is reported under the `Errors` key `"bad entity id!"`, which the key shape's pattern excludes (no spaces, no `!`);
2. an entry of `""` is reported under the empty key, which `@length(min: 1)` excludes;
3. an entry of 600 alphanumerics passes `is_entity_id` (which checks characters only, never a length — state.rs:3446-3455), so it is used as a key past the modelled max of 255 AND spliced whole into `BatchDescribeErrorDetail$ErrorMessage`, modelled `@length(min: 1, max: 2048)`.

Symptom 3 is also the one echo rule this crate states for itself and applies everywhere else: `quoted()` and `scope_text()` exist purely to cut a submitted value so the message built around it stays inside the modelled length, and this path skips them. The sibling single read (`DescribeEntity`) is safe only by accident — its `EntityId` is NOT skip_validation, so the boundary caps it at 255 before `invalid_entity_id_message` quotes it.

Note for the fixer: `tests/all/request_validation.rs` currently PINS the defect (`"Errors is keyed by the exact requested id"` with `MALFORMED = "bad entity id!"`), so that expectation has to move with the fix — the modelled key shape contradicts that stated reason.

## Evidence

Model (smithy_query shape="Errors"): `{"shape_id": "com.amazonaws.marketplacecatalog#Errors", "type": "map", "key": {"target": "EntityId", "length": {"min": 1, "max": 255}, "pattern": "^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$"}, "value": "BatchDescribeErrorDetail"}` and `BatchDescribeErrorDetail$ErrorMessage` -> `BatchDescribeErrorMessageContent`, `"length": {"min": 1, "max": 2048}`.

Handler, keying the map with the value it just refused — services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/ops/batch_describe_entities.rs:188-197:
```rust
            if !crate::state::is_entity_id(&request.entity_id) {
                errors.insert(
                    request.entity_id.clone(),
                    BatchDescribeErrorDetail {
                        error_code: Some(crate::state::BATCH_ERROR_VALIDATION.to_string()),
                        error_message: Some(crate::state::invalid_entity_id_message(
                            &request.entity_id,
                        )),
```
and the same for a well-formed-but-unbounded id at batch_describe_entities.rs:220-229 (`errors.insert(request.entity_id.clone(), ... entity_missing_message(&request.entity_id) ...)`).

Why the raw value reaches the handler — src/ops/types/batch_describe_entities_types.rs:20-26:
```rust
// `skip_validation` names `Catalog` (see `CancelChangeSetInput`) and `EntityId`: a batch
// read reports a malformed entry PER ENTRY in `Errors`, so an `EntityId` outside the
// published form must reach the handler rather than failing the whole call at the
// boundary.
    skip_validation = ["Catalog", "EntityId"]
```

No length check in the predicate — src/state.rs:3446-3455 (`is_entity_id`) tests only the first/last/middle CHARACTERS; the message helper splices the value whole, src/state.rs:3471-3476:
```rust
pub fn invalid_entity_id_message(entity_id: &str) -> String {
    format!(
        "Requested entity id '{entity_id}' is invalid. \
         It should match with {ENTITY_ID_PATTERN}."
```

The crate's own echo-bounding rule this path skips — src/state.rs:2534-2543:
```rust
/// A submitted details value as a message quotes it: its JSON rendering, cut at
/// [`DETAIL_VALUE_QUOTE_LIMIT`] characters so the message built around it stays
/// inside `ExceptionMessageContent`'s 2048.
fn quoted(value: &Value) -> String {
```
and src/state.rs:2189-2201 (`scope_text`: "cut at [`SCOPE_VALUE_LIMIT`] characters so it stays inside `ScopeValue`'s modelled length").

Test coverage that pins today's behaviour: tests/all/request_validation.rs:889-979 — `const MALFORMED: &str = "bad entity id!";` ... `"case {case:?}: Errors is keyed by the exact requested id, got {:?}"`. No test exercises an over-long or empty entry id.

## Steps to Reproduce

1. POST /BatchDescribeEntities with body {"EntityRequestList":[{"Catalog":"AWSMarketplace","EntityId":"bad entity id!"}]} — the 200 answer carries Errors keyed "bad entity id!", a key the modelled key shape's pattern ^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$ cannot hold.
2. POST /BatchDescribeEntities with EntityId set to an empty string — the 200 answer carries an Errors key of "", which the key shape's @length(min: 1) excludes.
3. POST /BatchDescribeEntities with EntityId set to 600 'a' characters — is_entity_id accepts it (characters only, no length), so the answer carries an Errors key of 600 characters (modelled max 255) and an ErrorMessage that quotes all 600 (modelled max 2048 for the whole message).
4. Compare with GET /DescribeEntity?catalog=AWSMarketplace&entityId=<the same 600-character value>, where the boundary's ResourceId @length(1..255) refuses it before invalid_entity_id_message ever quotes it.
