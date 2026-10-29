Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2271003192907dc435917d10fce42c387f42c2e4

# Count the SLO's resulting tag keys on create, not the request list's entries

_Filed: 2026-10-29T02:37:07Z_
_Operation: CreateServiceLevelObjective | Class: validation_

> _Note: this issue was identified against the code base at commit `2271003192907dc435917d10fce42c387f42c2e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The "at most 50 tags per SLO" bound is implemented twice in this crate with two different counting rules, and the create path uses the one that disagrees with where the tags actually land.

`CreateServiceLevelObjective` refuses a request by the NUMBER OF ENTRIES in the `Tags` list (`crate::state::tag_count_refusal`), while the only writer of a tag set (`put_tags`) stores the tags in a map keyed by tag key — so a list of 51 entries in which one key occurs twice would store exactly 50 tags, yet the create is refused with HTTP 400 `ValidationException` before anything is written.

The sibling attachment path on the same resource and the same bound counts the resulting SET instead (`tag_count_after_put`, "a key repeated in the request counts once"), so the identical 51-entry list attached to the same SLO through `TagResource` is accepted. The crate's own rule says the two must agree: `max_tags_of`'s doc states "the bound an attachment enforces is the bound the create of that same kind enforces". Today they do not, and the create is the one that refuses a request whose stored result is inside the bound.

Fix direction: count the create's tag set the way the attachment path already counts it (distinct keys, which is what `put_tags` will store), leaving the bound itself and every refusal message unchanged.

## Evidence

The create counts list ENTRIES — `services/.../src/state.rs:784-794`:
```
pub fn tag_count_refusal(tags: Option<&[Tag]>) -> Option<String> {
    let count = tags.map_or(0, <[Tag]>::len);
    (count > MAX_TAGS_PER_RESOURCE).then(|| {
```
but the writer stores a MAP keyed by tag key — `services/.../src/state.rs:358-369`:
```
/// request resolves to the last value given for it, which is what a map of the request's
/// pairs holds.
pub fn put_tags(&mut self, arn: &str, tags: &[Tag]) {
    ...
    for tag in tags { stored.insert(tag.key.clone(), tag.value.clone()); }
```
The sibling path counts the resulting SET — `services/.../src/state.rs:375-387`:
```
/// Counted over the UNION of the stored keys and the request's, because attaching is per
/// key: ... and a key repeated in the request counts once.
pub fn tag_count_after_put(&self, arn: &str, tags: &[Tag]) -> usize {
```
and the crate asserts the two bounds are the same bound — `services/.../src/state.rs:800-802`:
```
/// Which KIND an ARN addresses is [`tagged_resource_type`]'s rule, so the bound an attachment
/// enforces is the bound the create of that same kind enforces — a resource cannot be stored
/// carrying more tags than a later attachment to it would admit.
```
Call sites: `services/.../src/ops/create_service_level_objective.rs:242-246` (`tag_count_refusal`, refusing with `Scaffolded::validation`) against `services/.../src/ops/tag_resource.rs:196-205` (`tag_count_after_put` vs `max_tags_of`).

The modelled rule being enforced is about the tags ASSOCIATED WITH the resource, not the length of the request member — `smithy_query(query="doc", shape="CreateServiceLevelObjectiveInput", member="Tags")`: "A list of key-value pairs to associate with the SLO. You can associate as many as 50 tags with an SLO." `TagList` carries no `@uniqueItems` (`smithy_query(query="shape", shape="TagList")` -> `length 0..200`), so a list repeating a key is input the model admits.

Test coverage: the tag suites cover a repeated key only on the attachment and removal paths (`tests/all/tag_attachment.rs:169` "the same key twice in one request takes the last value given", `tests/all/resource_tags.rs:188` "the same key named twice in one request") and cover a create at exactly 50 distinct tags (`tests/all/list_tags_for_resource.rs:181-184` "an SLO holding the 50 tags its documented maximum admits"). No test pins a create whose list repeats a key, so nothing fixes the entry-count reading in place.

## Steps to Reproduce

1. Build a `Tags` list of 51 entries: 50 distinct keys `key-00`..`key-49`, plus one more entry repeating `key-00` with a different value (50 distinct keys in total).
2. `POST /slo` with `{"Name":"tag-count-slo","SliConfig":{...},"Tags":<that list>}` — expected: HTTP 200, the SLO stored carrying 50 tags (the set `put_tags` would write); actual: HTTP 400 `ValidationException` "1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50", and nothing stored.
3. Now create the same SLO with no `Tags`, then `POST /tag-resource` with `{"ResourceArn":"<the SLO ARN>","Tags":<the same 51-entry list>}` — it succeeds, because `tag_count_after_put` counts 50 distinct keys.
4. `GET /tags?ResourceArn=<the SLO ARN>` reports 50 tags — the same end state the create refused, showing the two paths enforce one documented bound with two different counts.
