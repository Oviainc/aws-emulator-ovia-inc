Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 909749a7cea0ac224aeb09b9b62bd72ae5efa8b6

# Create-time tags bypass the per-resource tag rule TagResource enforces, so a store can be created in a state where every later TagResource fails

_Filed: 2026-10-28T18:43:29Z_
_Operation: CreatePolicyStore | Class: validation_

> _Note: this issue was identified against the code base at commit `909749a7cea0ac224aeb09b9b62bd72ae5efa8b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::MAX_TAGS_PER_RESOURCE` is documented as bounding "what a resource ends up holding", explicitly distinguished from `TagMap`'s modelled per-request length of 200. `TagResource` enforces it: it computes the merged map and refuses with `TooManyTagsException` when the resource would end up holding more than 50. `CreatePolicyStore` copies the request's `tags` map into the new store with no such check, so a single create request carrying 51..200 tags persists a policy store holding more tags than the crate's own stated per-resource rule allows. Afterwards every `TagResource` call against that store is refused permanently (the merge is already over the limit), so the resource can never be tagged again even with a single key — one path accepts state the sibling path then treats as irrecoverable.

## Evidence

The rule, stated once in the state module — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/state.rs:103-109:
```
/// Published Verified Permissions quota: *Tags per resource* — 50. The model
/// states this limit twice: `TagResource`'s own documentation ("You can
/// associate as many as 50 tags with a resource") and `TooManyTagsException`'s
/// ("No more tags be added because the limit (50) has been reached"). It is NOT
/// the same bound as `TagMap`'s modelled length (..200), which bounds ONE
/// request's map; this bounds what a resource ends up holding.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;
```
Enforced by the sibling — src/ops/tag_resource.rs:166-183:
```
        let mut merged = store.tags.clone();
        merged.extend(...);
        if merged.len() > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(TagResourceErrorModeled::TooManyTagsException(
                crate::ops::too_many_tags(&input.resource_arn),
            )
```
Skipped by the create — src/ops/create_policy_store.rs:194-199:
```
            tags: input
                .tags
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
```
Model text (`smithy_query(query="doc", shape="TooManyTagsException")`): "No more tags be added because the limit (50) has been reached. To add new tags, use UntagResource to remove existing tags."

Test coverage: tests/all/tagging.rs:26-37 and :339-370 pin the 50-tag rule for TagResource only; tests/all/tagging.rs:167-175 exercises create-time tags with a single key, so no test or comment in the crate justifies the create path skipping the bound.

## Steps to Reproduce

1. CreatePolicyStore with validationSettings.mode = OFF and a `tags` map of 60 distinct keys — the call succeeds (200).
2. ListTagsForResource on the new store's arn: 60 tags are reported, more than MAX_TAGS_PER_RESOURCE (50).
3. TagResource on that arn with a single new key: 400 TooManyTagsException — and no sequence of TagResource calls can ever succeed until keys are removed, although the service itself created the over-limit state.
