Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 7cfd583752b909e6f589468ac42594db7887f86f

# CreateTable checks the per-table column cap last, so an oversized schema is graded by the type-resolution scan (404) instead of the quota (402) and that scan runs under the state lock

_Filed: 2026-10-28T19:04:49Z_
_Operation: CreateTable | Class: error-response_

> _Note: this issue was identified against the code base at commit `7cfd583752b909e6f589468ac42594db7887f86f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateTable` runs both of its per-column scans BEFORE it refuses a schema that is past `MAX_COLUMNS_PER_TABLE`: the duplicate-name scan (`crate::state::validate_schema`) at create_table.rs:265, and the per-column user-defined-type resolution loop at create_table.rs:357-366 — the latter while this service's single state lock is held (`state.lock()` at create_table.rs:315). The quota refusal only comes afterwards, at create_table.rs:398.

Its two siblings in this same crate do it the other way round and say why. `UpdateTable` checks the column cap at update_table.rs:557 and only then walks the added columns for duplicate names and unresolvable types (update_table.rs:581-614). `CreateType` checks `MAX_FIELDS_PER_TYPE` at create_type.rs:200 before walking the field list at create_type.rs:227-234.

Two consequences follow from the ordering:

1. The same pair of faults is graded in opposite order by two handlers of one crate. A `CreateTable` whose `allColumns` is past the cap AND names a column type the keyspace cannot resolve answers 404 `ResourceNotFoundException` (`UDT <name> does not exist`), while the analogous `UpdateTable` `addColumns` request answers 402 `ServiceQuotaExceededException`. Both operations model both errors, and this crate states explicitly that siblings must not grade one request shape differently (tag_resource.rs:174-179).

2. `allColumns` is modelled `len 1..` with NO maximum, so the type-resolution loop is a caller-sized scan that runs while the state lock is held, before anything refuses the size. That is the cost the cap exists to bound, per its own doc (state.rs:570-576) and per `CreateType`'s comment.

Moving the `MAX_COLUMNS_PER_TABLE` check ahead of the two scans fixes both and changes no accepted request: the existing cap test still gets 402.

## Evidence

The cap is the LAST schema check in `CreateTable`, after the type scan that already ran under the lock —

`services/.../src/ops/create_table.rs:315`: `let mut store = state.lock();`
`services/.../src/ops/create_table.rs:357-366`:
```
for column in &input.schema_definition.all_columns {
    if let Some(referenced) =
        crate::state::unresolvable_type_reference(&column.r#type, &usable_type_names)
    {
        return Err(CreateTableErrorModeled::ResourceNotFoundException(
            crate::state::type_not_found(&region, &input.keyspace_name, &referenced),
        )
        .into());
```
`services/.../src/ops/create_table.rs:395-399`:
```
// A table's schema is the store's third caller-driven growth dimension, and
// `allColumns` is modelled with no maximum, so it is capped the same way — here,
// and in `UpdateTable`, which is the only operation that can grow it afterwards.
if input.schema_definition.all_columns.len() > crate::state::MAX_COLUMNS_PER_TABLE {
```

The siblings put the cap FIRST and state the reason —

`services/.../src/ops/update_table.rs:553-557`:
```
// cap the way this crate caps its other two caller-driven growth dimensions, refused past
// the cap with the quota error this operation models — checked BEFORE anything is
// stored, so neither one oversized list nor a run of updates can grow the
// schema without bound, and the scan below stays bounded too.
if schema.all_columns.len() + added.len() > crate::state::MAX_COLUMNS_PER_TABLE {
```
`services/.../src/ops/create_type.rs:196-200`:
```
// `fieldDefinitions` is modelled `len 1..` with NO maximum, so the cap is this
// emulator's: it bounds what one call can store AND the work every later
// `GetType` pays to derive its answers from those fields. Checked before the scan
// below, so what a refused request costs stays proportional to the cap.
if input.field_definitions.len() > crate::state::MAX_FIELDS_PER_TYPE {
```

The crate's own rule about grading two faults consistently across siblings —
`services/.../src/ops/tag_resource.rs:174-179`: "of two faults in one request, the one that is wrong about the TAGS the request carries is the one the caller has to fix first ... `UntagResource` grades the two in the same order, so one request shape never gets two different messages depending on which sibling served it."

And the cap's own doc, which the under-lock scan defeats —
`services/.../src/state.rs:570-576`: "`addColumns` is modelled `len 1..`, with no maximum ... Both the create and the update refuse past this cap ... It also caps the duplicate-name scan an `addColumns` update runs while holding the state lock."

Test coverage: `tests/all/update_table.rs:859-880` creates a table with `MAX_COLUMNS_PER_TABLE + 1` columns of resolvable types and pins 402 `ServiceQuotaExceededException`; `tests/all/update_table.rs:789-857` pins the same for `addColumns`. Neither row names an unresolvable type, so neither pins the ordering under test here, and checking the cap first keeps both green.

## Steps to Reproduce

1. CreateKeyspace `{"keyspaceName": "ord_ks"}` and wait for it to settle.
2. CreateTable `{"keyspaceName": "ord_ks", "tableName": "too_wide", "schemaDefinition": {"allColumns": [{"name":"pk","type":"text"}, {"name":"c0","type":"nosuchtype"}, ... 1024 more int columns ...], "partitionKeys": [{"name":"pk"}]}}` — i.e. more than `MAX_COLUMNS_PER_TABLE` columns, one of which names a type the keyspace does not hold.
3. Observe 404 `ResourceNotFoundException` (`UDT nosuchtype does not exist`): the type scan at create_table.rs:357-366 ran first, under the state lock, over the whole oversized list.
4. Now the sibling: CreateTable a normal table in `ord_ks`, let it settle, then UpdateTable `{"keyspaceName": "ord_ks", "tableName": "<it>", "addColumns": [ ... more than MAX_COLUMNS_PER_TABLE entries, one of type "nosuchtype" ... ]}`.
5. Observe 402 `ServiceQuotaExceededException` — the same two faults, the opposite grading, from the handler that checks the cap first (update_table.rs:557).
