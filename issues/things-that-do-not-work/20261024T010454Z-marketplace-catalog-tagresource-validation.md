Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# TagResource accepts reserved `aws:`-prefixed tag keys

_Filed: 2026-10-24T01:04:54Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Tag$Key`'s modeled pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` admits a colon, so keys such as `aws:createdBy` pass boundary validation, and the handler's only extra check is the 200-tag count bound. The tag is merged into the resource's tag list and then reported back by `ListTagsForResource`. AWS reserves the `aws:` prefix for system tags across all tagging APIs: a user-supplied `aws:`-prefixed key is refused (ValidationException) and can never be created or edited by a caller. Expected: 422 ValidationException naming the reserved key; actual: 200 and the tag is persisted.

## Evidence

Spec/well-known semantics: `Tag$Key` is constrained only by length 1..128 and the character-class pattern above (model shape `com.amazonaws.marketplacecatalog#Tag`); AWS-wide tagging rules reserve the `aws:` prefix for AWS-generated tags, which users may not create or modify.
Handler: `src/ops/tag_resource.rs` lines ~175-198 merge the submitted tags into the stored list and only check `merged.len() > crate::state::MAX_RESOURCE_TAGS`; no key-prefix check exists in the handler or in `src/state.rs`.
Tests: no test in `tests/all/resource_policy_and_tagging.rs` (or elsewhere in the crate) exercises an `aws:`-prefixed tag key — the only tag refusal asserted is `tagging_past_the_modelled_bound_is_refused`.

## Steps to Reproduce

1. Create an entity via StartChangeSet and wait for SUCCEEDED; take its EntityArn
2. POST /TagResource with {"ResourceArn":"<EntityArn>","Tags":[{"Key":"aws:createdBy","Value":"me"}]}
3. Observe 200 instead of a ValidationException refusing the reserved prefix
4. POST /ListTagsForResource for the same ARN and observe the reserved tag reported back as a user tag
