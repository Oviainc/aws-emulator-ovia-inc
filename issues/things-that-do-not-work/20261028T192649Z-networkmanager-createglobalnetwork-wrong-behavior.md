Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e8dc968dccb3edde6e1cac399927b1a6964cab15

# Create-time Tags are stored verbatim, so a key named twice in one request is stored twice — against the crate's own "tag keys are unique per resource" rule that TagResource implements

_Filed: 2026-10-28T19:26:49Z_
_Operation: CreateGlobalNetwork | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e8dc968dccb3edde6e1cac399927b1a6964cab15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every create of this crate validates its `Tags` through `check_requested_tags` and then stores the raw list with `tags: input.tags.unwrap_or_default()`. No create normalises the list, so a request naming the same key twice (`[{env,a},{env,b}]`) is stored as TWO entries for one key.

The crate states the opposite rule in two places and implements it on the TagResource path only:
* `validate_tags` — the very check the creates call — documents that a repeated key is accepted BECAUSE "the last value for a key wins (see merge_tags)". On the create path nothing calls `merge_tags`, so the last value does NOT win: both values are stored and the FIRST one is what every later read and every later write sees.
* `tag_count_after_merge`, the function the create's own quota check uses, counts DISTINCT keys. So a create carrying 100 `Tags` entries over 50 distinct keys passes the `MAX_TAGS_PER_RESOURCE` (50) check and stores 100 entries — a resource whose stored tag list is twice the ceiling the same request was graded against.

The damage is not self-healing: `merge_tags` updates only the FIRST entry whose key matches (`existing.iter_mut().find(...)`), so a later `TagResource` for the duplicated key leaves the second copy untouched and the resource then reports the same key with TWO DIFFERENT values. `ListTagsForResource` reports the duplicated key as many times as the create named it. Only `UntagResource` (which uses `retain`) can clear it.

The same defect is in every create that stores tags — CreateGlobalNetwork, CreateSite, CreateLink, CreateDevice, CreateConnection, CreateCoreNetwork, CreateConnectPeer, CreateTransitGatewayPeering and the five attachment creates — all of which carry the identical comment claiming the TagResource rules hold for them.

## Evidence

The rule the crate states, on the function the creates themselves call — src/state.rs:6140-6144:
```
/// Check a tag set a request asks to APPLY (`TagResource`, and the `Tags` of a create).
///
/// Refuses an empty or over-long key, an over-long value, and any key carrying the
/// AWS-reserved prefix. A key repeated inside one request is NOT an error: tag keys are
/// unique per resource, so the last value for a key wins (see [`merge_tags`]).
pub fn validate_tags(tags: &[Tag]) -> Result<(), TagRejection> {
```
and src/state.rs:6209-6220:
```
/// Tag keys are unique per resource, so a key already held keeps its position and takes
/// the requested value and a new key is appended — which is what makes a repeated
/// `TagResource` overwrite rather than duplicate.
pub fn merge_tags(existing: &mut Vec<Tag>, requested: Vec<Tag>) {
    for tag in requested {
        match existing.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value,
```
The path that honours it — src/ops/tag_resource.rs:199:
```
        crate::state::merge_tags(held, input.tags);
```
The create path that claims the same rules and then skips the merge — src/ops/create_global_network.rs:140-143 and :196:
```
        // The same tag rules `TagResource` applies hold for the tags a create carries:
        // the AWS-reserved key prefix, the documented key/value lengths and the
        // per-resource tag quota are all refused before anything is stored.
        if let Err(problem) = crate::state::check_requested_tags(input.tags.as_ref()) {
...
            tags: input.tags.unwrap_or_default(),
```
The quota counter the create is graded by counts DISTINCT keys — src/state.rs:6201-6207:
```
/// How many tags the resource would carry once `requested` is merged into `existing`,
/// for the [`MAX_TAGS_PER_RESOURCE`] quota: a key already held is replaced, not added.
pub fn tag_count_after_merge(existing: &[Tag], requested: &[Tag]) -> usize {
    let mut keys: BTreeSet<&str> = existing.iter().filter_map(|tag| tag.key.as_deref()).collect();
    keys.extend(requested.iter().filter_map(|tag| tag.key.as_deref()));
    keys.len()
```
(reached from `check_requested_tags`, src/state.rs:6059-6066: `let count = tag_count_after_merge(&[], tags); if count > MAX_TAGS_PER_RESOURCE {`).

Test coverage: tests/all/resource_tagging.rs:271-281 pins the rule for the TagResource path only — "A key already held keeps its position and takes the new value; a new key is appended. Tag keys are unique per resource, so `env` must not appear twice." — and every `initial:` tag set in `tag_cases()` uses distinct keys, so no test creates a resource with a repeated key.

## Steps to Reproduce

1. POST /global-networks with Tags = [{"Key":"env","Value":"a"},{"Key":"env","Value":"b"}] → 200
2. ListTagsForResource on the new global network's ARN → the list reports `env` TWICE (values "a" and "b"), although src/state.rs:6143-6144 states "tag keys are unique per resource, so the last value for a key wins"
3. TagResource that ARN with Tags = [{"Key":"env","Value":"c"}] → 200
4. ListTagsForResource again → `env`="c" AND `env`="b": `merge_tags` rewrote only the first entry, so one key now reports two values
5. Separately: POST /global-networks with 100 Tags entries covering 50 distinct keys → accepted (tag_count_after_merge = 50 ≤ MAX_TAGS_PER_RESOURCE) and 100 entries are stored, past the ceiling the request was graded against
