Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: e0e9a90161aff7ec754055c046e25b8f716c6f2d

# An empty tag key is accepted (and silently removes nothing) instead of being refused as a tag-restriction violation

_Filed: 2026-10-24T21:15:38Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `e0e9a90161aff7ec754055c046e25b8f716c6f2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AWS's universal resource-tagging restrictions bound a tag key at 1-128 Unicode characters: the empty string is not a valid tag key, and naming it is a parameter violation, which this service answers with HTTP 422 `UnprocessableEntityException` attributed to the member the key arrived on.

`crate::state::tag_key_violations` — the one place a tag key is judged, shared by `UntagResource` (`check_tag_keys("tagKeys", ..)`) and by tag-on-create (`check_tags`) — checks only the MAXIMUM length (128 chars) and the reserved `aws:` prefix. It has no minimum, so an empty key passes.

Expected: `DELETE /tags/{ResourceArn}?tagKeys=` (or any request naming "" among its tag keys) is refused with 422 `UnprocessableEntityException` and `errors: {"tagKeys": [..]}`, leaving the stored tags untouched.
Actual: `src/ops/untag_resource.rs` runs `check_tag_keys`, which finds no violation, then calls `state.remove_tag_keys(..)` which removes nothing, and answers 204 No Content — an honest-looking success for a request AWS rejects. The same hole lets a create store a tag whose key is the empty string (`check_tags` -> `tag_key_violations`), which `ListTagsForResource` then reports back.

## Evidence

Spec/grounding: AWS resource-tagging restrictions (the same source the crate cites for MAX_TAG_KEY_CHARS=128, MAX_TAG_VALUE_CHARS=256, MAX_TAGS=50 and the reserved `aws:` prefix in src/state.rs lines ~126-147) state a minimum key length of 1 Unicode character; `UnprocessableEntityException` (errorKind client, httpError 422) is the refusal this service uses for member violations, with the per-attribute `errors` map (src/scaffolded.rs).
Handler: src/state.rs `tag_key_violations()` (lines ~978-995) emits a phrase only when `length > MAX_TAG_KEY_CHARS` or the key starts with `aws:`; src/ops/untag_resource.rs `handle()` calls `check_tag_keys("tagKeys", &input.tag_keys)` and then `state.remove_tag_keys(..)`, so an empty key reaches the store and no-ops.
Tests: tests/all/tag_restrictions.rs covers oversized keys/values, too many tags and `aws:`-prefixed keys (including on `tagKeys`), and tests/all/resource_tag_removal.rs covers keys the resource does not carry; a grep for an empty-string tag key across tests/ finds none, so no test asserts this rejection.

## Steps to Reproduce

1. Boot the emulator and POST /packaging_groups {"id":"g","tags":{"env":"prod"}}; note the returned arn.
2. DELETE /tags/{arn}?tagKeys= (an empty tag key): observe HTTP 204 and, via GET /tags/{arn}, the tag map unchanged — where AWS refuses the request with 422 UnprocessableEntityException and errors keyed on `tagKeys`.
3. DELETE /tags/{arn}?tagKeys=&tagKeys=env: observe the empty key is accepted alongside the real one instead of the whole request being refused.
4. POST /packaging_groups {"id":"g2","tags":{"":"v"}}: observe HTTP 200 and GET /tags/{arn-of-g2} reporting a tag whose key is the empty string.
