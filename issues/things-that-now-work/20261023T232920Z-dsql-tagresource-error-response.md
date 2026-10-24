Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 145ab011fad305b9f8c024d3a43ce6653fc244ab

# TagResource's bad-ARN refusal returns the synthetic framework ValidationException, omitting the modeled required `reason` (and `fieldList`)

_Filed: 2026-10-23T23:29:20Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `145ab011fad305b9f8c024d3a43ce6653fc244ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For a `resourceArn` that names no Aurora DSQL resource, TagResource returns `crate::scaffolded::Scaffolded::validation(..)` — the synthetic `smithy.framework#ValidationException` whose only member is `message`. The service's own `ValidationException` declares `reason` as @required (plus optional `fieldList`), and that is the shape a real client deserializes and branches on.

Expected: HTTP 400 with a body carrying `message` AND `reason` (`fieldValidationFailed`), exactly as the sibling tagging operations answer the identical input.

Actual: body carries `message` only. `ListTagsForResource` and `UntagResource` answer the same malformed ARN with the modeled shape via `crate::state::invalid_resource_arn(..)` (message + reason + fieldList), so the three tagging operations disagree about the body of one logical failure.

## Evidence

Spec: `ValidationException` members `message` (@required) and `reason` (@required, ValidationExceptionReason) plus `fieldList`; ValidationException is a service-common error so TagResource may return the modeled shape (other handlers widen it in with `extra_errors`). Handler: `src/ops/tag_resource.rs` lines 187-194 uses `Scaffolded::validation` (synthetic schema with only `message`, `src/scaffolded.rs` lines 117-178) whereas `src/ops/untag_resource.rs` lines 191-196 and `src/ops/list_tags_for_resource.rs` lines 163-168 use `crate::state::invalid_resource_arn` (`state.rs` lines 970-983). Tests: `tests/all/tag_resource.rs::tagging_an_arn_that_names_no_dsql_resource_is_refused_as_invalid` asserts the refusal but not the modeled `reason` member.

## Steps to Reproduce

1. POST /tags/arn:aws:s3:::my-bucket with {"tags": {"k": "v"}} → 400 with body {"message": "resourceArn must be the ARN of an Aurora DSQL resource ..."} and no `reason`.
2. GET /tags/arn:aws:s3:::my-bucket → 400 with body containing `message`, `reason` and `fieldList`.
3. Compare: the same logical refusal has two different bodies; the modeled required `reason` is missing from the TagResource one.
