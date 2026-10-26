Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.put-approval-result-already-completed-rejected
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.put-approval-result-already-completed-rejected|run#19|AssertionError|assert error.get("Message") == expected_message, (
Issue-Commit: 949cf2f2a2f42313b725a3f96914da0fa4cf14da

# CodePipeline PutApprovalResult: ApprovalAlreadyCompletedException carries a locally-invented message instead of AWS's fixed wording

> _Note: this issue was identified against the code base at commit `949cf2f2a2f42313b725a3f96914da0fa4cf14da`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutApprovalResult

## Description
One net-new root cause.

Cause 1 of 1: the duplicate-submission refusal of PutApprovalResult returns the right error code but the wrong error message. Failing check: codepipeline.put-approval-result-already-completed-rejected.

AWS: PutApprovalResult models ApprovalAlreadyCompletedException (Smithy: services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/com-amazonaws-codepipeline-codepipeline-20150709.json — operation PutApprovalResult, errors include ApprovalAlreadyCompletedException, errorKind client, HTTP 400 defaulted; shape doc "The approval action has already been approved or rejected.", single member `message` targeting Message with @length min 1 max 5000). Re-submitting the same approval token for an approval action that has already received a decision is refused with HTTP 400, code `ApprovalAlreadyCompletedException`, and the recorded message is entirely FIXED text — it contains no values taken from the request or from state at all:
  "The approval request that you responded to is already complete or has expired. Review the latest revisions in the pipeline, and then try again."
The model does not specify the message string itself (it only constrains its length), so the wording above is recorded behavior: it is what the replayed check asserts as real AWS's response for this error class, and it is a single invariant sentence pair with no interpolated pipeline/stage/action name.

Emulator: the status (400) and the error code (ApprovalAlreadyCompletedException) are correct — the check's Code assertion passed — but the message is a different, locally-authored sentence that interpolates request values:
  "The approval action 'Approve' in stage 'Gate' of pipeline 'sim2real-cp-appr1-c07f538e2dfc' has already received a response"
Breaking that down against the requirement: NONE of it is AWS's fixed text. The emulator's template is "The approval action '<actionName>' in stage '<stageName>' of pipeline '<pipelineName>' has already received a response", where actionName/stageName/pipelineName are values echoed from the PutApprovalResult request (here Approve / Gate / the generated pipeline name — those three are per-run values that fill the emulator's template, not part of any requirement). AWS's message by contrast has no variable parts: both sentences are fixed text, and the second sentence ("Review the latest revisions in the pipeline, and then try again.") is missing from the emulator's response entirely. The trailing period is also absent.

Why: a client that keys on the message rather than only the code gets the wrong answer. Callers that surface the approval failure to an operator lose AWS's remediation sentence ("Review the latest revisions in the pipeline, and then try again."), which is the part that tells a reviewer the token may have expired against a newer pipeline revision rather than merely been reused. Message-matching assertions in customer test suites and in CI gating around manual-approval retries fail against the emulator while passing against AWS, and log scrapers / support tooling that classify approval failures by message text mis-bucket this refusal. Because the emulator's variant also embeds the pipeline name, it leaks per-request identifiers into a string AWS keeps constant, so golden-file comparisons of error payloads are not stable across environments.

Fix: the PutApprovalResult handler's approval-token/already-completed rejection path in the codepipeline service crate — the place that constructs the ApprovalAlreadyCompletedException body. It must emit AWS's fixed message verbatim and stop interpolating pipelineName/stageName/actionName into it. The surrounding behavior is already correct and must be preserved: HTTP 400, code ApprovalAlreadyCompletedException, and no state change (the check's subsequent assertions that the action stays Succeeded with the first call's summary are downstream of this failed assertion and should be re-verified once the message is fixed).

Traces: codepipeline.put-approval-result-already-completed-rejected (mismatch — AssertionError on the Message comparison at check line 67, after the Code assertion passed).

## Evidence
Check: codepipeline.put-approval-result-already-completed-rejected [mismatch]

Scenario: create a V2/SUPERSEDED pipeline with an S3 Source stage and an Approval stage (stage "Gate", action "Approve", actionTypeId Approval/AWS/Manual/1), start an execution, read the approval token from GetPipelineState, call PutApprovalResult once successfully (positive control returns approvedAt and the action goes to Succeeded), then call PutApprovalResult again with the SAME token.

Second call — expected (real AWS):
  HTTP 400
  Error.Code    = "ApprovalAlreadyCompletedException"
  Error.Message = "The approval request that you responded to is already complete or has expired. Review the latest revisions in the pipeline, and then try again."
  (entirely fixed text; no request- or state-derived substitutions)

Second call — actual (emulator):
  HTTP 400
  Error.Code    = "ApprovalAlreadyCompletedException"   <- correct
  Error.Message = "The approval action 'Approve' in stage 'Gate' of pipeline 'sim2real-cp-appr1-c07f538e2dfc' has already received a response"   <- diverges

Raw replay failure:
  File "check_codepipeline_put_approval_result_already_completed_rejected.py", line 67, in run
    assert error.get("Message") == expected_message
  AssertionError: expected message 'The approval request that you responded to is already complete or has expired. Review the latest revisions in the pipeline, and then try again.', got "The approval action 'Approve' in stage 'Gate' of pipeline 'sim2real-cp-appr1-c07f538e2dfc' has already received a response"

Fixed vs variable parts of the emulator's message: fixed text is "The approval action '", "' in stage '", "' of pipeline '", "' has already received a response"; the variable parts are actionName=Approve, stageName=Gate and pipelineName=sim2real-cp-appr1-c07f538e2dfc, all three echoed from the request (the pipeline name is a per-run generated value and is not itself a requirement). AWS's message has no variable parts.

Model citation: Smithy PutApprovalResult errors -> ApprovalAlreadyCompletedException (client, httpError 400 defaulted); shape ApprovalAlreadyCompletedException doc "The approval action has already been approved or rejected.", member message -> Message @length(min 1, max 5000). The model constrains only the length of message, so the exact wording above is from the recorded behavior the check asserts.

Not covered by any existing issue: this check is the only failure in the suite report not marked ALREADY FILED or OWNED by. The other 51 codepipeline-family failures are owned by 20261025T032228Z-codepipeline-execution-engine-still-stalls-an-action-never.md, 20261023T080113Z-codepipeline-control-plane-declaration-validation-stage-transition-state.md, 20261025T032228Z-codepipeline-s3-deploy-action-demands-objectkey-when-extract.md, 20261024T114145Z-codepipeline-createpipeline-never-triggers-its-own-execution-and.md, 20261024T014332Z-codebuild-codepipeline-ecr-codepipeline-build-stages-fail-and-record-no-action.md, 20261022T172625Z-codebuild-codepipeline-ecr-ci-chain-pipeline-executions-never.md, 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md and 20261022T173520Z-codepipeline-events-sqs-codepipeline-stage-action-state-change-events-are-never.md. None of those concerns the PutApprovalResult error message.

## Steps to Reproduce
1. git_clone, then run the behavioral suite and replay check codepipeline.put-approval-result-already-completed-rejected.

Manual reproduction against the emulator:
1. Create an S3 artifact bucket (versioning enabled) and put a zip object at key "source.zip".
2. Create an IAM role trusted by codepipeline.amazonaws.com with s3 read/write on that bucket.
3. CreatePipeline with pipelineType=V2, executionMode=SUPERSEDED, artifactStore {type: S3, location: <bucket>} and two stages:
   - "Source": action "Source", actionTypeId {category: Source, owner: AWS, provider: S3, version: "1"}, configuration {S3Bucket, S3ObjectKey: source.zip, PollForSourceChanges: "false"}, outputArtifacts [{name: SourceOutput}].
   - "Gate": action "Approve", actionTypeId {category: Approval, owner: AWS, provider: Manual, version: "1"}.
4. StartPipelineExecution(name=<pipeline>).
5. Poll GetPipelineState(name=<pipeline>) until stageStates["Gate"].actionStates["Approve"].latestExecution.token is present; capture that token.
6. PutApprovalResult(pipelineName=<pipeline>, stageName="Gate", actionName="Approve", result={summary: "looks good", status: "Approved"}, token=<token>) -> succeeds, returns approvedAt.
7. PutApprovalResult again with the SAME token and result={summary: "duplicate attempt", status: "Approved"}.
8. Inspect the ClientError: Code is ApprovalAlreadyCompletedException (correct) but Message is the emulator's interpolated "The approval action '<action>' in stage '<stage>' of pipeline '<pipeline>' has already received a response" instead of AWS's fixed "The approval request that you responded to is already complete or has expired. Review the latest revisions in the pipeline, and then try again."
