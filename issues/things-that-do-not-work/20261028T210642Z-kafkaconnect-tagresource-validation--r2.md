Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 8c3df3565db28ea355f1b3b898224a3d8ff548b1

# Tag keys and values outside the modelled TagKey/TagValue length bounds are accepted and stored

_Filed: 2026-10-28T21:06:42Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `8c3df3565db28ea355f1b3b898224a3d8ff548b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The tag-write seam this crate funnels EVERY tag through (`crate::state::tag_map_breach` / `tag_key_breach` -> `tag_breach`) applies exactly two rules — the reserved `aws:` key prefix and the tag character set — and reads NO length at all. The handler's own comment says the handler is where the rules the model "cannot express" are refused, explicitly listing the lengths as the part the model DOES express (and therefore the part the boundary validator is assumed to enforce). The crate's own test states the opposite: the generated `Tags` schema does not carry the key/value member constraints, so nothing enforces them, and the test calls that "its own issue".

The result is that a tag whose key or value is outside the modelled bounds is accepted with a 2xx and persisted, and `ListTagsForResource` then reports it back. Concretely:

* An EMPTY key (`{"": "v"}`) passes both rules — `is_tag_text("")` is `true` because `.all()` over no characters is vacuously true (state.rs:415-418), and `claims_reserved_tag_prefix("")` is `false` because `key.as_bytes().get(..4)` is `None` (state.rs:407-411) — so it is stored, although `TagKey` is modelled `length min 1`.
* A 129-character key and a 257-character value are likewise stored, although `TagKey` is `length 1..128` and `TagValue` is `length 0..256`.

The same unchecked seam serves the `tags` member of `CreateConnector`, `CreateCustomPlugin` and `CreateWorkerConfiguration`, each of which calls `tag_map_breach` precisely so that "a create cannot store a tag the tagging path would refuse" — so every tag entry point in this service shares the gap, and `UntagResource` will likewise act on an out-of-bounds key. Note this is a DIFFERENT gap from the already-fixed reserved-prefix/character-set issue (20261024T152903Z-kafkaconnect-tagresource-validation.md), whose own evidence section records that the tests cover "keys/values at the modelled length bounds" only.

Fixing it belongs in this crate: `tag_map_breach`/`tag_key_breach` is already the one in-crate place every tag rule is applied, and the crate already compensates in-crate for a vendored-codegen omission of this exact shape (see `src/ops/types/date_time_wire.rs`, which hand-writes member schemas because the codegen never emits `with_timestamp_format`).

## Evidence

Model (the rule and the numbers) — smithy_query(query="shape", shape="Tags,TagKey,TagValue"):
```
"Tags": {"type": "map", "length": {"min": 0, "max": 200},
  "key":   {"target": "TagKey",   "length": {"min": 1, "max": 128}},
  "value": {"target": "TagValue", "length": {"min": 0, "max": 256}}}
"TagKey":   {"type": "string", "length": {"min": 1, "max": 128}}
"TagValue": {"type": "string", "length": {"min": 0, "max": 256}}
```

Side 1 — the handler assumes the lengths are enforced elsewhere, services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/tag_resource.rs:205-207:
```
            // A tag rule the model cannot express (`TagKey`/`TagValue` carry
            // lengths and no pattern), so the handler is where it is refused:
            // the modelled 400, blaming the member that carried the tags.
```
(the same assumption is written into each create, e.g. src/ops/create_connector.rs:209-213: "`TagKey` and `TagValue` carry lengths and no pattern, so the handler is where the rules are applied.")

Side 2 — the crate's own test says nothing enforces them, tests/all/worker_configuration_delete_and_tags.rs:608-611:
```
    // (Refusing a length PAST a bound on a map key or value is the shared
    // schema-validation layer's job, not this handler's, and this emulator's
    // generated `Tags` schema does not carry those member constraints yet; that
    // gap is its own issue and is not asserted here.)
```

The seam that reads no length — src/state.rs:426-441:
```
fn tag_breach<'a>(
    entries: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> Option<TagRuleBreach> {
    let mut found: Option<TagRuleBreach> = None;
    for (key, value) in entries {
        if claims_reserved_tag_prefix(key) {
            return Some(TagRuleBreach::ReservedKeyPrefix);
        }
        if found.is_none()
            && (!is_tag_text(key) || value.is_some_and(|value| !is_tag_text(value)))
        {
            found = Some(TagRuleBreach::OutsideCharacterSet);
        }
    }
    found
}
```
why an empty key slips through both rules — src/state.rs:407-411 and 415-418:
```
fn claims_reserved_tag_prefix(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}
...
fn is_tag_text(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_alphanumeric() || c == ' ' || TAG_PUNCTUATION.contains(c))
}
```
and the write that follows with no further check — src/state.rs:1683-1701 (`attach_tags`): `if let Some(breach) = tag_map_breach(&incoming) { ... }` then `would_hold > MAX_TAGS_PER_RESOURCE` then `for (key, value) in incoming { tags.insert(key, value); }` — the merged-set COUNT is the only size the write looks at.

Call sites that share the gap: src/ops/tag_resource.rs:203 (`state.attach_tags(..)`), src/ops/untag_resource.rs:196 (`state.remove_tags(..)`), src/ops/create_connector.rs:214, src/ops/create_custom_plugin.rs:188, src/ops/create_worker_configuration.rs:191.

Test coverage: tests/all/worker_configuration_delete_and_tags.rs:615-618 exercises only values AT the bounds (`let key_at_max = "k".repeat(128);` / `let value_at_max = "v".repeat(256);`) and the test's name says so (`a_tag_key_or_value_at_a_modelled_bound_round_trips_and_is_removed_by_its_key`); no row anywhere in tests/ passes a key of length 0 or 129, or a value of length 257, so no existing assertion pins the current acceptance.

## Steps to Reproduce

1. Create a worker configuration: POST /v1/worker-configurations with {"name":"wc-tag-len","propertiesFileContent":"QUJD"} and note the returned workerConfigurationArn (a connector or a custom plugin works the same way).
2. POST /v1/tags/{workerConfigurationArn} with body {"tags": {"": "v"}} — the emulator answers 2xx: `tag_map_breach` returns None for an empty key (state.rs:407-418), so `attach_tags` inserts it, although `TagKey` is modelled `length min 1`.
3. GET /v1/tags/{workerConfigurationArn} — the response reports a tag whose key is the empty string.
4. POST /v1/tags/{workerConfigurationArn} with a 129-character key ({"tags": {"kkkk…k": "v"}}) and again with a 257-character value — both are accepted and reported back by GET /v1/tags/{arn}, although TagKey is length 1..128 and TagValue is length 0..256.
5. Repeat through a create instead of the tagging path — POST /v1/worker-configurations with {"name":"wc-tag-len-2","propertiesFileContent":"QUJD","tags":{"":"v"}} — the same out-of-bounds tag is stored, because the create runs the same `crate::state::tag_map_breach` seam (create_worker_configuration.rs:191).
6. DELETE /v1/tags/{arn}?tagKeys=<the 129-character key> — the removal acts on a key the model's TagKey bound forbids, via the same length-blind `tag_key_breach` (state.rs:457).
