Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: e39df06442d4f2a86a0fa0e3a1f1c22c399555b8

# TagResource accepts tag keys and values prefixed with `aws:`, which the modelled member documentation forbids

_Filed: 2026-10-28T16:01:56Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `e39df06442d4f2a86a0fa0e3a1f1c22c399555b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `tags` member of `TagResource` documents two restrictions per entry: a length bound and a reserved-prefix rule — "It can't be prefixed with `aws:`" — stated for the key and again for the value. The modelled traits enforce only the lengths and the key's character class; `TagKey`'s pattern `^[a-zA-Z+-=._:/]+$` happily matches `aws:owner`, so the boundary validator does not refuse it either.

The handler then applies only `crate::state::bound_tags` (entry count plus key/value character counts) and merges the map into the stored record, so `TagResource` with `{"aws:owner":"finance"}` answers 200 and a later `ListTagsForResource` reports the reserved key back. No helper, comment or test in this crate mentions the prefix rule, so nothing in the crate justifies the divergence: the documented restriction is simply unimplemented on the one path that documents it.

## Evidence

The model states the rule itself — `smithy_query(query="doc",shape="TagResourceInput",member="tags")`:

> "**Key** (string) – A key is the required name of the tag. The string value can be from 1 to 128 Unicode characters in length. **It can't be prefixed with `aws:`** and can only contain the set of Unicode characters specified by this Java regular expression: `"^([\p{L}\p{Z}\p{N}_.:/=+\-]*)$")`."
> "**Value** (string) – A value is the optional value of the tag. The string value can be from 1 to 256 Unicode characters in length. **It can't be prefixed with `aws:`** ..."

The same sentences are carried in the handler's own header, `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/tag_resource.rs:51-59`:
```
//! * `tags` (TagMap, len 0..50) **required** — The tags to be assigned to the Neptune
//!   Analytics resource. ... It can't be prefixed with aws: and can only
//!   contain the set of Unicode characters specified by this Java regular expression:
```

The boundary cannot catch it: `smithy_query(query="shape",shape="TagKey")` -> `{"length": {"min": 1, "max": 128}, "pattern": "^[a-zA-Z+-=._:/]+$"}` (the class contains `:`, so `aws:owner` matches), and `TagValue` carries `@length(0..256)` and no pattern at all.

The handler does nothing about it — `src/ops/tag_resource.rs:151-201`:
```
        // Nothing caller-sized reaches the store unchecked — the per-entry bound on what
        // one record's tags may hold is stated once, in `crate::state`.
        if let Err(message) = crate::state::bound_tags(&input.tags) {
...
        for (key, value) in input.tags {
            tags.insert(key, value);
        }
        Ok(Self::Output {})
```
and the helper it reaches checks only count and lengths — `src/state.rs:3013-3031`:
```
pub fn bound_tags(tags: &HashMap<String, String>) -> Result<(), String> {
    bound_tag_count(tags.len())?;
    for (key, value) in tags {
        let key_chars = key.chars().count();
        if key_chars > MAX_TAG_KEY_CHARS {
...
        let value_chars = value.chars().count();
        if value_chars > MAX_TAG_VALUE_CHARS {
```
with its own doc naming the only constraints the crate considers (`src/state.rs:3005-3012`): "Refuse a `tags` map that would put more into one stored record than it may hold ([`MAX_TAGS`] entries of at most [`MAX_TAG_KEY_CHARS`] / [`MAX_TAG_VALUE_CHARS`] characters). ... The model's own `@length` traits on `TagMap`/`TagKey`/`TagValue` refuse these at the boundary too". A grep of the crate for `aws:` finds no reserved-prefix rule and no test asserting one (`tests/all/resource_tagging.rs` exercises the entry bound, key/value lengths and ARN resolution only).

## Steps to Reproduce

1. CreateGraph with `graphName: "tagged-graph"` and `provisionedMemory: 16`; note the returned `arn`.
2. POST /tags/{arn} (TagResource) with body `{"tags":{"aws:owner":"finance"}}`.
3. Observe HTTP 200 instead of a 400 `ValidationException` for the documented reserved prefix.
4. GET /tags/{arn} (ListTagsForResource) — the response reports `{"aws:owner":"finance"}`, so the reserved key is now stored on the graph record.
5. Repeat with `{"tags":{"owner":"aws:finance"}}` — the value half of the same documented rule is likewise accepted.
