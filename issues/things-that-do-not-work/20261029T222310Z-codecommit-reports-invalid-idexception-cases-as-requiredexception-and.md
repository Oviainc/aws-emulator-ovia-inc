Issue-Source: trace-sweep-divergence
Issue-Trace: codecommit-approval-apis-invalid-and-missing-pull-requests
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codecommit-approval-apis-invalid-and-missing-pull-requests codecommit-approval-evaluation-and-state-validation-errors codecommit-approval-state-and-evaluation-against-nonexistent-pull-request codecommit-approval-state-and-evaluation-malformed-pull-request-id codecommit-approval-rule-content-validation-errors codecommit-approval-rule-crud-against-nonexistent-pull-request codecommit-approval-rule-crud-malformed-pull-request-id codecommit-approval-rule-pull-request-id-errors codecommit-get-comments-for-pull-request-error-paths codecommit-pull-request-comment-error-paths codecommit-pull-request-comment-operations-nonexistent-pr codecommit-merge-by-squash-invalid-pull-request-id codecommit-merge-pull-request-by-squash-not-found codecommit-merge-pull-request-validation-and-not-found-errors codecommit-merge-pull-request-by-threeway-invalid-id codecommit-post-comment-for-pull-request-error-paths codecommit-approval-override-validation-errors
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|GetPullRequestApprovalStates|4xx->4xx|$.message
Issue-Key: codecommit|DeletePullRequestApprovalRule|4xx->4xx|$.message
Issue-Key: codecommit|GetCommentsForPullRequest|4xx->4xx|$.message
Issue-Key: codecommit|MergePullRequestBySquash|4xx->4xx|$.message
Issue-Key: codecommit|MergePullRequestByThreeWay|4xx->4xx|$.message
Issue-Key: codecommit|PostCommentForPullRequest|4xx->4xx|$.message
Issue-Key: codecommit|GetPullRequestOverrideState|4xx->4xx|$.__type
Issue-Key: codecommit|GetCommentsForPullRequest|4xx->4xx|$.__type
Issue-Key: codecommit|EvaluatePullRequestApprovalRules|4xx->4xx|$.__type
Issue-Key: codecommit|GetPullRequestApprovalStates|4xx->4xx|$.__type
Issue-Related: 20261029T222310Z-codecommit-pullrequestdoesnotexistexception-carries-the-smithy-doc-string-instead.md
Issue-Commit: 170d008231b721c2b3436293a90aa925526f50a2

# CodeCommit reports Invalid*IdException cases as *RequiredException and omits the offending value from the message (10 causes)

> _Note: this issue was identified against the code base at commit `170d008231b721c2b3436293a90aa925526f50a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPullRequestApprovalStates

## Description
TEN net-new causes sharing one fault: for malformed or empty identifier members, the emulator (a) classifies an EMPTY value as the '<member>RequiredException' class where AWS raises the 'Invalid<member>Exception' class, and (b) emits the exception shape's documentation string as the message instead of AWS's message template, which names the offending value. A fixer must close ALL TEN before submitting.

Shared AWS grounding (Smithy, com.amazonaws.codecommit): InvalidPullRequestIdException, PullRequestIdRequiredException, InvalidRevisionIdException and RevisionIdRequiredException are all errorKind client, httpError 400 [defaulted], with a single `message` member. Their `doc` strings are exactly the strings the emulator is returning, which is the tell. Recorded behavior shows AWS's runtime messages instead:
  - malformed id: 'The following pull request ID is not valid: <pullRequestId>. To view a list of valid pull request IDs for this repository, use ListPullRequests.' — fixed text with ONE variable part, the pullRequestId copied verbatim from the request.
  - empty/null id: code InvalidPullRequestIdException (NOT PullRequestIdRequiredException) and message 'The following pull request ID is empty or null and therefore not valid: . To view a list of valid pull request IDs for this repository, use ListPullRequests.' — same template with the empty value interpolated.
  - empty/null revisionId: code InvalidRevisionIdException (NOT RevisionIdRequiredException) and message 'A revision ID is required, but the value is either empty or null.' (fixed text).
Emulator (all ten): status 400 is right; code is right only for the malformed-id cases; message is always the shape doc text, which omits the offending value entirely.
Why: AWS SDKs model each of these as a distinct exception type, so a client with `except InvalidPullRequestIdException` / `catch (InvalidRevisionIdException)` never enters its handler when the emulator answers the *Required* class — the failure escapes as an unexpected exception. Clients and operators that surface the message also lose the only piece of actionable data AWS puts in it: which id was rejected.
Fix: the identifier validation in each operation's handler under services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/ — select Invalid*Exception for both malformed AND empty values, and build the message from a per-error template that interpolates the request's value, rather than from the modeled shape documentation.

=== Cause 1 of 10: GetPullRequestApprovalStates, malformed pullRequestId ===
Traces: codecommit-approval-apis-invalid-and-missing-pull-requests#task#2 (also codecommit-approval-evaluation-and-state-validation-errors#task#3 and two more).
=== Cause 2 of 10: DeletePullRequestApprovalRule, malformed pullRequestId ===
Traces: codecommit-approval-rule-content-validation-errors#task#4.
=== Cause 3 of 10: GetCommentsForPullRequest, malformed pullRequestId ===
Traces: codecommit-get-comments-for-pull-request-error-paths#task#1.
=== Cause 4 of 10: MergePullRequestBySquash, malformed pullRequestId ===
Traces: codecommit-merge-by-squash-invalid-pull-request-id#task#1.
=== Cause 5 of 10: MergePullRequestByThreeWay, malformed pullRequestId ===
Traces: codecommit-merge-pull-request-by-threeway-invalid-id#task#3.
=== Cause 6 of 10: PostCommentForPullRequest, malformed pullRequestId ===
Traces: codecommit-post-comment-for-pull-request-error-paths#task#1.
=== Cause 7 of 10: GetPullRequestOverrideState, EMPTY pullRequestId -> wrong error class ===
Emulator answers PullRequestIdRequiredException / 'A pull request ID is required, but none was provided.'; AWS answers InvalidPullRequestIdException with the 'empty or null and therefore not valid' message.
Traces: codecommit-approval-override-validation-errors#task#3.
=== Cause 8 of 10: GetCommentsForPullRequest, EMPTY pullRequestId -> wrong error class ===
Traces: codecommit-get-comments-for-pull-request-error-paths#task#2 (also #task#5).
=== Cause 9 of 10: EvaluatePullRequestApprovalRules, EMPTY revisionId -> wrong error class ===
Emulator answers RevisionIdRequiredException / 'A revision ID is required, but was not provided.'; AWS answers InvalidRevisionIdException / 'A revision ID is required, but the value is either empty or null.'
Traces: codecommit-approval-evaluation-and-state-validation-errors#task#1.
=== Cause 10 of 10: GetPullRequestApprovalStates, EMPTY revisionId -> wrong error class ===
Traces: codecommit-approval-evaluation-and-state-validation-errors#task#2.

## Evidence
Malformed-id causes (1-6) — expected $.message 'The following pull request ID is not valid: <pullRequestId>. To view a list of valid pull request IDs for this repository, use ListPullRequests.' (recorded values: 'not-a-valid-id!!', 'abc-not-numeric', 'not-a-valid-pr-id!!', 'not-a-valid-pull-request-id!!'); actual $.message 'The pull request ID is not valid. Make sure that you have provided the full ID and that the pull request is in the specified repository, and then try again.' == the Smithy doc of InvalidPullRequestIdException, with no id.
Empty-id causes (7-8) — expected $.__type 'InvalidPullRequestIdException', $.message 'The following pull request ID is empty or null and therefore not valid: . To view a list of valid pull request IDs for this repository, use ListPullRequests.'; actual $.__type 'PullRequestIdRequiredException', $.message 'A pull request ID is required, but none was provided.'
Empty-revisionId causes (9-10) — expected $.__type 'InvalidRevisionIdException', $.message 'A revision ID is required, but the value is either empty or null.'; actual $.__type 'RevisionIdRequiredException', $.message 'A revision ID is required, but was not provided.'
All twenty steps answered HTTP 400 as expected; the divergence is code and/or message only.

## Steps to Reproduce
1. Replay codecommit-approval-apis-invalid-and-missing-pull-requests#task#2, codecommit-approval-rule-content-validation-errors#task#4, codecommit-get-comments-for-pull-request-error-paths#task#1, codecommit-merge-by-squash-invalid-pull-request-id#task#1, codecommit-merge-pull-request-by-threeway-invalid-id#task#3, codecommit-post-comment-for-pull-request-error-paths#task#1 — each sends a syntactically invalid pullRequestId and must get the 'The following pull request ID is not valid: <id>' message.
2. Replay codecommit-approval-override-validation-errors#task#3 and codecommit-get-comments-for-pull-request-error-paths#task#2 — each sends an empty pullRequestId and must get __type InvalidPullRequestIdException.
3. Replay codecommit-approval-evaluation-and-state-validation-errors#task#1 and #task#2 — each sends an empty revisionId and must get __type InvalidRevisionIdException.
4. Compare $.__type and $.message against the recorded responses.
