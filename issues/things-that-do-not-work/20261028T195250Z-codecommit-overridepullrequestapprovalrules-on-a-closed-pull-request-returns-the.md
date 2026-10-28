Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit.merge-pull-request-refused-when-approval-rules-not-satisfied
Issue-Shape-Id: com.amazonaws.codecommit#CodeCommit_20150413
Issue-Key: codecommit|OverridePullRequestApprovalRules|PullRequestAlreadyClosedException
Issue-Commit: 1fce8a5a4e99700c9d3c5ad3e1ef05753020fc4d

# OverridePullRequestApprovalRules on a closed pull request returns the generic PullRequestAlreadyClosedException message instead of the operation's override-specific wording

> _Note: this issue was identified against the code base at commit `1fce8a5a4e99700c9d3c5ad3e1ef05753020fc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
OverridePullRequestApprovalRules

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: `OverridePullRequestApprovalRules` correctly refuses a merged/closed pull request with `PullRequestAlreadyClosedException` (HTTP 400), but it serialises the WRONG `message` — it emits the shared, generic sentence used by the pull-request-mutation operations rather than the override-specific sentence AWS returns for this operation.

AWS: Smithy (services/service-com-amazonaws-codecommit-codecommit-20150413/smithy-model/com-amazonaws-codecommit-codecommit-20150413.json) binds `PullRequestAlreadyClosedException` (errorKind client, httpError 400) to `OverridePullRequestApprovalRules`, whose input is `{pullRequestId, revisionId, overrideStatus}` (all required) with an empty output. The shape's own documentation string is the generic "The pull request status cannot be updated because it is already closed.", but the model does not specify per-operation message text, so the requirement here is recorded behavior only: against real CodeCommit, an `OverridePullRequestApprovalRules` call naming a pull request whose status is `CLOSED` is refused with code `PullRequestAlreadyClosedException` and the message

    "This pull request is already merged or closed. You cannot override the approval status of a merged or closed pull request. Modify your request, and then try again."

This message is entirely FIXED text — it interpolates no value from the request or from state (no pull request id, no revision id, no status, no override status appear in it). The same pull request, in the same closed state, refusing `UpdatePullRequestTitle` carries a DIFFERENT fixed sentence ("The change cannot be made because the state of the pull request is CLOSED. You cannot update the title or description of a pull request after it has been closed."), which the emulator already gets right — so the two refusals are distinct strings keyed to the operation, not one shared string keyed to the exception shape.

Emulator: the status (400) and the code (`PullRequestAlreadyClosedException`) match, but the message body is "The pull request status cannot be updated because it is already closed." The replayed assertion fails on exactly that value. The handler builds the error from a single crate-wide constant: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/override_pull_request_approval_rules.rs lines 288-299 return `PullRequestAlreadyClosedException { message: Some(crate::state::PULL_REQUEST_ALREADY_CLOSED_MESSAGE.to_string()) }`, i.e. the generic shape-documentation sentence shared across operations, with no override-specific variant.

Why: a client that branches on, logs, or surfaces the refusal text gets the wrong diagnosis. The override-specific sentence is what tells an operator that the approval-status decision surface is frozen because the pull request is merged or closed and that the request should be modified; the generic sentence instead claims "the pull request status cannot be updated", which describes a status-update refusal the caller never attempted. Tests, runbooks and support tooling that golden-match CodeCommit error text (a common practice because this exception reuses one code across many operations, so the message is the only discriminator) diverge against the emulator while passing against AWS. It also makes the emulator's own two closed-PR refusals indistinguishable in intent even though AWS words them apart.

Fix: the operation's handler/validation for `OverridePullRequestApprovalRules` — the closed-state guard must carry its own override-specific message constant rather than reusing the shared generic `PULL_REQUEST_ALREADY_CLOSED_MESSAGE` that `crate::state` exposes for the title/description mutation path. The code and 400 status are already right; only the message text belongs to this operation and must be distinguished from the shared one.

Traces: codecommit.merge-pull-request-refused-when-approval-rules-not-satisfied. Everything earlier in that check passes — the unsatisfied-rule merge refusal and its message, the destination-branch non-effect, the OVERRIDE/REVOKE round trip, the post-override merge and its mergeMetadata, the idempotent re-merge, and the `UpdatePullRequestTitle` closed-PR refusal including its message — so the check isolates this single message divergence as the last assertion it reaches.

Note: no currently open issue covers this. The open issue 20261027T073300Z-codecommit-getpullrequestoverridestate-stub.md concerns `GetPullRequestOverrideState` being a scaffold stub; in this replay that operation answers correctly (the check's OVERRIDE and REVOKE probes both read back the expected state), and the divergence filed here is in `OverridePullRequestApprovalRules`'s error message, a different operation and a different surface.

## Evidence
Check: codecommit.merge-proull-request-refused-when-approval-rules-not-satisfied (verdict: mismatch)

Failing assertion (check file line 316):
    try:
        cc.override_pull_request_approval_rules(
            pullRequestId=pr_id, revisionId=_revision(cc, pr_id), overrideStatus="REVOKE",
        )
        raise AssertionError("the override of a merged pull request was revoked")
    except ClientError as exc:
        code = exc.response["Error"]["Code"]           # -> PullRequestAlreadyClosedException (MATCHES)
        message = exc.response["Error"].get("Message")
        assert message == (
            "This pull request is already merged or closed. You cannot override the approval "
            "status of a merged or closed pull request. Modify your request, and then try again."
        ), message

Replay output:
    AssertionError: The pull request status cannot be updated because it is already closed.

Expected (AWS, recorded): HTTP 400, Code `PullRequestAlreadyClosedException`, Message
    "This pull request is already merged or closed. You cannot override the approval status of a merged or closed pull request. Modify your request, and then try again."

Actual (emulator): HTTP 400, Code `PullRequestAlreadyClosedException`, Message
    "The pull request status cannot be updated because it is already closed."

Message anatomy: the expected message is 100% fixed text; there are no request- or state-derived substitutions in it (the pull request id, revision id and overrideStatus do not appear). So the whole string is the requirement.

Emulator source: services/service-com-amazonaws-codecommit-codecommit-20150413/src/ops/override_pull_request_approval_rules.rs:288-299 — the closed-state guard constructs `PullRequestAlreadyClosedException` with `crate::state::PULL_REQUEST_ALREADY_CLOSED_MESSAGE`, the generic shape-doc sentence shared with the pull-request title/description path (which AWS words differently and which the emulator already returns correctly, so the two must not share one constant).

Smithy corroboration: OverridePullRequestApprovalRules declares PullRequestAlreadyClosedException (client, httpError 400, defaulted) among its errors; input `{pullRequestId, revisionId, overrideStatus}` all required; output Unit. The model fixes the code and status (both already correct) but not the per-operation message, hence "recorded behavior only" for the text.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator.
2. Run the behavioral suite; replay check `codecommit.merge-pull-request-refused-when-approval-rules-not-satisfied`.

Or reproduce directly against the emulator's codecommit endpoint:
1. CreateRepository, then PutFile on branch `main` (file.txt) to get commit c_base.
2. CreateBranch `feature` at c_base, PutFile on `feature` with parentCommitId=c_base to get commit c_feat.
3. CreatePullRequest with one target {repositoryName, sourceReference=feature, destinationReference=main}; note pullRequestId.
4. CreatePullRequestApprovalRule with an Approvers statement requiring 1 approval from a pool ARN that nobody matches.
5. OverridePullRequestApprovalRules(pullRequestId, revisionId=<current>, overrideStatus="OVERRIDE").
6. MergePullRequestByFastForward(pullRequestId, repositoryName) — the pull request becomes CLOSED with mergeMetadata.isMerged true.
7. Call OverridePullRequestApprovalRules(pullRequestId, revisionId=<current>, overrideStatus="REVOKE") on the now-closed pull request.
8. Inspect the refusal: Code is `PullRequestAlreadyClosedException` and status is 400 (both correct), but Message is "The pull request status cannot be updated because it is already closed." instead of AWS's "This pull request is already merged or closed. You cannot override the approval status of a merged or closed pull request. Modify your request, and then try again."
9. For contrast, call UpdatePullRequestTitle on the same closed pull request: the emulator returns the correct, differently worded message for the same exception code, showing the override path is reusing the wrong constant.
