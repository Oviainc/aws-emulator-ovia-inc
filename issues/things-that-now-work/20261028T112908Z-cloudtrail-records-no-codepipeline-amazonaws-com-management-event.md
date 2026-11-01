Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__codepipeline.a-manual-approval-decision-is-audited-with-the-reviewer-role-not-the-pipeline
Issue-Key: cloudtrail__codepipeline.a-manual-approval-decision-is-audited-with-the-reviewer-role-not-the-pipeline|_await#30|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: aeaf03eb4bcbb7a6c4e9492334e38170d85c4ec6

# CloudTrail records no codepipeline.amazonaws.com management event, so a manual-approval decision (PutApprovalResult) and its refused replay can never be attributed to the reviewer session through LookupEvents

> _Note: this issue was identified against the code base at commit `aeaf03eb4bcbb7a6c4e9492334e38170d85c4ec6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
One net-new root cause, grounded in the single failing check of the cloudtrail__codepipeline pair. (The check is NOT marked ALREADY FILED or OWNED by the replay report; the open issues 20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md, 20261028T010530Z-cloudtrail-event-history-records-nothing-for-read-only.md and 20261025T032228Z-codepipeline-execution-engine-still-stalls-an-action-never.md are the nearest neighbours and are named here, but none of them carries this check's replay key or names codepipeline.amazonaws.com as an audited event source.)

Cause 1 of 1: CloudTrail event history contains no record for a CodePipeline management call, so an approval decision cannot be audited at all.

AWS: CodePipeline is a CloudTrail-integrated service; every CodePipeline API call, successful or refused, is written to CloudTrail event history as a management event with eventSource "codepipeline.amazonaws.com" (the cloudTrailEventSource the CodePipeline Smithy service shape declares), eventCategory "Management", and readOnly=false for a write such as PutApprovalResult (Smithy: PutApprovalResult takes required pipelineName/stageName/actionName/result/token and returns approvedAt; its modelled refusals are ActionNotFoundException, ApprovalAlreadyCompletedException, InvalidApprovalTokenException, PipelineNotFoundException, StageNotFoundException, ValidationException, all HTTP 400). For a decision submitted by an assumed role, the record carries userIdentity.type="AssumedRole", userIdentity.arn = the reviewer's SESSION arn, sessionContext.sessionIssuer.arn = the reviewer's ROLE arn (never the pipeline's service role), and requestParameters echoing pipelineName, stageName, actionName and result.status ("Approved"/"Rejected") plus the reviewer's summary. The refused replay of a spent approval token produces a SECOND, distinct record (different eventID) with errorCode equal to the SDK's error code verbatim for this operation (the trailing "Exception" is not stripped, i.e. InvalidApprovalTokenException), a non-empty errorMessage, no responseElements, and the SAME reviewer attribution. Reference: https://docs.aws.amazon.com/codepipeline/latest/userguide/monitoring-cloudtrail-logs.html. The error codes/messages themselves come from the recorded response: the check captures the refusal code at runtime and only requires that CloudTrail repeat it verbatim.

Emulator: the check never completes — it is killed by the harness at the 60s ceiling while polling CloudTrail LookupEvents with LookupAttributes=[{EventName: PutApprovalResult}] (its own budget for that poll is 255s). LookupEvents returns no PutApprovalResult record at all: neither the successful decision nor the refused replay, for any page up to the 40-page ceiling. This matches the established shape of this emulator's audit plane, where event history holds only CloudTrail's own writes and no other service's event source is ever recorded (already filed separately for ec2.amazonaws.com, iam.amazonaws.com, states.amazonaws.com, elasticloadbalancing/rds, s3 and cognito event sources — codepipeline.amazonaws.com is a further, unfiled source). Note for the fixer: the check's earlier legs run against CodePipeline (GetPipelineState must publish the Review/ManualApproval action as InProgress with an approval token, PutApprovalResult must accept it and then refuse the spent token, and the action must reach Succeeded) — the emulator's handlers for those exist, and if the execution driver also stalls here that stall is the subject of 20261025T032228Z; the divergence filed HERE is the missing audit record, which is the check's terminal read and cannot be satisfied no matter how the CodePipeline legs behave.

Why: a client that uses CloudTrail to prove WHO approved a deployment gets nothing. Governance tooling that reads event history to show that a manual approval was answered by a reviewer principal rather than by the pipeline's service role, or to show that a replayed/stale approval token was refused and by whom, sees an empty Events list and concludes the approval never happened. Any audit, compliance or four-eyes control built on this contract silently passes on the emulator while reporting no activity.

Fix: the CloudTrail audit-record ingestion path — whatever component records a served request into the event-history store used by LookupEvents — must capture CodePipeline requests (eventSource codepipeline.amazonaws.com, eventCategory Management, readOnly from the operation), including the caller identity resolved from the request's signing credentials (userIdentity.type/arn and sessionContext.sessionIssuer.arn for an assumed-role session), the echoed requestParameters, and, on the error path, errorCode/errorMessage with no responseElements. The emulation of the operation itself (services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/ops/put_approval_result.rs) already produces the success and the InvalidApprovalTokenException refusal; what is missing is the emission of the audit record and the identity plumbed into it.

Traces: cloudtrail__codepipeline.a-manual-approval-decision-is-audited-with-the-reviewer-role-not-the-pipeline (verdict mismatch; "Check timed out after 60.0s").

## Evidence
Failing check: cloudtrail__codepipeline.a-manual-approval-decision-is-audited-with-the-reviewer-role-not-the-pipeline — verdict [mismatch], reported detail "Error: Check timed out after 60.0s".

Expected (AWS): after a reviewer session calls PutApprovalResult(pipelineName=<pipeline>, stageName="Review", actionName="ManualApproval", result={status:"Approved", summary:<reviewer summary>}, token=<live token>), CloudTrail LookupEvents with LookupAttributes=[{AttributeKey:"EventName", AttributeValue:"PutApprovalResult"}] returns a record whose parsed CloudTrailEvent has:
  eventSource = "codepipeline.amazonaws.com", eventCategory = "Management", readOnly = false, errorCode absent;
  userIdentity.type = "AssumedRole", userIdentity.arn = the reviewer's assumed-role SESSION arn, userIdentity.sessionContext.sessionIssuer.arn = the reviewer ROLE arn, and the pipeline's service-role arn nowhere inside userIdentity;
  requestParameters.pipelineName/stageName/actionName equal to the values sent, requestParameters.result.status = "Approved", and the reviewer's summary present.
And a SECOND record for the replay of the spent token, with a distinct eventID, errorCode equal verbatim to the SDK code the emulator itself returned for that refusal (recorded behavior: the check reads the code off the ClientError rather than assuming one; AWS does not strip the trailing "Exception" for this operation, i.e. "InvalidApprovalTokenException"), a non-empty errorMessage, no responseElements, and the same sessionContext.sessionIssuer.arn.

Actual (emulator): LookupEvents filtered on EventName=PutApprovalResult yields zero events across every page; the check's bounded wait (_INDEX_BUDGET_S = 255s, polling every 10s) never finds a record and the run is terminated at the harness 60s ceiling. No record exists for either the successful decision or its refused replay, so none of the attribution, requestParameters or errorCode assertions above can be evaluated. The same emptiness for other event sources is already filed per-source (ec2, iam, sfn, elbv2/rds, s3, cognito); codepipeline.amazonaws.com is not covered by any open issue.

## Steps to Reproduce
1. Clone the repo and boot the emulator (git_clone, then run_behavioral_suite).
2. Replay the single check cloudtrail__codepipeline.a-manual-approval-decision-is-audited-with-the-reviewer-role-not-the-pipeline.
   Equivalently, by hand against the emulator:
   a. Create two versioned S3 buckets (source + artifact) and put a small zip at source.zip in the source bucket.
   b. Create an IAM role trusted by codepipeline.amazonaws.com (the pipeline service role) and a SECOND, distinct IAM role (the reviewer) trusted by the account root and allowed codepipeline:PutApprovalResult / codepipeline:GetPipelineState.
   c. CreatePipeline with a Source stage (AWS/S3 provider, PollForSourceChanges=false) and a "Review" stage holding one AWS/Manual Approval action named "ManualApproval".
   d. Poll GetPipelineState until Review/ManualApproval reports latestExecution.status == "InProgress" with a token.
   e. AssumeRole into the reviewer role and, with those credentials, call PutApprovalResult with that token and result {status:"Approved", summary:<any>}. Then call it AGAIN with the same (now spent) token and record the error code returned.
   f. Call cloudtrail LookupEvents with LookupAttributes=[{AttributeKey:"EventName", AttributeValue:"PutApprovalResult"}], paging to exhaustion.
3. Observe: step (f) returns an empty Events list — no record with eventSource "codepipeline.amazonaws.com" for either the accepted approval or the refused replay — so the reviewer-attribution (userIdentity.arn / sessionContext.sessionIssuer.arn), requestParameters echo and errorCode-verbatim assertions can never be reached, and the check exhausts its budget.
