Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 442a99274869dbb92025fb75ce375f34a4aa7eac

# UntagResource skips the tag-key bound its sibling TagResource and all four tag-on-create paths apply to the same keys

_Filed: 2026-10-29T12:14:09Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `442a99274869dbb92025fb75ce375f34a4aa7eac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate judges caller-supplied tag content in two shared builders: `crate::state::reserved_tag_key_rejection` (the `aws:` namespace) and `crate::state::tag_bounds_rejection` (the tag count, the tag-key character bound and the tag-value character bound). Every other handler that takes tag keys or tag maps runs BOTH — `TagResource`, `CreateChannelGroup`, `CreateChannel`, `CreateOriginEndpoint` and `CreateHarvestJob`. `UntagResource` runs only the first, so a `TagKeys` entry past `MAX_TAG_KEY_CHARS` is accepted and answered 204 while `TagResource` answers the crate's 400 ValidationException for a key of exactly the same length. The model puts no `@length` on `TagKey`, so the request boundary does not catch it either and nothing in the crate judges it on this path.

The divergence is not explained anywhere: `tag_bounds_rejection`'s own doc says `member_path` "is the wire name of the member the tags arrived in, which differs per operation exactly as it does for [`reserved_tag_key_rejection`]" — i.e. the builder is written to be called from every tagging path, with `UntagResource`'s `tagKeys` as one of them, and `UntagResource` is the one caller that does not.

## Evidence

The sibling that applies both rules — services/service-com-amazonaws-mediapackagev2-mediapackagev2/src/ops/tag_resource.rs:217-226:
```
        if let Some(rejection) =
            crate::state::reserved_tag_key_rejection("tags", input.tags.keys().map(String::as_str))
        {
            return Err(TagResourceErrorModeled::ValidationException(rejection).into());
        }

        let merged = crate::state::merged_tags(&current, &input.tags);
        if let Some(rejection) = crate::state::tag_bounds_rejection("tags", &merged) {
            return Err(TagResourceErrorModeled::ValidationException(rejection).into());
        }
```
and the same pairing in src/ops/create_channel_group.rs:225-236 (`tag_bounds_rejection("tags", &tags)` then `reserved_tag_key_rejection("tags", ...)`) and src/ops/create_origin_endpoint.rs:390-401 / src/ops/create_harvest_job.rs:274-285 (`"Tags"`).

The handler that skips it — src/ops/untag_resource.rs:184-200, which runs the reserved-namespace rule and then goes straight to the write:
```
        if let Some(rejection) = crate::state::reserved_tag_key_rejection(
            "tagKeys",
            input.tag_keys.iter().map(String::as_str),
        ) {
            return Err(UntagResourceErrorModeled::ValidationException(rejection).into());
        }
        let untagged = store.with_tags_mut(&region, &target, at, |tags| {
            for key in &input.tag_keys {
                tags.remove(key);
            }
        });
```
(no `tag_bounds_rejection` call anywhere in the file).

The bound the skipped builder owns — src/state.rs:375-377 and 416-423:
```
/// The most characters a tag KEY may carry — AWS's published tag-key limit of 128
/// characters, which the model's `TagMap` does not express either.
pub const MAX_TAG_KEY_CHARS: usize = 128;
...
        if key.chars().count() > MAX_TAG_KEY_CHARS {
            return refuse(format!(
                "A tag key carries at most {MAX_TAG_KEY_CHARS} characters; one in the \
                 request carries {}",
```
and the doc that names `UntagResource`'s member as one of its callers — src/state.rs:390-392: "`member_path` is the wire name of the member the tags arrived in, which differs per operation exactly as it does for [`reserved_tag_key_rejection`]".

Model: `UntagResourceRequest$TagKeys` targets `TagKeyList` with `httpQuery: "tagKeys"` and no `@length` on the key (`smithy_query(query="shape", shape="UntagResourceRequest,TagMap")`), so the boundary validator enforces nothing here.

Tests: tests/all/reserved_tag_keys.rs covers the `aws:` rule on both operations; no test in tests/ asserts a key-length refusal for UntagResource.

## Steps to Reproduce

1. Boot the emulator. POST /channelGroup with {"ChannelGroupName":"g"} and note the returned Arn.
2. POST /tags/<arn> with {"tags":{"<a 200-character key>":"v"}} (TagResource). Observe 400 ValidationException whose message reads "A tag key carries at most 128 characters; one in the request carries 200".
3. DELETE /tags/<arn>?tagKeys=<the same 200-character key> (UntagResource). Observe 204 — the same key, over the same crate-owned bound, judged by nothing.
4. Compare with DELETE /tags/<arn>?tagKeys=aws:anything, which IS refused 400 by the reserved-namespace rule the same handler does apply.
