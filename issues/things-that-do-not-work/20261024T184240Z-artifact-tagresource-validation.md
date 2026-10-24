Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# TagResource accepts tag keys in the reserved `aws:` namespace

_Filed: 2026-10-24T18:42:40Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The emulator's `TagResource` merges whatever keys the request carries into the resource's tag set, checking only the per-resource count (50). AWS reserves the `aws:` tag-key prefix for AWS's own use: a `TagResource` call that tries to create or modify a key beginning with `aws:` is refused (a 400 `ValidationException` here, since that is the client-input error this model declares), and such a key can never appear in `ListTagsForResource` output for a user-created tag.

Expected (AWS): POST /tags/<inquiry ARN> with {"tags":{"aws:cloudformation:stack-name":"mine"}} is refused with 400 `ValidationException` (reason `fieldValidationFailed`, fieldList naming `tags`); the resource's tag set is unchanged.

Actual (emulator): the call answers 204 and the reserved key is persisted — a subsequent `GET /tags/<arn>` reports `aws:cloudformation:stack-name` among the resource's tags, and `UntagResource` will happily delete it. The modelled `TagKey` pattern `^[a-zA-Z0-9\s_.:/=+\-@]*$` admits the `:` character, so nothing at the boundary refuses it and the handler adds no check. The same hole exists on `CreateComplianceInquiryRequest$tags`, which flows into the same stored tag set.

## Evidence

Spec: `com.amazonaws.artifact#TagsMap` = map<TagKey(len 1..128, pattern ^[a-zA-Z0-9\s_.:/=+\-@]*$), TagValue(len 0..256)>, @length(max 50); `TagResource` declares `ValidationException` (HTTP 400, "Request fails to satisfy the constraints specified by an AWS service"). Well-established AWS-wide tagging semantics: keys prefixed `aws:` are reserved for AWS use and cannot be created, edited or deleted through a service's tagging API.

Handler: services/service-com-amazonaws-artifact-artifact/src/ops/tag_resource.rs — `let mut tags = resource.tags.clone(); tags.extend(input.tags); if tags.len() > crate::state::TAGS_PER_RESOURCE_LIMIT { ... }` — the ONLY rule enforced is the 50-tag count (`crate::state::too_many_tags`); no key-namespace check exists anywhere in the crate (src/state.rs defines only `TAGS_PER_RESOURCE_LIMIT`).

Tests: tests/all/resource_tag_writes.rs and tests/all/resource_tags.rs exercise tag writes, the 50-tag limit and the unknown-ARN refusal; neither sends an `aws:`-prefixed key, so no test asserts either behaviour. No existing issue (open or fixed) mentions reserved tag keys.

## Steps to Reproduce

1. POST /v1/compliance-inquiry/create with {"name":"t","inquiryContent":{"query":"ping"}} and read the inquiry id, then build its ARN (arn:aws:artifact:<region>:<account>:compliance-inquiry/<id>) — or read it from complianceInquirySummary.arn.
2. POST /tags/<url-encoded ARN> with body {"tags":{"aws:cloudformation:stack-name":"mine"}} signed for the artifact service.
3. Observe 204; expect a 400 ValidationException instead, because the key is in the reserved `aws:` namespace.
4. GET /tags/<url-encoded ARN> and observe the reserved key reported in `tags`, which real AWS never returns for a caller-created tag.
5. Repeat the write through CreateComplianceInquiry's own `tags` member ({"aws:foo":"bar"}) to see the same reserved key accepted on the create path.
