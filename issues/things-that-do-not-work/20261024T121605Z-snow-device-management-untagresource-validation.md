Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 9e66ef5d78d13d37b68af2ed28461695d94c8b52

# UntagResource accepts tag keys AWS refuses (reserved aws: prefix, empty key)

_Filed: 2026-10-24T12:16:05Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `9e66ef5d78d13d37b68af2ed28461695d94c8b52`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AWS reserves the `aws:` tag-key prefix: "You can't edit or delete tag names or values with this prefix", so a DELETE /tags/{resourceArn}?tagKeys=aws:something is refused with ValidationException (HTTP 400), as is an empty tag key. The emulator's UntagResource performs no check at all on `tagKeys`: it resolves the ARN and then calls `tags.remove(key)` for every key, answering HTTP 200 for keys the service would have rejected. The crate's own `tag_violation` encodes the same rules for TagResource, so the refusal class exists but is not applied on the removal path.

## Evidence

Spec: smithy_query(query="operation", operation="UntagResource") — declares ValidationException (HTTP 400); TagKeys carries no model constraint, which is why the AWS-wide tagging rules (cited in src/state.rs: RESERVED_TAG_PREFIX "the rule is service-independent, which is why the model carries no pattern for it") have to be enforced in the handler, as TagResource does via `crate::state::tag_violation`.
Handler: src/ops/untag_resource.rs lines 222-245 — `for key in &input.tag_keys { tags.remove(key); }` with no validation of the keys.
Tests: tests/all/resource_tag_removal.rs asserts only removal semantics and ARN resolution (400 BadRequestException for an unparseable ARN, 403, 404); no row sends a reserved or empty tag key.

## Steps to Reproduce

1. Start the emulator with an activated device in the signed region.
2. DELETE /tags/arn%3Aaws%3Asnow-device-management%3A<region>%3A000000000000%3Amanaged-device%2F<deviceId>?tagKeys=aws%3Acost-center.
3. Observe HTTP 200 instead of a 400 ValidationException about the reserved `aws:` prefix.
4. Repeat with an empty tagKeys value (tagKeys=): also accepted with HTTP 200.
