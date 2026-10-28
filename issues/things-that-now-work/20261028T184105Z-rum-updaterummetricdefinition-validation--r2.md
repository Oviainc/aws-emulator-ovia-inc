Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ca4f679c1050fde26f4ddfc17b2fc28d3435b25f

# DimensionKeys' modelled key @length and value @pattern are never applied anywhere, although this crate takes responsibility for exactly those two unappliable constraint classes for TagMap

_Filed: 2026-10-28T18:41:05Z_
_Operation: UpdateRumMetricDefinition | Class: validation_

> _Note: this issue was identified against the code base at commit `ca4f679c1050fde26f4ddfc17b2fc28d3435b25f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`MetricDefinitionRequest$DimensionKeys` targets `DimensionKeysMap`, whose KEY shape `DimensionKey` declares `@length(min: 1, max: 280)` and whose VALUE shape `DimensionName` declares `@length(min: 1, max: 255)` plus `@pattern("^(?!:).*[^\s].*")`. Neither the key's `@length` nor the value's `@pattern` is ever applied:

1. The map KEY is not a constrained leaf for the framework's boundary walk at all — this crate itself states that rule, in prose, as the reason it applies `TagKey`'s constraints by hand (state.rs:2002-2006). So a `DimensionKeys` entry keyed by the EMPTY STRING, or by a 500-character field path, is accepted and persisted.
2. The map VALUE's `@pattern` opens with the negative lookahead `(?!:)`, which the framework compiles with the `regex` crate — and a pattern that fails to compile is TREATED AS MATCHING (vendor/smithy-schema-server/src/constraints.rs:421-424). So a dimension name of `":PageId"` or of `" "` (present, non-empty, but blank — exactly what `.*[^\s].*` forbids) passes the boundary and is persisted.

This is the same class of hole the crate already closes for `TagMap`: `crate::state::validation` exists precisely to apply "the modelled constraints this service applies ITSELF — the ones whose `@pattern` the boundary walk cannot compile", and `ConstrainedString` enumerates AppMonitorName, AppMonitorDomain and TagKey. `DimensionName` is a FOURTH look-around pattern in this same model that the enumeration misses, and `DimensionKey` is a second map key whose constraints nothing applies. `grep` for `DimensionName|DimensionKey|ConstrainedString::` over `src/` shows `ConstrainedString` is only ever instantiated for the three enumerated shapes.

Both handlers that accept a `MetricDefinitionRequest` (`UpdateRumMetricDefinition` and `BatchCreateRumMetricDefinitions`) gate it only on `crate::state::metric_definition_refusal`, whose own doc comment asserts the opposite of what happens — "The modelled constraints (every member's length, the namespace's pattern) are refused before a handler runs." The definition is stored by `MetricDefinitionRecord::from_request`, which copies `dimension_keys` verbatim, and `BatchGetRumMetricDefinitions` then reports the out-of-constraint entries.

Filed against `UpdateRumMetricDefinition`, whose `(operation, validation)` pair carries only a FIXED issue about a different rule (the three custom-metric rules, `20261028T055857Z-rum-updaterummetricdefinition-validation`); `(BatchCreateRumMetricDefinitions, validation)` is held by an OPEN issue about yet another rule (the six documented extended-metric DimensionKeys PAIRS, `20261028T164629Z-...-validation--r2`). Neither of those issues is about the map's own modelled `@length`/`@pattern` — in fact the open one explicitly assumes the boundary handles them ("so any non-blank name passes the boundary"), which is what this issue shows is not true. The gap is in the shared judge, so the fix applies to both callers.

## Evidence

MODEL — `smithy_query(query="shape", shape="DimensionKeysMap,DimensionKey,DimensionName")`:
```
"DimensionKeysMap": {"type": "map", "length": {"min": 0, "max": 29},
   "key":   {"target": "DimensionKey",  "length": {"min": 1, "max": 280}},
   "value": {"target": "DimensionName", "length": {"min": 1, "max": 255},
             "pattern": "^(?!:).*[^\\s].*"}}
```

FRAMEWORK — map keys are never validated, vendor/smithy-schema-server/src/constraints.rs:569-570:
```
/// no codec reads it, so serialization is unaffected. Map *keys* are not
/// covered (codegen does not capture key constraints yet).
```
and vendor/smithy-schema-server/src/constraints.rs:1090-1092:
```
        // Map key: the codec writes the entry key as a string against the map
        // ...
        // toggle the frame; the key itself is not a constrained leaf.
```
FRAMEWORK — an uncompilable `@pattern` is treated as satisfied, vendor/smithy-schema-server/src/constraints.rs:421-424:
```
/// True if `value` satisfies the Smithy `@pattern` `regex` (a find, not an
/// anchored match). A pattern that fails to compile is treated as matching
/// (we never reject a value because of an unparseable model pattern).
fn pattern_matches(regex: &'static str, value: &str) -> bool {
```

THIS CRATE'S OWN STATEMENT OF BOTH RULES — services/service-com-amazonaws-rum-rum/src/state.rs:2002-2008 (the doc of `tag_key_violation`):
```
/// validator does not validate a map KEY at all — it captures the key as the path segment
/// of the entry's VALUE and moves on (`write_leaf` in
/// `vendor/smithy-schema-server/src/constraints.rs`: "the key itself is not a constrained
/// leaf") — and `TagKey`'s `@pattern` would be unapplied even on a path that did validate
/// it, because it opens with a negative lookahead the validator's engine cannot compile.
/// So the constraints the model declares on a tag key would otherwise never be applied at
/// all.
```
and services/service-com-amazonaws-rum-rum/src/state/validation.rs:13-23, whose enumeration is exhaustive and MISSES `DimensionName`:
```
//! Nothing here invents a constraint. Every check runs a `@length` bound or a `@pattern`
//! the model declares ([`ConstrainedString::constraints`]), for the shapes whose pattern
//! the FRAMEWORK's boundary walk cannot apply at all: it compiles a `@pattern` with the
//! `regex` crate, which has no look-around, and treats a pattern it cannot compile as
//! matching (`pattern_matches` in `vendor/smithy-schema-server/src/constraints.rs`). Three
//! of this service's string shapes are written with look-around, so their patterns would
//! otherwise never be applied to anything:
//!
//! * `AppMonitorDomain` — `(?=…)` / `(?!…)` guards around the host-name grammar;
//! * `AppMonitorName` — a leading `(?!\.)`;
//! * `TagKey` — a leading `(?!aws:)`.
```
The enum that list drives, services/service-com-amazonaws-rum-rum/src/state/validation.rs:194-200 — only three arms, no dimension shape:
```
fn matches_pattern(shape: ConstrainedString, value: &str) -> bool {
    match shape {
        ConstrainedString::AppMonitorName => is_app_monitor_name(value),
        ConstrainedString::AppMonitorDomain => is_app_monitor_domain(value),
        ConstrainedString::TagKey => is_tag_key(value),
    }
}
```

THE CLAIM THE CODE BREAKS — services/service-com-amazonaws-rum-rum/src/state.rs:1495 (doc of the ONE judge both handlers call):
```
/// The modelled constraints (every member's length, the namespace's pattern) are refused
/// before a handler runs. What is left are the rules the model states in PROSE, which is
/// why they live here:
```
`metric_definition_refusal` (state.rs:1522-1589) reads `definition.dimension_keys` only to collect the map's KEYS for the EventPattern cross-check (state.rs:1568-1588) and, for custom metrics, their first-level keys (state.rs:1695-1700). No length and no pattern is ever applied to a key or to a dimension NAME; `grep -n 'DimensionName|DimensionKey\b|ConstrainedString::' src/` matches only validation.rs:196-198, validation.rs:401/406 and state.rs:2018/2073 — all three enumerated shapes, none of them a dimension shape.

WHERE THE VALUE IS STORED UNCHECKED — services/service-com-amazonaws-rum-rum/src/ops/update_rum_metric_definition.rs:194-199 and :276-279:
```
        if let Some(message) = crate::state::metric_definition_refusal(&input.metric_definition) {
            return Err(UpdateRumMetricDefinitionErrorModeled::ValidationException(
                ValidationException { message },
            )
            .into());
        }
...
        *stored = crate::state::MetricDefinitionRecord::from_request(
            &input.metric_definition_id,
            &input.metric_definition,
        );
```
and the sibling caller services/service-com-amazonaws-rum-rum/src/ops/batch_create_rum_metric_definitions.rs:318-326. `MetricDefinitionRecord::from_request` copies the map verbatim (state.rs:525-529), and `to_metric_definition` reports it back (state.rs:578-582).

TEST COVERAGE OF THE GAP: none. `grep -n 'DimensionName|DimensionKey|dimension_keys|DimensionKeys' tests/` shows every row in tests/all/metric_definitions.rs, tests/all/metric_definition_validation.rs and tests/all/custom_metric_event_fields.rs builds its map from `EXTENDED_DIMENSION_KEYS` or from well-formed literals (e.g. metric_definition_validation.rs:236 `"DimensionKeys": dimension_keys(&EXTENDED_DIMENSION_KEYS[0..1])`, custom_metric_event_fields.rs:279 `"DimensionKeys": {"browser.name": "BrowserName"}`). No test sends an empty key, an over-length key, a colon-leading name or a blank name, so nothing pins the current acceptance with a stated reason.

## Steps to Reproduce

1. CreateAppMonitor(Name="m", Domain="example.com")
2. PutRumMetricsDestination(AppMonitorName="m", Destination="CloudWatch")
3. BatchCreateRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch", MetricDefinitions=[{"Name":"JsErrorCount","EventPattern":"{\"event_type\":[\"com.amazon.rum.js_error_event\"],\"metadata\":{\"browserName\":[\"Chrome\"]}}","DimensionKeys":{"metadata.browserName":" "}}]) — note the dimension NAME is a single space. Observed: HTTP 200, Errors=[] and a minted MetricDefinitionId. Expected: a refusal, because DimensionName declares @pattern "^(?!:).*[^\\s].*", which a blank value does not satisfy.
4. Repeat step 3 with DimensionKeys={"metadata.browserName":":PageId"} (a colon-leading name, excluded by the pattern's (?!:) guard). Observed: accepted and stored.
5. Repeat step 3 with DimensionKeys={"":"BrowserName"} (an empty map KEY). Observed: accepted and stored, although DimensionKey declares @length(min: 1). The same holds for a key longer than 280 characters, past @length(max: 280).
6. UpdateRumMetricDefinition(AppMonitorName="m", Destination="CloudWatch", MetricDefinitionId=<id from step 3>, MetricDefinition=<any of the three definitions above>). Observed: HTTP 200 and the stored definition replaced. Expected: the modelled ValidationException.
7. BatchGetRumMetricDefinitions(AppMonitorName="m", Destination="CloudWatch") reports the out-of-constraint DimensionKeys entries, so the unapplied constraints are observable from a later read.
8. For contrast, TagResource(ResourceArn=<m's ARN>, Tags={"aws:x":"v"}) IS refused 400, because `tag_keys_refusal` applies `TagKey`'s key constraints by hand for exactly the two framework limitations above.
