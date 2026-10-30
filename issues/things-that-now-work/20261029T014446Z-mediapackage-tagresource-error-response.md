Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 1211f7611505016a47ab1bf8932fdc778b7fd399

# TagResource judges the tag-content rules only after resolving the ARN, so a request UntagResource and both creates answer 422 is answered 404

_Filed: 2026-10-29T01:44:46Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `1211f7611505016a47ab1bf8932fdc778b7fd399`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Four operations of this crate name a caller-supplied tag: `CreateChannel`, `CreateOriginEndpoint`, `TagResource` and `UntagResource`. Three of them judge what the tags SAY (the character set and the reserved `aws:` key prefix) before they touch the store, so a malformed tag is refused with this service's 422 `UnprocessableEntityException` keyed by the member the request carried. `TagResource` is the only one that resolves the `ResourceArn` against the store FIRST and judges the tags afterwards, so the same malformed tag map sent against an ARN this Region holds no resource for is answered 404 `NotFoundException` about the resource instead of 422 about the member.

Nothing in the request's tag content depends on the stored record — `TagResource`'s own comment says so ("Judging the MERGED map is judging the request's own tags: every key already stored passed this very check on the way in") — so the per-entry content half of `bound_tag_entries` could be applied before the lookup exactly as `UntagResource` applies `validate_tag_key` before its lookup. The result is that one tag rule produces two different statuses depending on which door the caller used, and a client that branches on 422-vs-404 to tell "fix your tags" from "no such resource" is told the wrong thing by `TagResource` alone.

## Evidence

Both sides are in this crate.

TagResource resolves the resource first, and only then judges the tags:
- `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/tag_resource.rs:193-198`:
```
        let mut state = state.lock();
        let Some(tags) = state.tags_mut(&region, &input.resource_arn, &target) else {
            return Err(
                crate::state::resource_not_found(target.kind(), target.id()).into(),
            );
        };
```
- `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/tag_resource.rs:215`:
```
        crate::state::bound_tag_entries(merged.iter().map(|(k, v)| (k.as_str(), v.as_str())))?;
```

UntagResource judges the keys BEFORE the lookup:
- `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/untag_resource.rs:183-193`:
```
        // Judged BEFORE anything is removed, so a refused call leaves the resource's tag
        // set exactly as it found it. A key outside the documented character set could
        // never have been stored, and a key carrying the reserved `aws:` prefix names a
        // tag AWS owns ...
        for key in &input.tag_keys {
            crate::state::validate_tag_key("tagKeys", key)?;
        }
```
  with the store lookup only afterwards, at `untag_resource.rs:198-203`.

Both creates judge the tags before touching the store:
- `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/create_channel.rs:172`: `crate::state::bound_tags(&input.tags)?;` — ahead of `if state.has_channel(&region, &input.id)` at `create_channel.rs:182`.
- `services/service-com-amazonaws-mediapackage-mediapackage/src/ops/create_origin_endpoint.rs:220`: `crate::state::bound_tags(&input.tags)?;` — ahead of `let mut state = state.lock();` at `create_origin_endpoint.rs:288`.

The crate states this is meant to be one rule:
- `services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs:620-622`: "This is the one place those rules are decided, so a create's request-shaped tag map and a `TagResource`'s merged result cannot be judged differently."
- `services/service-com-amazonaws-mediapackage-mediapackage/src/state.rs:572-575`: "Because the reserved prefix is refused on NAMING the key and not on writing it, `UntagResource` answers the same refusal as a tag map does: a caller may neither create, nor modify, nor delete such a tag."
- The suite's own statement of the rule, `services/service-com-amazonaws-mediapackage-mediapackage/tests/all/reserved_and_malformed_tags.rs:9-12`: "every operation that names a tag — `CreateChannel`, `CreateOriginEndpoint`, `TagResource` and `UntagResource` — refuses such a request with `UnprocessableEntityException` (HTTP 422) keyed by the member the request carried".

Test coverage: `tests/all/reserved_and_malformed_tags.rs::tag_resource_refuses_a_tag_the_caller_may_not_store_and_leaves_the_set_as_it_was` only ever tags an ARN that resolves (it calls `create_with_tags` first, line 256), so no existing test pins the unresolvable-ARN ordering either way.

## Steps to Reproduce

1. Boot the emulator with the MediaPackage service mounted.
2. Do NOT create any channel named `no-such-channel`.
3. POST /tags/arn%3Aaws%3Amediapackage%3Aus-east-1%3A<account>%3Achannels%2Fno-such-channel with body {"tags": {"aws:cloudformation:stack-name": "x"}} — observe 404 NotFoundException keyed by `channel`.
4. DELETE /tags/arn%3Aaws%3Amediapackage%3Aus-east-1%3A<account>%3Achannels%2Fno-such-channel?tagKeys=aws%3Acloudformation%3Astack-name — observe 422 UnprocessableEntityException keyed by `tagKeys`.
5. POST /channels with body {"id": "c1", "tags": {"aws:cloudformation:stack-name": "x"}} — observe 422 keyed by `tags`, confirming the creates refuse the same content before resolving anything.
6. The same reserved key is therefore answered 422 by three tag doors and 404 by TagResource.
