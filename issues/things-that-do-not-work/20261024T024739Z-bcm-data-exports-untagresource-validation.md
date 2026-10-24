Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# UntagResource does not validate the ResourceArn's account before the lookup, so a foreign ARN gets the wrong error class

_Filed: 2026-10-24T02:47:39Z_
_Operation: UntagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The tagging operations of this service validate the `ResourceArn`'s ACCOUNT segment before any existence lookup: an ARN naming another account is unusable whatever this account holds, so it is refused with the modelled `ValidationException` (400, message "Resource doesn't support tagging") and never looked up. `ListTagsForResource` and `TagResource` both call `crate::state::validate_taggable_resource_arn`; `UntagResource` does not.

Expected: `UntagResource` with a well-formed ARN naming a different account answers `ValidationException` with the fixed tagging refusal message, decided without consulting the store.
Actual: the ARN goes straight into the export lookup and the caller gets `ResourceNotFoundException` ("Resource not found"), i.e. a different error code and a different meaning (not-found invites a create-then-retry) than the sibling tagging operations and than recorded AWS behaviour.

## Evidence

Handler: `src/ops/untag_resource.rs` lines 146-159 — no `crate::state::validate_taggable_resource_arn(&input.resource_arn)` call, unlike `src/ops/tag_resource.rs` line 178 and `src/ops/list_tags_for_resource.rs` line 183. `src/state.rs` (`validate_taggable_resource_arn`, `RESOURCE_NOT_TAGGABLE_MESSAGE`) documents the rule as recorded AWS behaviour for the tagging operations. Test coverage: `tests/all/tagging_arn_validation.rs` asserts the rule for `ListTagsForResource` and `TagResource` only, and its header (lines 14-15, 120-121) explicitly excludes `UntagResource` as "still a 501 stub" — a comment that is now stale since the handler is implemented, so nothing asserts this path.

## Steps to Reproduce

1. CreateExport to establish an export in this account (optional; the refusal must not depend on it).
2. Call UntagResource with ResourceArn = arn:aws:bcm-data-exports:us-east-1:111122223333:export/other_tenant (a well-formed ARN of a different account) and ResourceTagKeys = ["env"].
3. Observe ResourceNotFoundException ("Resource not found") instead of ValidationException with "Resource doesn't support tagging".
4. Compare with TagResource / ListTagsForResource on the same ARN, which answer 400 ValidationException.
