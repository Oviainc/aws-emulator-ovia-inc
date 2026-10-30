Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue
Issue-Key: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue|run#46|Timeout|time.sleep(10)
Issue-Commit: 716594ad03747236be5f362ba3ba8170b7a2d152

# batch__events__sqs: the Batch Job State Change rule-routing check never reaches a verdict — no job-state-change event is observed on the rule's SQS target

> _Note: this issue was identified against the code base at commit `716594ad03747236be5f362ba3ba8170b7a2d152`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Cause 1 of 1: a Batch job's state changes are not observable through an EventBridge rule whose pattern pins the job's queue ARN, so the whole observe-through-EventBridge path never completes.

AWS: Batch's control plane is modelled (Smithy: SubmitJob POST /v1/submitjob -> {jobId, jobArn, jobName}, errors ClientException 400 / ServerException 500; TerminateJob POST /v1/terminatejob with required jobId and required reason; DescribeJobs POST /v1/describejobs -> JobDetailList; CreateComputeEnvironment POST /v1/createcomputeenvironment with required type (CEType, i.e. UNMANAGED is a modelled value) and optional serviceRole). The event stream itself is NOT in the Smithy model, so for that part this is recorded/documented behavior only: AWS Batch publishes to the account's DEFAULT EventBridge bus an event with source "aws.batch" and detail-type "Batch Job State Change"; its `resources` is exactly the job ARN; its `detail` is the job in the same member names DescribeJobs reports (jobId, jobArn, jobName, jobQueue as the queue ARN, jobDefinition as the job-definition ARN, status, createdAt in epoch millis, statusReason once set); no event is published for the initial SUBMITTED status, and a job on an UNMANAGED compute environment rests in RUNNABLE until a caller ends it, at which point TerminateJob's `reason` appears in the FAILED event's detail.statusReason. A rule whose pattern is {"source":["aws.batch"],"detail-type":["Batch Job State Change"],"detail":{"jobQueue":[<queue ARN>]}} must receive the events of jobs on that queue and must receive NOTHING for a sibling queue it does not name.

Emulator: the check never reaches a verdict — the replay reports `Check timed out after 60.0s`. The check is structured so that nothing it observes is used as evidence until it has first proved, per queue, that the EventBridge-rule -> SQS wiring can deliver (a throwaway rule on a source the check itself PutEvents to) and then drained both queues to empty; after that it submits a job and polls the watched SQS queue for a RUNNABLE `Batch Job State Change` event. No verdict is produced, so no event was ever observed on the rule's SQS target within the budget. The Batch crate does queue and publish job state changes (services/service-com-amazonaws-batch-awsbatchv20160810/src/state.rs defines EVENT_SOURCE = "aws.batch", JOB_STATE_CHANGE_DETAIL_TYPE = "Batch Job State Change", DEFAULT_EVENT_BUS = "default" and job_state_change_detail(), and src/ops/submit_job.rs calls publish_job_state_changes/drive_job_clock), so the break is on the delivery side of that publish rather than in the Batch record itself. Hypothesis: the rule's nested `detail.jobQueue` pattern never matches the published envelope, or the published event never reaches the default bus's rules at all, so the rule's SQS target stays empty forever. The suite's own EventBridge pattern checks show a nested-object pattern defect (events__sqs.events-pattern-language-nested-object-pattern-matches-the-leaf-path, carried by the open issue 20261029T182228Z-events-sqs-eventbridge-puttargets-accepts-conflicting-invalid-input-forms-the.md); that issue does not carry this check's replay key, so this one is filed separately and should be closed only once this check reaches a verdict.

Why: a client that watches Batch the documented, polling-free way — a rule on aws.batch / "Batch Job State Change" scoped to one job queue — sees nothing. Job progress and terminal outcome become invisible to any event-driven consumer (fan-out to SQS, Lambda, Step Functions), and the negative half of the contract is equally unobservable: a consumer cannot tell "correctly withheld from the sibling queue" from "nothing is ever delivered anywhere".

Fix: the Batch job-state-change publisher and the EventBridge default-bus rule matching/target-delivery path it publishes through — i.e. the publish seam in services/service-com-amazonaws-batch-awsbatchv20160810/src/state.rs (publish_job_state_changes) together with EventBridge's event-pattern matching for nested `detail` members and its SQS target delivery. The Batch detail projection and the Batch control-plane handlers look correct and are not where the change belongs.

Traces: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue (verdict mismatch, `Check timed out after 60.0s`).

## Evidence
Expected (AWS): after SubmitJob onto an ENABLED job queue backed by an UNMANAGED compute environment, an envelope arrives on the SQS queue targeted by the rule {"source":["aws.batch"],"detail-type":["Batch Job State Change"],"detail":{"jobQueue":[<watched queue ARN>]}} with source="aws.batch", detail-type="Batch Job State Change", account=<caller account>, region=<request region>, resources=[<job ARN>], detail.jobId=<submitted job id>, detail.jobQueue=<watched queue ARN>, detail.status="RUNNABLE", detail.jobDefinition starting with "arn:", detail.createdAt an epoch-millis integer equal to DescribeJobs' createdAt, and detail.jobName equal to DescribeJobs' jobName; no SUBMITTED event appears; after TerminateJob(jobId, reason=<reason>) a second envelope arrives with detail.status="FAILED" whose detail.statusReason contains <reason>; and the sibling queue's rule (pinned to the other job queue's ARN) receives nothing and reports 0 visible / 0 in-flight messages until a job is submitted to that sibling queue, at which point its own RUNNABLE event arrives there. (The ARNs, job ids, account, region and timestamps above are per-run values taken from the request and the emulator's state; the fixed parts of the contract are the source string, the detail-type string, the member set and the status values.)

Actual (emulator): the check produces no verdict at all — `Check timed out after 60.0s`. Nothing was observed on the watched queue, so neither the delivered envelope nor the withheld-from-sibling half of the contract could be evaluated.

## Steps to Reproduce
1. Create an IAM role trusting batch.amazonaws.com with AWSBatchServiceRole attached.
2. CreateComputeEnvironment type=UNMANAGED state=ENABLED serviceRole=<that role>; poll DescribeComputeEnvironments until status=VALID.
3. CreateJobQueue twice (state=ENABLED, priority=10) on that compute environment; poll DescribeJobQueues until each is VALID. Call them WATCHED and SIBLING and note their ARNs.
4. Create two SQS queues and set each one's Policy to allow events.amazonaws.com sqs:SendMessage with Condition ArnLike aws:SourceArn arn:aws:events:<region>:<account>:rule/*.
5. Prove each wiring delivers: PutRule on the default bus with pattern {"source":["<your own source>"],"detail":{"token":["<token>"]}}, PutTargets the queue, PutEvents that source/token, and confirm the envelope arrives on the queue. Delete the throwaway rule/target and drain both queues to empty.
6. PutRule WATCH with pattern {"source":["aws.batch"],"detail-type":["Batch Job State Change"],"detail":{"jobQueue":[<WATCHED ARN>]}} targeting the first queue, and PutRule SIBLING with the same pattern but <SIBLING ARN> targeting the second queue.
7. RegisterJobDefinition (type=container) and SubmitJob onto WATCHED.
8. ReceiveMessage on the first queue and look for an envelope with source="aws.batch", detail-type="Batch Job State Change" and detail.status="RUNNABLE" for the submitted jobId. Observed: none arrives; the check hangs and is cut at 60s.
9. TerminateJob(jobId, reason=<reason>) and look for the FAILED envelope carrying <reason> in detail.statusReason. Observed: none.
10. Poll the SIBLING queue over the same window and then SubmitJob onto SIBLING; its own RUNNABLE envelope is expected there. Observed: never reached, because step 8 never completes.
