Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# TagResource accepts reserved aws:-prefixed tag keys instead of refusing them

_Filed: 2026-10-24T05:11:59Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AWS reserves the `aws:` tag-key prefix for system tags on every taggable resource: a `TagResource` (or tag-on-create) request carrying a key beginning with `aws:` is refused — the caller cannot create, modify or delete such a tag. Real MediaPackage v2 rejects the request with a client error (HTTP 400) and writes nothing.

The emulator's one tag-validation gate, `crate::state::tag_bounds_rejection`, checks only the tag COUNT (<= 50), key length (<= 128) and value length (<= 256). A key such as `aws:cloudformation:stack-name` is accepted and stored, so a later `ListTagsForResource` / `GetChannelGroup` reports a system tag the caller was never allowed to set, and `UntagResource` lets a caller delete one. The same gate is reused by tag-on-create (`CreateChannelGroup`, `CreateChannel`, `CreateOriginEndpoint`, `CreateHarvestJob`), so all of them accept reserved keys too.

## Evidence

Spec/well-established behavior: AWS tagging rules — "Do not use the `aws:` prefix in your tag names or values because it is reserved for AWS use. You can't edit or delete tag names or values with this prefix" — applied uniformly by taggable services; `TagResource` models `ValidationException` (HTTP 400) as its only error, which is the shape such a refusal takes here.

Handler/state: `src/state.rs:303-345` — `tag_bounds_rejection` validates only `MAX_TAGS_PER_RESOURCE`, `MAX_TAG_KEY_CHARS` and `MAX_TAG_VALUE_CHARS`; no prefix check anywhere in the crate (no occurrence of `"aws:"` in `src/`). Call sites: `src/ops/tag_resource.rs:209`, `create_channel_group.rs:225`, `create_channel.rs:364`, `create_origin_endpoint.rs:350`, `create_harvest_job.rs:229`.

Tests: `tests/all/resource_tags.rs` / `resource_tagging.rs` exercise the count/length bounds; no test submits an `aws:`-prefixed key, so nothing asserts the current permissive behavior. The same class was filed for a sibling service (20261024T010403Z-mediapackage-vod-createpackaginggroup-validation.md).

## Steps to Reproduce

1. Create a channel group `tg` and note its ARN.
2. POST `/tags/{url-encoded group ARN}` with `{"tags": {"aws:cloudformation:stack-name": "mine"}}`. Expected: HTTP 400 ValidationException and no tag written. Actual: HTTP 204.
3. GET `/tags/{url-encoded group ARN}`. Actual: the reserved `aws:` key is reported as a user tag.
4. Also POST `/channelGroup` with `Tags: {"aws:foo": "bar"}`. Expected: HTTP 400. Actual: the group is created carrying the reserved key.
