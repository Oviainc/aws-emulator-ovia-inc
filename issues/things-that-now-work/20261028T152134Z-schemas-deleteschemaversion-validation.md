Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7e4efc65cc687607a3c1322cd06c2fb91d881b32

# A signed SchemaVersion label ("+1") resolves to a stored version and deletes it, though the crate's one definition of a version label says a sign names no version

_Filed: 2026-10-28T15:21:34Z_
_Operation: DeleteSchemaVersion | Class: validation_

> _Note: this issue was identified against the code base at commit `7e4efc65cc687607a3c1322cd06c2fb91d881b32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Schema::version_number` is documented as "The ONE definition of how a request's version label is read, so every reader and the delete resolve it the same way", and it states the rule explicitly: "a version number is the whole decimal value, so anything else — surrounding space, a sign, a decimal point, a word — names no version." Its body is `requested.parse::<u64>().ok()`, and Rust's `u64: FromStr` ACCEPTS an optional leading `+`, so `"+1".parse::<u64>() == Ok(1)`.

The label therefore does not behave as the rule under it says. `SchemaVersion` is an unconstrained `__string` URI label on DeleteSchemaVersion (OPERATIONS.md:78), so `DELETE /v1/registries/name/{r}/schemas/name/{s}/version/+1` (or the unambiguous percent-encoded `/version/%2B1`) reaches `remove_version_named("+1")`, which resolves it to version 1 and REMOVES that stored definition — along with its code bindings and its idempotency-token entry — where the crate's own rule requires the same `NotFoundException` it answers for `/version/latest`. The handler's comment states the intended behaviour in the same words: "a value that is not a decimal version number matches no stored version and is as absent as a version already deleted".

The same label is read by every version-scoped operation through `version_for`, so `?schemaVersion=+1` on DescribeSchema / ExportSchema / DescribeCodeBinding / PutCodeBinding also resolves version 1 rather than refusing. The delete is the consequential one: a label the crate says names nothing destroys stored state.

## Evidence

The stated rule and the code that breaks it — services/service-com-amazonaws-schemas-schemas/src/state.rs:1388-1396:
```
    /// The version number a `SchemaVersion` label names, if it names one at all.
    ///
    /// The ONE definition of how a request's version label is read, so every reader
    /// and the delete resolve it the same way: a version number is the whole decimal
    /// value, so anything else — surrounding space, a sign, a decimal point, a word —
    /// names no version.
    fn version_number(requested: &str) -> Option<u64> {
        requested.parse::<u64>().ok()
    }
```
`u64::from_str` accepts a leading `+`, so the one input the comment names ("a sign") is the one the body admits: `"+1"` yields `Some(1)`.

The delete path that then mutates state — state.rs:1433-1438:
```
    pub fn remove_version_named(&mut self, requested: &str, now: SystemTime) -> bool {
        Self::version_number(requested).is_some_and(|version| self.remove_version(version, now))
    }
```
reached from services/service-com-amazonaws-schemas-schemas/src/ops/delete_schema_version.rs:219-242, whose comment repeats the rule it relies on:
```
        // `SchemaVersion` names ONE stored version, resolved by the same state helper
        // every read of a version resolves it through, so a value that is not a
        // decimal version number matches no stored version and is as absent as a
        // version already deleted. Only that version goes — the schema, its metadata
        // and its other versions survive, and the freed number is never minted again.
...
        if !schema.remove_version_named(&input.schema_version, now) {
            return Err(DeleteSchemaVersionErrorModeled::NotFoundException(
                crate::state::schema_version_not_found(
```
and from the read path — state.rs:1377-1385 (`version_for`), whose doc says `None` means "a value that is not a decimal version number at all, which no stored version can match".

The member is an unconstrained string, so the framework refuses nothing before the handler: OPERATIONS.md:78 — "`SchemaVersion` (string, @httpLabel, required)".

Test coverage: the suite pins only the non-numeric and whitespace spellings, never a signed one — tests/all/schema_read_resolution.rs:61 `const UNHELD_VERSIONS: [&str; 2] = ["7", "latest"];`, tests/all/schema_last_modified.rs:264-269 ("a refused DeleteSchemaVersion (not a version number at all)", `version_uri(REGISTRY, SCHEMA, "latest")` -> 404), and tests/all/registry_schema_lifecycle.rs:630-636 (`?schemaVersion=%201` and `?schemaVersion=1%20`). Nothing pins `+1`, so the current behaviour is not pinned with a stated reason.

## Steps to Reproduce

1. POST /v1/registries/name/mine -> 201 (CreateRegistry)
2. POST /v1/registries/name/mine/schemas/name/s with {"Type":"OpenApi3","Content":"{\"openapi\":\"3.0.0\"}"} -> 201, minting version 1
3. GET /v1/registries/name/mine/schemas/name/s/versions -> 200, SchemaVersions holds "1"
4. DELETE /v1/registries/name/mine/schemas/name/s/version/%2B1 (the label "+1")
5. Observe 204 and that version 1 is gone: GET /v1/registries/name/mine/schemas/name/s/versions -> 200 with an empty SchemaVersions, and GET /v1/registries/name/mine/schemas/name/s -> 404. Per state.rs:1390-1393 ("a sign ... names no version") and delete_schema_version.rs:219-223, this label must have been refused with NotFoundException, exactly as /version/latest is (tests/all/schema_last_modified.rs:264-269)
6. Same with a read: GET /v1/registries/name/mine/schemas/name/s?schemaVersion=%2B1 resolves version 1 instead of answering 404
