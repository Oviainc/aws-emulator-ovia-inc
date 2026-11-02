Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.error-conflict-publish-version-stale-revision
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.error-conflict-publish-version-stale-revision|run#17|AssertionError|assert message == expected_message, (
Issue-Commit: a97363f812621dd47aa0891b39cb5eeebafd4acb

# PublishStateMachineVersion stale-revisionId ConflictException message is rewritten in emulator-local wording instead of AWS's "Failed to publish the State Machine version for revision ... The current State Machine revision is ..."

> _Note: this issue was identified against the code base at commit `a97363f812621dd47aa0891b39cb5eeebafd4acb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishStateMachineVersion

## Description
One net-new root cause, grounded in the single failing behavioral check the replay report left unmarked (no ALREADY-FILED and no OWNED marker).

Cause 1 of 1 — PublishStateMachineVersion's optimistic-concurrency refusal uses the right HTTP status and error code but the wrong error message: the emulator emits its own generic "revision ID ... is ..., not the specified ..." sentence instead of the message AWS words for this error class.
Failing check id: sfn.error-conflict-publish-version-stale-revision.

AWS: Smithy (services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json) models PublishStateMachineVersion with input {stateMachineArn (required, Arn, @length 1..256), revisionId (RevisionId), description} and output {creationDate (required), stateMachineVersionArn (required)}; its modeled errors are ConflictException (@httpError 409), InvalidArn (400), ServiceQuotaExceededException (402), StateMachineDeleting (400), StateMachineDoesNotExist (400), ValidationException (400). The model documents the member rule verbatim: "Only publish the state machine version if the current state machine's revision ID matches the specified ID. ... If the specified revision ID doesn't match the state machine's current revision ID, the API returns ConflictException." ConflictException's own doc: "Updating or deleting a resource can cause an inconsistent state. ... HTTP Status Code: 409", and its single member is `message`. The message AWS returns for this specific conflict class is the two-sentence template (recorded behavior for the exact text, which the check asserts verbatim):
  "Failed to publish the State Machine version for revision {requestedRevisionId}. The current State Machine revision is {currentRevisionId}."
Fixed text, which must be reproduced exactly: "Failed to publish the State Machine version for revision ", ". The current State Machine revision is ", and the trailing ".". Variable parts, taken from the request and from state: {requestedRevisionId} is the caller's `revisionId` input member, {currentRevisionId} is the state machine's current revisionId as DescribeStateMachine reports it. (The two UUIDs in the replay output are one run's values filling that template — they are not part of the requirement.)

Emulator: the status/code are right (the check's `code == "ConflictException"` assertion passed) but the message is a different sentence built from the state machine ARN:
  "the revision ID of 'arn:aws:states:<region>:<account>:stateMachine:<name>' is '<current>', not the specified '<requested>'"
It is lowercase, it is one clause rather than AWS's two sentences, it has no terminating period, it injects the state machine ARN (which AWS's message does not mention), and it inverts the order in which the two revision IDs appear. Replay detail: AssertionError: expected message 'Failed to publish the State Machine version for revision f9c93f14-....... The current State Machine revision is bbc78b18-.......', got "the revision ID of 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-behav-sm-pub-...' is 'bbc78b18-...', not the specified 'f9c93f14-...'".

Why: a client using revisionId as an optimistic-concurrency guard distinguishes "someone updated the state machine under me, re-read and retry" from other 409 conflicts (concurrent DeleteStateMachineVersion / UpdateStateMachine(publish=true)) by the message body, because ConflictException carries no other discriminator — the shape has only `message`. Retry/CI logic and log-scrapers that match AWS's fixed text, or that parse the current revision out of it to re-drive the publish, see no match and either fail the conflict open or surface an unrecognized error. Emulator-based tests of concurrency handling therefore pass code that would break against AWS.

Fix: the PublishStateMachineVersion handler's revisionId precondition check and the ConflictException it raises (services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/publish_state_machine_version.rs and whatever shared conflict/refusal-message helper it calls) must emit AWS's two-sentence template with the requested revisionId from the request and the current revisionId from state, rather than a generic ARN-plus-revision sentence. The status (409) and code (ConflictException) already match and should not change. Note: no existing open issue covers this operation — the nearest open sfn issues (20261024T043150Z-sfn-step-functions-control-plane-no-iam-passrole-authorization.md, 20261025T013638Z-sfn-step-functions-aliases-and-redrive-context-diverge-liststatemachinealiases.md, 20261024T114502Z-sfn-step-functions-asl-definition-validator-accepts-invalid-definitions.md) concern PassRole authorization, alias/redrive reads and ASL validation respectively, and the replay report did not mark this check as owned by any of them.

Traces: sfn.error-conflict-publish-version-stale-revision.

## Evidence
sfn.error-conflict-publish-version-stale-revision — verdict mismatch.

Expected (AWS): PublishStateMachineVersion(stateMachineArn=<sm>, revisionId=<stale>) where <stale> is no longer the current revision is refused with HTTP 409, error code ConflictException, and message exactly
  "Failed to publish the State Machine version for revision <stale>. The current State Machine revision is <current>."
and no version is created (ListStateMachineVersions unchanged).

Actual (emulator): HTTP 409 / ConflictException — correct — but message
  "the revision ID of '<stateMachineArn>' is '<current>', not the specified '<stale>'".
Replay assertion: AssertionError: expected message 'Failed to publish the State Machine version for revision f9c93f14-4f93-47e4-be7d-444c92522e3b. The current State Machine revision is bbc78b18-bfe7-44e1-8cdc-458b33594fa6.', got "the revision ID of 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-behav-sm-pub-fe11da7f9f88' is 'bbc78b18-bfe7-44e1-8cdc-458b33594fa6', not the specified 'f9c93f14-4f93-47e4-be7d-444c92522e3b'" (the UUIDs, ARN and name are that run's values; the requirement is the template).

The check aborted on the message assertion, so the downstream assertions it also makes (the rejected publish must not have created a version; a publish with the CURRENT revisionId must succeed and become visible via ListStateMachineVersions) were never graded — they are not claimed as divergent here, only unobserved.

## Steps to Reproduce
1. iam:CreateRole with a trust policy for states.amazonaws.com.
2. sfn:CreateStateMachine(name=<name>, definition={"StartAt":"Pass0","States":{"Pass0":{"Type":"Pass","End":true}}}, roleArn=<role>) -> <sm_arn>.
3. sfn:UpdateStateMachine(stateMachineArn=<sm_arn>, definition={"StartAt":"Pass1",...}) and poll sfn:DescribeStateMachine until `revisionId` is populated -> <stale>.
4. sfn:UpdateStateMachine(stateMachineArn=<sm_arn>, definition={"StartAt":"Pass2",...}) and poll sfn:DescribeStateMachine until `revisionId` differs from <stale> -> <current>.
5. sfn:PublishStateMachineVersion(stateMachineArn=<sm_arn>, revisionId=<stale>).
   Expected: HTTP 409, code ConflictException, message "Failed to publish the State Machine version for revision <stale>. The current State Machine revision is <current>."
   Observed: HTTP 409, code ConflictException, message "the revision ID of '<sm_arn>' is '<current>', not the specified '<stale>'".
6. sfn:ListStateMachineVersions(stateMachineArn=<sm_arn>) must be unchanged by the refused call, and sfn:PublishStateMachineVersion(stateMachineArn=<sm_arn>, revisionId=<current>) must succeed and the new version must appear in ListStateMachineVersions (not reached in this replay).
Replay: sfn.error-conflict-publish-version-stale-revision.
