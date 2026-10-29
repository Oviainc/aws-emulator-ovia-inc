Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-approval-apis-invalid-and-missing-pull-requests
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-approval-apis-invalid-and-missing-pull-requests codecommit-approval-state-and-evaluation-against-nonexistent-pull-request codecommit-approval-state-and-evaluation-malformed-pull-request-id codecommit-approval-override-validation-errors codecommit-approval-evaluation-and-state-validation-errors codecommit-create-pull-request-error-paths
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|EvaluatePullRequestApprovalRules|4xx->4xx|$.message
Issue-Key: codecommit|GetPullRequestOverrideState|4xx->4xx|$.message
Issue-Key: codecommit|OverridePullRequestApprovalRules|4xx->4xx|$.message
Issue-Key: codecommit|UpdatePullRequestApprovalState|4xx->4xx|$.message
Issue-Key: codecommit|GetPullRequest|4xx->4xx|$.message
Issue-Commit: 170d008231b721c2b3436293a90aa925526f50a2

# CodeCommit PullRequestDoesNotExistException carries the Smithy doc string instead of AWS's message (5 causes)

> _Note: this issue was identified against the code base at commit `170d008231b721c2b3436293a90aa925526f50a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EvaluatePullRequestApprovalRules

## Description
FIVE net-new causes, all one shared fault: every pull-request-scoped operation that must raise PullRequestDoesNotExistException emits the exception SHAPE'S DOCUMENTATION STRING as its `message` rather than AWS's runtime message for that error class. Status (400) and code (PullRequestDoesNotExistException) are already correct in each case; only the message text diverges, and it diverges identically, so one message template fixes all five. A fixer must close ALL FIVE before submitting.

=== Cause 1 of 5: EvaluatePullRequestApprovalRules ===
AWS: Smithy EvaluatePullRequestApprovalRules models PullRequestDoesNotExistException (errorKind client, httpError 400 [defaulted], single member `message`). Recorded behavior: for a pull request id that does not exist, AWS answers HTTP 400, __type PullRequestDoesNotExistException, message 'The pull request ID does not exist. Verify that you provided a valid pull request ID, and then try your operation again. To get a list of pull request IDs, use ListPullRequests.' That message is entirely FIXED text — it interpolates nothing from the request or from state, and in particular it does NOT echo the supplied id or the repository name.
Emulator: HTTP 400 and the right code, but message 'The pull request ID <id> could not be found. Make sure that you have specified the correct repository name and pull request ID, and then try again.', i.e. the shape's `doc` text ('The pull request ID could not be found. Make sure that you have specified the correct repository name and pull request ID, and then try again.') with the request's id spliced in. Hypothesis: the error constructor derives the message from the modeled shape documentation instead of a per-error message template.
Why: operators and client code match on the documented AWS message (and on the 'use ListPullRequests' remediation it names); a client that asserts or logs AWS's wording sees different prose, and the emulator additionally leaks the requested id into a message AWS keeps value-free.
Fix: the PullRequestDoesNotExistException construction used by services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/evaluate_pull_request_approval_rules.rs — give the error class its own fixed message template rather than the shape doc.
Traces: codecommit-approval-apis-invalid-and-missing-pull-requests#task#1.

=== Cause 2 of 5: GetPullRequestOverrideState ===
AWS: same modeled error and the same fixed message as Cause 1 for a nonexistent pull request id.
Emulator: the same doc-derived message with the id spliced in.
Why: as Cause 1.
Fix: src/ops/get_pull_request_override_state.rs.
Traces: codecommit-approval-apis-invalid-and-missing-pull-requests#task#3 (also codecommit-approval-override-validation-errors#task#4, codecommit-approval-state-and-evaluation-against-nonexistent-pull-request#task#5, codecommit-approval-state-and-evaluation-malformed-pull-request-id#task#5).

=== Cause 3 of 5: OverridePullRequestApprovalRules ===
AWS: same modeled error and the same fixed message.
Emulator: the same doc-derived message with the id spliced in.
Why: as Cause 1.
Fix: src/ops/override_pull_request_approval_rules.rs.
Traces: codecommit-approval-apis-invalid-and-missing-pull-requests#task#4.

=== Cause 4 of 5: UpdatePullRequestApprovalState ===
AWS: same modeled error and the same fixed message.
Emulator: the same doc-derived message with the id spliced in.
Why: as Cause 1.
Fix: src/ops/update_pull_request_approval_state.rs.
Traces: codecommit-approval-apis-invalid-and-missing-pull-requests#task#5.

=== Cause 5 of 5: GetPullRequest ===
AWS: same modeled error and the same fixed message.
Emulator: the same doc-derived message with the id spliced in.
Why: as Cause 1.
Fix: src/ops/get_pull_request.rs.
Traces: codecommit-create-pull-request-error-paths#task#5 (also #task#6).

## Evidence
Expected (recorded AWS, all five steps): HTTP 400, $.__type 'PullRequestDoesNotExistException', $.message 'The pull request ID does not exist. Verify that you provided a valid pull request ID, and then try your operation again. To get a list of pull request IDs, use ListPullRequests.' (fixed text, no interpolation).
Actual (emulator, all five steps): HTTP 400, $.__type 'PullRequestDoesNotExistException', $.message 'The pull request ID 999999999 could not be found. Make sure that you have specified the correct repository name and pull request ID, and then try again.' — the shape doc string plus the request's pull-request id.
Fixed vs variable: AWS's text is 100% fixed; the emulator's '999999999' is a value taken from the request and is not part of any AWS template for this error.

## Steps to Reproduce
1. Replay fixture codecommit-approval-apis-invalid-and-missing-pull-requests, steps task#1, task#3, task#4, task#5 (each calls an approval/PR operation with pullRequestId 999999999 against an existing repository).
2. Replay fixture codecommit-create-pull-request-error-paths step task#5 (GetPullRequest with pullRequestId 999999999).
3. Compare $.message with the recorded response: status and __type match, message text does not.
