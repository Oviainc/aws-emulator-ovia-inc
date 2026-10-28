Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d3f11f1bc4743e3822b3321dc229aeefed87f2d3

# The create's 50-tag ceiling counts Tags LIST ENTRIES while the attach path counts distinct keys, so a create whose Tags repeats a key is refused for a set that is within the maximum

_Filed: 2026-10-28T19:10:13Z_
_Operation: CreateServiceLevelObjective | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d3f11f1bc4743e3822b3321dc229aeefed87f2d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate enforces one documented tag ceiling (50 tags per SLO) on two paths, and the two count differently.

`CreateServiceLevelObjective` calls `crate::state::tag_count_refusal`, which compares the LENGTH of the request's `Tags` list against `MAX_TAGS_PER_RESOURCE`. `TagResource` calls `crate::state::tag_count_after_put`, which counts the UNION of distinct KEYS, because `put_tags` stores a tag set keyed by key and a key named twice in one request collapses to one stored tag.

Consequence: a create carrying 51 `Tags` entries of which two share a key is refused with HTTP 400 `ValidationException`, even though the tag set it would have stored is exactly 50 — the documented maximum. The identical 51-entry list attached to the same (already created, untagged) SLO through `TagResource` is ACCEPTED and stores those same 50 tags. So the same list is legal through one path of this crate and illegal through the other, for one resource and one quota.

That contradicts the crate's own stated invariant in `max_tags_of`'s doc — "the bound an attachment enforces is the bound the create of that same kind enforces" — and `tag_count_refusal`'s own doc sentence, which says the count is "checked against the whole tag set the new resource will carry" (the SET, which dedupes keys), not against the member's entry count.

## Evidence

The attach path's counting rule, deduplicating keys — `services/service-com-amazonaws-applicationsignals-applicationsignals/src/state.rs:370-385`:

```
/// How many tags the resource stored under `arn` would carry once `tags` were attached by
/// [`Self::put_tags`].
///
/// Counted over the UNION of the stored keys and the request's, because attaching is per
/// key: a request that only re-values keys the resource already carries adds nothing to the
/// count, and a key repeated in the request counts once.
pub fn tag_count_after_put(&self, arn: &str, tags: &[Tag]) -> usize {
    let stored = self.tags.get(arn);
    let added = tags
        .iter()
        .filter(|tag| stored.is_none_or(|stored| !stored.contains_key(&tag.key)))
        .map(|tag| tag.key.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    stored.map_or(0, BTreeMap::len) + added
}
```

The create path's counting rule, which does NOT dedupe — `state.rs:769-792` (doc sentence at :776, code at :783):

```
/// ... checked against the whole tag set the new resource will carry.
...
pub fn tag_count_refusal(tags: Option<&[Tag]>) -> Option<String> {
    let count = tags.map_or(0, <[Tag]>::len);
    (count > MAX_TAGS_PER_RESOURCE).then(|| {
```

The invariant the two are supposed to share — `state.rs:794-807`:

```
/// Which KIND an ARN addresses is [`tagged_resource_type`]'s rule, so the bound an attachment
/// enforces is the bound the create of that same kind enforces — a resource cannot be stored
/// carrying more tags than a later attachment to it would admit.
pub fn max_tags_of(resource_arn: &str) -> usize {
```

The store that makes the two sets equal (one entry per KEY) — `state.rs:360-368`:

```
pub fn put_tags(&mut self, arn: &str, tags: &[Tag]) {
    ...
    for tag in tags {
        stored.insert(tag.key.clone(), tag.value.clone());
    }
}
```

The two call sites: `src/ops/create_service_level_objective.rs:242-246`

```
// The maximum this resource's own `Tags` doc states — "You can associate as many as
// 50 tags with an SLO" — which the modelled `TagList` bound is wider than.
if let Some(message) = crate::state::tag_count_refusal(input.tags.as_deref()) {
    return Err(crate::scaffolded::Scaffolded::validation(message));
}
```

and `src/ops/tag_resource.rs:196-205`

```
let count_after = store.tag_count_after_put(&arn, &input.tags);
let maximum = crate::state::max_tags_of(&arn);
if count_after > maximum {
    return Err(TagResourceErrorModeled::ServiceQuotaExceededException(...
```

Test coverage: `tests/all/slo_indicator_required.rs:333-375` drives the create with 50 / 51 / 200 tags and asserts 400 past 50, but every list it builds carries DISTINCT keys, so the repeated-key case is pinned by no test; `tests/all/tag_attachment.rs:345` pins the attach refusal at `MAX_TAGS_PER_RESOURCE + 1` distinct keys. Both stay green once the create counts the resulting set.

## Steps to Reproduce

1. Start the emulator and point an `application-signals` client at it.
2. `CreateServiceLevelObjective` with `Name=dup-tags`, a valid `SliConfig`, and a `Tags` list of 51 entries whose keys are `k1..k50` plus a SECOND entry for key `k1` (50 distinct keys, 51 entries).
3. Observe HTTP 400 `ValidationException`: "1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50" — although the set that would be stored holds 50 tags, the documented maximum.
4. Now `CreateServiceLevelObjective` with `Name=dup-tags` and NO `Tags` -> 200.
5. `TagResource` on that SLO's ARN with the IDENTICAL 51-entry list -> HTTP 200, and `ListTagsForResource` reports 50 tags.
6. Expected: one counting rule for one quota — both paths count the distinct-key set the resource ends up carrying (as `tag_count_after_put` and `max_tags_of`'s doc state), so the create is accepted exactly where the attach is.
