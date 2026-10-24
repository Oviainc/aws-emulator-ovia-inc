Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 31b7b1cee38152b5b5fa499830b0bd9f17428f0d

# DeleteAutomationRule's not-found body uses the generic read-path text instead of naming the rule ARN

_Filed: 2026-10-24T10:54:43Z_
_Operation: DeleteAutomationRule | Class: error-response_

> _Note: this issue was identified against the code base at commit `31b7b1cee38152b5b5fa499830b0bd9f17428f0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A `DeleteAutomationRule` call against an ARN that no rule carries (never created, or already deleted) is correctly answered `ResourceNotFoundException` / HTTP 404, but the message is this service's single fixed read-path sentence, `"The specified resource could not be found"` (`crate::state::RESOURCE_NOT_FOUND_MESSAGE`).

The real service distinguishes the READ path from the MUTATION path for this resource: a read (`GetAutomationRule`) answers the generic sentence, while a mutation against a rule ARN that does not exist answers a message built from the request ARN, of the form `"Rule with ARN <ruleArn> does not exist"`. `DeleteAutomationRule` is a mutation on exactly the same resource, taking the same required `ruleArn` member as `UpdateAutomationRule`, so it falls on the mutation side of that split.

Expected: 404 `ResourceNotFoundException` whose message names the `ruleArn` the request supplied (`Rule with ARN arn:aws:compute-optimizer::<account>:automation-rule/<ruleId> does not exist`). Actual: 404 `ResourceNotFoundException` with the account- and resource-independent constant, so a client retrying or batching deletes cannot tell which ARN failed from the error body.

The same divergence applies to the other rule mutations that currently reuse the same constant (`TagResource`, `UntagResource` in `src/ops/tag_resource.rs` / `src/ops/untag_resource.rs`); fixing the shared helper so mutation paths format the ARN while the read paths keep the generic sentence addresses the family, which is why this is filed once against the delete path rather than per tagging operation.

## Evidence

Spec/model: `DeleteAutomationRule` takes `ruleArn` (@required, pattern `^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$`) and declares `ResourceNotFoundException` (client, httpError 404) — smithy_query(query="operation", operation="DeleteAutomationRule").

Established service behavior: the AWS recording cited by the already-filed sibling issue `20261023T134046Z-compute-optimizer-automation-error-messages-not-built-from` (trace `compute-optimizer-automation.delete-automation-rule-stale-revision-rejected`) shows the real service rejecting a MUTATION against a deleted rule with `ResourceNotFoundException` and the message `"Rule with ARN arn:aws:compute-optimizer::000000000000:automation-rule/42d39f82e52fa35c does not exist"`, while the read (`GetAutomationRule`) in the same recording uses `"The specified resource could not be found"` — i.e. the two paths are worded differently by AWS. That issue is scoped to `UpdateAutomationRule`; the delete path is a separate operation and is not covered by it.

Handler: `src/ops/delete_automation_rule.rs:212-219` builds `ResourceNotFoundException { message: Some(crate::state::RESOURCE_NOT_FOUND_MESSAGE.to_string()) }`; the constant is declared in `src/state.rs:344` and documented there as "FIXED and resource-independent … names neither the ARN nor the id", and every not-found of the service shares it.

Test coverage: `tests/all/error_payloads.rs:289-313` asserts the DELETE cases (`delete-a-deleted-rule`, `delete-a-rule-that-never-existed`) carry exactly `NOT_FOUND_MESSAGE` and additionally asserts the body echoes no ARN — i.e. the current, generic behavior is pinned by a test, so the test has to move with the fix (the same test file pins the `update-a-deleted-rule` case that the trace-derived sibling issue already shows to be wrong).

## Steps to Reproduce

1. Enroll the caller: `UpdateEnrollmentConfiguration` with `{"status":"Active"}`.
2. `CreateAutomationRule` with a minimal valid rule; keep `ruleArn` and `ruleRevision`.
3. `DeleteAutomationRule` with that `ruleArn` and `ruleRevision` — succeeds (HTTP 200, empty body).
4. Call `DeleteAutomationRule` again with the same `ruleArn` and `ruleRevision`, this time without a `clientToken` so no idempotent replay applies.
5. Observe HTTP 404 `ResourceNotFoundException` with message `"The specified resource could not be found"`; expected a mutation-path message naming the ARN, e.g. `"Rule with ARN <ruleArn> does not exist"`.
6. For contrast, call `GetAutomationRule` with the same ARN — the generic sentence IS correct there, showing the emulator conflates the read and mutation wordings.
