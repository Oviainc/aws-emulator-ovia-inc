Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# TagResource accepts caller-supplied tag keys with the reserved `aws:` prefix

_Filed: 2026-10-24T07:17:30Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` merges whatever keys the request carries into the resource's tag set. A key beginning with the AWS-reserved `aws:` prefix (e.g. `aws:cloudformation:stack-name`) passes the modelled `TagKey` pattern — which permits `:` — so the emulator stores it and `ListTagsForResource` then reports a user-created `aws:`-prefixed tag; `UntagResource` will likewise happily delete one.

Expected: AWS reserves the `aws:` tag-key prefix for system tags. A tagging call in which the caller supplies such a key is refused with the modelled `ValidationException` (HTTP 400) and no tag is stored; a caller cannot create, overwrite or remove `aws:`-prefixed tags through the public tagging API.

Actual: the prefix is never inspected — the key is stored like any other and is observable through `ListTagsForResource`.

## Evidence

Spec/well-known semantics: the modelled `Tag$Key` is `TagKey` with pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` (smithy_query shape=Tag), which admits `aws:...`; the AWS-wide tagging rule is that the `aws:` prefix is reserved for AWS use and cannot be created, edited or deleted by a caller, and service tagging APIs answer such an attempt with a validation error. `ValidationException` (400) is declared on `TagResource`.

Handler: services/service-com-amazonaws-partnercentralbenefits-partnercentralbenefitsservice/src/ops/tag_resource.rs, `handle` — the only checks are the resource lookup (`taggable_resource_mut`) and the per-resource tag count (`crate::state::merged_tags` vs `MAX_TAGS_PER_RESOURCE`); no key is inspected. `src/ops/untag_resource.rs` removes any named key unconditionally.

Test coverage: none. tests/all/resource_tagging.rs and tests/all/resource_tags.rs cover key/value length refusals (`"k".repeat(129)`), ARN forms and the quota, but no case supplies an `aws:`-prefixed key (grep for `aws:` in tests finds only ARN strings).

## Steps to Reproduce

1. Grant the partner entitlement and create a benefit application; note its Arn.
2. Call TagResource(resourceArn=<that Arn>, tags=[{Key:'aws:createdBy', Value:'mine'}]).
3. Observe HTTP 200. Expected: 400 ValidationException stating that tag keys with the reserved `aws:` prefix cannot be supplied, and no tag stored.
4. Call ListTagsForResource(resourceArn=<that Arn>) and observe the `aws:createdBy` tag reported back.
5. Call UntagResource(resourceArn=<that Arn>, tagKeys=['aws:createdBy']) and observe HTTP 200 with the reserved tag removed.
