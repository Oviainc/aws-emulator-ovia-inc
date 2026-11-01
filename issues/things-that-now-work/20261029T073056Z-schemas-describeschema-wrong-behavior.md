Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ff66eaaa221f5d1d5d406f24a736c9617410f815

# A present-but-empty schemaVersion is answered 404 "has no version ." instead of resolving the latest stored version, the one the same request without the parameter resolves

_Filed: 2026-10-29T07:30:56Z_
_Operation: DescribeSchema | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ff66eaaa221f5d1d5d406f24a736c9617410f815`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Four operations take `SchemaVersion` as a `@httpQuery(schemaVersion)` member documented "Specifying this limits the results to only this schema version." — DescribeSchema, ExportSchema, DescribeCodeBinding and PutCodeBinding. Each passes `input.schema_version.as_deref()` straight into the one read resolution (`crate::state::ComAmazonawsSchemasSchemasState::readable_schema_version` / `schema_at_version_mut`), which resolves the label through `Schema::version_for` -> `Schema::version_number`.

A request that carries the query parameter with an EMPTY value (`?schemaVersion=`) deserializes to `Some("")`, not `None`, so `version_number` returns `None` on its explicit `requested.is_empty()` guard, the resolution answers `NoSuchSchemaVersion`, and the handler maps that to 404 `NotFoundException` with the degenerate sentence "Schema <s> in registry <r> has no version ." (the empty label interpolated into `schema_version_not_found`). The same request WITHOUT the parameter is served 200 from the latest stored version.

This crate states the opposite rule for a selecting query member sent empty, for the identically-formed documentation sentence: `SearchSchemas`'s `keywords_of` doc reads "specifying this limits the results" as meaning that a value which specifies nothing limits nothing, and the handlers for `registryNamePrefix`, `schemaNamePrefix`, `discovererIdPrefix` and `sourceArnPrefix` all treat an empty value as narrowing nothing. `DescribeSchema`'s own comment quotes the very same sentence form for `SchemaVersion` ("`SchemaVersion` \"limits the results to only this schema version\"; absent, the latest stored version answers"), yet an empty value — which limits to no version at all — is turned into a missing-resource refusal rather than into "no limit", so the one crate reads one doc sentence form two ways.

The refusal is also not one of the spellings the crate deliberately rejects: `version_number`'s doc and the suite that pins it enumerate different SPELLINGS of a number (a sign, zero padding, surrounding space, a decimal point or exponent, a radix prefix, a non-ASCII digit, a word, a too-wide number). An empty value is not a spelling of any version; it names no version because it names nothing.

## Evidence

Model (smithy_query query="shape" shape="DescribeSchemaRequest"; identical text on ExportSchemaRequest$SchemaVersion, DescribeCodeBindingRequest$SchemaVersion and PutCodeBindingRequest$SchemaVersion):
  "SchemaVersion": {"target": "__string", "httpQuery": "schemaVersion", "doc": "<p>Specifying this limits the results to only this schema version.</p>"}

Handler — services/service-com-amazonaws-schemas-schemas/src/ops/describe_schema.rs:171-192:
        // The one resolution every read that names a schema version makes: the
        // registry, the schema in it and the version the request selects, each step
        // answering its own refusal. See `crate::state::readable_schema_version`.
        //
        // `SchemaVersion` "limits the results to only this schema version"; absent, the
        // latest stored version answers. A version this schema does not hold is as
        // absent as a schema that was never created.
        ... = state
            .readable_schema_version(
                &input.registry_name,
                &input.schema_name,
                input.schema_version.as_deref(),
            )
            .map_err(|refusal| {
                DescribeSchemaError::from(DescribeSchemaErrorModeled::NotFoundException(
                    refusal.not_found(),
                ))
            })?;
The same call with the same mapping: export_schema.rs:200-210, describe_code_binding.rs:197-207, put_code_binding.rs:204-214 (`schema_at_version_mut`).

Shared resolution — services/service-com-amazonaws-schemas-schemas/src/state.rs:1396-1404:
    pub fn version_for(&self, requested: Option<&str>) -> Option<&SchemaVersion> {
        match requested {
            None => self.versions.values().next_back(),
            Some(raw) => self.versions.get(&Self::version_number(raw)?),
        }
    }
and state.rs:1415-1420, which is where `Some("")` dies:
    fn version_number(requested: &str) -> Option<u64> {
        // Screen the spelling before handing the digits to `u64: FromStr`, which on its
        // own accepts a leading `+` and so would let a signed label name a version.
        if requested.is_empty() || !requested.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
The sentence that then goes on the wire — state.rs:3911-3915:
    pub fn schema_version_not_found(registry: &str, schema: &str, requested: Option<&str>) -> NotFoundException {
        match requested {
            Some(version) => not_found(format!(
                "Schema {schema} in registry {registry} has no version {version}."
            )),
with `requested = Some("")`, i.e. "... has no version .".

A present-but-empty query parameter arrives as `Some("")`, never `None` — vendor/smithy-schema-server/src/http_binding.rs:104-110:
            for pair in query_str.split('&') {
                let mut it = pair.splitn(2, '=');
                let k = percent_decode_query(it.next().unwrap_or(""));
                let v = percent_decode_query(it.next().unwrap_or(""));
                query.entry(k.clone()).or_insert_with(|| v.clone());
and :277-292 (`if values.is_empty() { continue; } ... StringValueDeserializer::new(v)`), so a `?schemaVersion=` yields one value, the empty string.

Same-crate contradiction — services/service-com-amazonaws-schemas-schemas/src/ops/search_schemas.rs:303-310:
/// The keywords one `Keywords` value carries, folded to lower case.
///
/// `Keywords` is a single `__string`, but the member is plural — "limits the results
/// to only schemas that include the provided keywords" — so one value carries a LIST
/// of keywords, separated by commas or whitespace; empty pieces are dropped. A value
/// that carries no keyword at all (empty, or separators only) therefore limits
/// nothing, which is all "specifying this limits the results" can mean for a value
/// that specifies none.
and the same treatment of an empty selecting value in list_registries.rs:216-219 (`input.registry_name_prefix.as_deref().is_none_or(|prefix| name.starts_with(prefix))`) and list_schemas.rs:219-226.

Test coverage: tests/all/schema_version_label_spellings.rs:57-75 pins 14 labels that name no version ("+1", "01", " 1", "1.0", "0x1", "18446744073709551616", …) across DescribeSchema/ExportSchema/PutCodeBinding/DescribeCodeBinding/DeleteSchemaVersion, and its RULE at :274-278 enumerates spelling classes ("a sign, zero padding, surrounding space, a decimal point or exponent, a radix prefix, a non-ASCII digit, a word, a number too wide to be a version"). The EMPTY value is not among the rows and not among the classes, so nothing pins the current 404.

## Steps to Reproduce

1. Start the emulator. POST /v1/registries/name/mine -> 201 (CreateRegistry).
2. POST /v1/registries/name/mine/schemas/name/order.placed with {"Type":"OpenApi3","Content":"{\"openapi\":\"3.0.0\",\"info\":{\"title\":\"t\",\"version\":\"1.0.0\"},\"paths\":{}}"} -> 201, minting version 1.
3. GET /v1/registries/name/mine/schemas/name/order.placed -> 200 with SchemaVersion "1" (no schemaVersion parameter: the latest stored version answers).
4. GET /v1/registries/name/mine/schemas/name/order.placed?schemaVersion= (the parameter present with an empty value) -> 404 NotFoundException, Message "Schema order.placed in registry mine has no version ."
5. Expected: the empty value specifies no version, so it limits nothing and the latest stored version answers 200 — the crate's own reading of the same documentation sentence form (search_schemas.rs:303-310).
6. The same empty value is refused identically on the three sibling operations that take the member: GET .../order.placed/export?type=JSONSchemaDraft4&schemaVersion=, POST .../order.placed/language/Java8?schemaVersion= and GET .../order.placed/language/Java8?schemaVersion=, all through crate::state::Schema::version_number.
