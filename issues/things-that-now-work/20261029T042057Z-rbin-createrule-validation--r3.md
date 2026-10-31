Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: e64a39f23852967f0a300826d48f789726fbc966

# Reserved `aws:` namespace is enforced on tag KEYS only, while this crate's own rule for it covers keys and values

_Filed: 2026-10-29T04:20:57Z_
_Operation: CreateRule | Class: validation_

> _Note: this issue was identified against the code base at commit `e64a39f23852967f0a300826d48f789726fbc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` owns this service's reserved-tag-namespace rule, and the doc attached to the constant states it as covering tag keys AND tag values: "the `aws:` prefix is reserved for AWS use, and tag keys and values carrying it can be neither created nor edited nor deleted through a tagging API" (src/state.rs:226-228). The same doc explains WHY the handlers must enforce it themselves — the model cannot express it, because the member's pattern "admits a colon anywhere" — and then enumerates the doors held to it: `CreateRule$Tags`, `TagResource$Tags` and `UntagResource$TagKeys`.

The implementation, however, is key-only. `is_reserved_tag_key` takes a single `key` (src/state.rs:246), and both tag-writing doors feed it only the key half of each `Tag`: `create_rule.rs:218` maps `|tag| tag.key.as_str()`, `tag_resource.rs:153` maps `|tag| tag.key.as_str()`. The VALUE half is never examined, and `Tag$Value`'s pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` admits a colon anywhere exactly as `Tag$Key`'s does, so the boundary validation gate in `src/service.rs` passes an `aws:`-prefixed value straight through to the handler — the very reason the crate gives for checking the key in the handler.

Result: `CreateRule` with `Tags=[{"Key":"env","Value":"aws:managed"}]` answers HTTP 201 and PERSISTS a tag value in the namespace the crate says a caller may neither create nor edit, and `TagResource` does the same with HTTP 201 — while the identical request naming `aws:` on the key side is refused HTTP 400 by the very next line of the same handler. The crate explicitly carves out the members it does NOT hold to the rule and says why ("A rule's resource selection (`ResourceTags`, `ExcludeResourceTags`) is NOT held to it: those members select the resources a rule retains by tags those resources already carry", src/state.rs:233-236) — no such carve-out is stated for tag values, so the value half is an unenforced half of a rule the module claims to own in one place.

## Evidence

The rule as this crate states it, keys AND values — `services/service-com-amazonaws-rbin-amazonrecyclebin/src/state.rs:222-237`:
```
/// The tag-key prefix AWS reserves for its own tags: a caller may not create, change or
/// remove a tag whose key claims it, and a tagging API refuses such a request with its
/// parameter-fault error.
///
/// Source: AWS General Reference, "Tag naming limits and requirements" — the `aws:`
/// prefix is reserved for AWS use, and tag keys and values carrying it can be neither
/// created nor edited nor deleted through a tagging API.
///
/// The model cannot express the rule — `TagKey`'s pattern admits a colon anywhere — so
/// the boundary validator passes such a key through and every door a caller names a tag
/// key of its OWN through is held to it here: `CreateRule$Tags`, `TagResource$Tags` and
/// `UntagResource$TagKeys`. A rule's resource selection (`ResourceTags`,
/// `ExcludeResourceTags`) is NOT held to it: those members select the resources a rule
/// retains by tags those resources already carry, which creates, changes and deletes
/// nothing.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";
```

The check it hands to the handlers examines a key only — `src/state.rs:246-257`:
```
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.as_bytes()
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX.as_bytes()))
}
...
pub fn names_reserved_tag_key<'a>(keys: impl IntoIterator<Item = &'a str>) -> bool {
    keys.into_iter().any(is_reserved_tag_key)
}
```

Both writing doors pass only the key half — `src/ops/create_rule.rs:217-219`:
```
        let reserved_tag_key = crate::state::names_reserved_tag_key(
            input.tags.iter().flatten().map(|tag| tag.key.as_str()),
        );
```
and `src/ops/tag_resource.rs:153`:
```
        if crate::state::names_reserved_tag_key(input.tags.iter().map(|tag| tag.key.as_str())) {
```

Why the gate cannot catch it either — `smithy_query(query="shape", shape="Tag")`: `"Value": {"target": "TagValue", "required": true, "length": {"min": 0, "max": 256}, "pattern": "^([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)$"}` — the value pattern admits a colon anywhere, exactly as `Key`'s does, which is the premise src/state.rs:230-232 uses to justify checking in the handler at all.

Test coverage: `tests/all/reserved_tag_keys.rs:6-9` states the suite's rule as key-only ("a request that would create, change or remove a tag whose key claims the `aws:` namespace") and every row of both tables varies the KEY (`reserved_keys()` at lines 69-89, `ordinary_keys()` at lines 97-115), with values always ordinary literals (e.g. `pairs(&[(&key, "changed")])` at line 345). A repo-wide grep for `aws:` in this crate returns only key rows, ARN literals and the state doc — no test anywhere exercises a tag VALUE in the reserved namespace, so nothing pins the current acceptance.

## Steps to Reproduce

1. POST /rules signed for us-east-1 with {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"},"Tags":[{"Key":"aws:team","Value":"ops"}]} — HTTP 400 ValidationException, "Value at 'tags' failed to satisfy constraint: Member must not contain a tag key beginning with aws:, which is reserved for AWS use" (src/ops/create_rule.rs:220-225, src/state.rs:272-280). This is the enforced half.
2. Repeat with the prefix moved to the VALUE: {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"},"Tags":[{"Key":"env","Value":"aws:managed"}]} — HTTP 201, and the response reports the tag. src/ops/create_rule.rs:218 passed only `tag.key` to the check, so nothing looked at the value.
3. GET /tags/{RuleArn} (percent-encoded) — HTTP 200 with Tags=[{"Key":"env","Value":"aws:managed"}]: the reserved-namespace value is persisted, not just echoed.
4. POST /tags/{RuleArn} with {"Tags":[{"Key":"owner","Value":"aws:billing"}]} — HTTP 201 (src/ops/tag_resource.rs:153 checks keys only), so the second tag-writing door edits in a reserved-namespace value too.
5. Compare the two outcomes against src/state.rs:226-228, which states that `aws:`-carrying tag keys AND VALUES "can be neither created nor edited nor deleted through a tagging API", and against src/state.rs:233-236, where the module explicitly names the members it does NOT hold to the rule and why — tag values are not among them.
