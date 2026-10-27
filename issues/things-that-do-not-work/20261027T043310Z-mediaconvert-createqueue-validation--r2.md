Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f0793968580971e18796aad3fd0f4b67dc633a3c

# Create operations store Tags with neither the empty-key rule nor the per-resource tag cap TagResource enforces

_Filed: 2026-10-27T04:33:10Z_
_Operation: CreateQueue | Class: validation_

> _Note: this issue was identified against the code base at commit `f0793968580971e18796aad3fd0f4b67dc633a3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `Create*` handler in this crate writes the request's `Tags` straight onto the record it stores, with no check at all, while `TagResource` — the other door onto the very same tag map — enforces two rules before it writes: a tag key that carries no characters is refused with `BadRequestException`, and a merge that would carry the record past `state::MAX_TAGS_PER_RESOURCE` is refused with `ServiceQuotaExceededException`.

So the two doors onto one tag set disagree:

* `CreateQueue` (and `CreatePreset`, `CreateJobTemplate`, `CreateJob`) with `tags: {"": "v"}` stores a tag whose key names nothing — a tag `TagResource` refuses on the same resource, and one `UntagResource` can never name;
* `CreateQueue` with 500 tags stores 500 tags on the record, past the cap the state module publishes and declares the record's bound. The next `TagResource` on that resource then answers 402 for every single key, because the merge is already over the cap.

The state module's own invariant section states the record-level bound as a fact about the record, not about one operation (`state.rs:35`), and the crate's test `resource_tagging.rs:437` pins that the tags a create carried ARE the stored set the tagging operations work on — so the create doors are writing into the capped map without applying the cap. The fix is to apply the rules `TagResource` already applies (the empty-key refusal and the `MAX_TAGS_PER_RESOURCE` ceiling) on the create paths too, from one place, so the two doors cannot disagree.

(This is a different defect from the fixed `CreateQueue` `ConcurrentJobs` validation issue: it concerns the `Tags` member on all four create paths.)

## Evidence

The rules, enforced on ONE door only — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/tag_resource.rs:179`:
```rust
        // "You can tag resources with a key-value pair or with only a key" — so an empty
        // VALUE is a tag, but a tag with no key names nothing.
        if input.tags.keys().any(|key| key.is_empty()) {
            return Err(TagResourceErrorModeled::BadRequestException(BadRequestException {
                message: Some("Tags must not contain an empty tag key.".to_string()),
```
and `tag_resource.rs:207`:
```rust
        if merged.len() > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(TagResourceErrorModeled::ServiceQuotaExceededException(
```

The other doors onto the same map, with no check — `src/ops/create_queue.rs:303`:
```rust
            tags: input.tags.unwrap_or_default().into_iter().collect(),
```
identically `src/ops/create_preset.rs:209`, `src/ops/create_job_template.rs:307`, `src/ops/create_job.rs:539`.

The invariant the crate states for the record (not for one operation) — `src/state.rs:35`:
```text
//! * A record's `tags` are capped at [`MAX_TAGS_PER_RESOURCE`], so `TagResource` cannot
//!   grow one record without limit either; past the cap it answers the same quota error.
```
and `src/state.rs:1287`: "A CHOSEN capacity bound, as [`MAX_QUEUES`] is ... it exists so `TagResource` cannot grow a record without limit." The module header one line above states the whole premise: "Nothing here grows without a ceiling" (`src/state.rs:19`).

The create doors write into exactly the map the tagging operations read — `src/state.rs:4403` (`tags`) and `src/state.rs:4415` (`tags_mut`) resolve `TaggedResource::Queue/Job/Preset/JobTemplate` to `record.tags`.

Test coverage: `tests/all/resource_tagging.rs:704` (`tagging_past_the_cap_is_a_quota_refusal_that_changes_nothing`) and `tests/all/resource_tagging.rs:676` (the empty-tag-key 400) pin both rules for `TagResource`; `tests/all/resource_tagging.rs:437` (`tags_from_the_create_call_are_the_set_the_tagging_operations_see`) pins that a create's tags are the same stored set. No test pins the create paths accepting an empty key or an over-cap map.

## Steps to Reproduce

1. POST /2017-08-29/queues with {"name":"q-empty-key","tags":{"":"v"}} — observe HTTP 201, the queue is stored.
2. GET /2017-08-29/tags/arn:aws:mediaconvert:<region>:<account>:queues/q-empty-key — the reported tag set carries the empty key.
3. POST /2017-08-29/tags with {"arn":"<that arn>","tags":{"":"v"}} — observe HTTP 400 BadRequestException "Tags must not contain an empty tag key." for the very tag the create stored.
4. POST /2017-08-29/queues with {"name":"q-over-cap","tags":{ ... 60 distinct keys ... }} (state::MAX_TAGS_PER_RESOURCE is 50) — observe HTTP 201 and ListTagsForResource reporting all 60.
5. POST /2017-08-29/tags with {"arn":"<q-over-cap arn>","tags":{"one-more":"x"}} — observe HTTP 402 ServiceQuotaExceededException, i.e. the record is already past the cap the state module declares for it.
