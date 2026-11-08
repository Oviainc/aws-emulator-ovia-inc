Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue
Issue-Key: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue|run#46|Timeout|time.sleep(10)
Issue-Commit: db763654427c553d8b06d764576848aa87eee804

# batch: a Batch job's lifecycle transitions publish no `Batch Job State Change` event to EventBridge, so a rule pinned to the job queue ARN never delivers to its SQS target

> _Note: this issue was identified against the code base at commit `db763654427c553d8b06d764576848aa87eee804`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
One net-new root cause. The failing check was reported neither ALREADY FILED nor OWNED by an open issue, so no open work item carries this run's replay key for it. I searched the open/fixed issue set for a Batch eventing issue (list_existing_issues query='batch', 342 entries) and found none: the existing `batch` issues cover the quota-share/identifier plane, refusal wording, tag-mutation status codes and DeleteQuotaShare stubs — none of them covers Batch's EventBridge publication path.

=== Cause 1 of 1: AWS Batch emits no `aws.batch` / `Batch Job State Change` event when a job changes state ===
AWS: recorded behavior only for the event vocabulary — the EventBridge event body is not described by any Smithy model. Smithy (batch) does model the state this event is a projection of: SubmitJob returns jobId/jobArn/jobName, DescribeJobs reports jobName, jobArn, jobQueue, jobDefinition, status and createdAt (epoch millis), and TerminateJob takes a `reason`. AWS Batch publishes one EventBridge event per job-state TRANSITION on the default bus with source `aws.batch`, detail-type `Batch Job State Change`, `account` = the caller's account, `region` = the calling region, `resources` = exactly [the job ARN], and a `detail` that is a projection of the job record: detail.jobId, detail.jobArn, detail.jobQueue (the job queue ARN), detail.jobName, detail.jobDefinition (an ARN), detail.createdAt (epoch-millis integer) and detail.status set to the state the job transitioned INTO. The initial SUBMITTED state is documented as emitting no event, so for a job on a queue backed by an UNMANAGED compute environment the first event carries status RUNNABLE; a subsequent TerminateJob produces a second event with status FAILED whose detail.statusReason carries the reason string passed to TerminateJob. The events are routable by content: a rule whose pattern is {"source":["aws.batch"],"detail-type":["Batch Job State Change"],"detail":{"jobQueue":[<queue ARN>]}} must receive only the events of jobs submitted to that queue, and a sibling rule pinned to a different job queue ARN must receive nothing.
Emulator: nothing observable is ever produced for this pipeline. The check builds the whole wiring (IAM Batch service role, an UNMANAGED compute environment polled to VALID, two job queues polled to VALID, two SQS queues with policies admitting events.amazonaws.com, two ENABLED default-bus rules each pinned to one job queue ARN, plus a per-queue warm-up rule on a source the check controls to prove the EventBridge->SQS leg can deliver at all), submits a job, waits for DescribeJobs to report a jobArn, then polls the watched SQS queue for the RUNNABLE `Batch Job State Change` envelope. No envelope with source `aws.batch` and detail-type `Batch Job State Change` ever arrives and the check produces no terminal verdict: the suite reports `mismatch` with `Check timed out after 60.0s`. Every later graded observation is therefore unreachable too: the envelope field assertions (source/detail-type/account/region/resources == [jobArn]), the detail projection assertions (detail.jobName and detail.createdAt must equal what DescribeJobs reports for the same job), the absence leg on the sibling queue (no stray event, 0 visible and 0 in-flight messages), the second transition after TerminateJob (status FAILED with detail.statusReason containing the TerminateJob reason), the "no SUBMITTED event is ever created" assertion, and the repair leg in which a job submitted to the sibling queue does produce a RUNNABLE event there naming the sibling queue ARN.
Why: a client that drives Batch asynchronously is blind. Job completion/failure fan-out (notify on FAILED, chain the next stage on SUCCEEDED, dead-letter on a terminated job) is built on these events, and they are the only push signal Batch offers; without them a consumer must fall back to polling DescribeJobs, and any EventBridge rule, Lambda target, Step Functions callback or alarm wired to `aws.batch` silently never fires. Because the emulator emits nothing at all rather than emitting something wrong, a mis-scoped rule pattern and a correct one are indistinguishable, so routing/filtering logic cannot be validated against it either.
Fix: the Batch job state machine (the SubmitJob / scheduler-sweep / TerminateJob transition path and the job record it owns) must publish a `Batch Job State Change` event onto the default event bus for every transition after SUBMITTED, with source `aws.batch`, resources = [job ARN], and a detail projected from the same job record DescribeJobs serves (so the two can never disagree), including statusReason on a terminate-driven FAILED transition.
Traces: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue

## Evidence
From this behavioral replay (266/766 passed; batch__events__sqs 0/2 — the other check in that service is marked ALREADY FILED, blocked by the unimplemented EC2 `CreateInternetGateway` stub, and this is the one the suite marked neither ALREADY FILED nor OWNED).

batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue [mismatch]:
  Error: Check timed out after 60.0s

Expected (AWS): after SubmitJob onto a job queue backed by an UNMANAGED compute environment, an envelope arrives on the SQS queue targeted by the rule pinned to that job queue's ARN:
  {"source": "aws.batch", "detail-type": "Batch Job State Change", "account": <caller account>, "region": <calling region>, "resources": [<job ARN>], "detail": {"jobId": <jobId from SubmitJob>, "jobArn": <jobArn from DescribeJobs>, "jobQueue": <watched job queue ARN>, "jobName": <same as DescribeJobs>, "jobDefinition": "arn:...", "createdAt": <same epoch-millis integer DescribeJobs reports>, "status": "RUNNABLE"}}
  then, after TerminateJob(jobId, reason=<reason>), a second envelope for the same jobId with detail.status == "FAILED" and detail.statusReason containing <reason>; and never an envelope with detail.status == "SUBMITTED".
  The sibling rule, pinned to a DIFFERENT job queue ARN, must receive nothing while no job has been submitted there (0 visible / 0 in-flight on its queue), and must receive the RUNNABLE event once a job IS submitted to that sibling queue.
Actual (emulator): no envelope with source "aws.batch" / detail-type "Batch Job State Change" is ever delivered; the check never reaches a verdict and is cut off at the harness deadline. The field values quoted above are per-run generated ids/ARNs/timestamps, not requirements — the requirement is the envelope's fixed source and detail-type strings, resources == [the job ARN], and a detail whose jobId/jobArn/jobQueue/jobName/createdAt agree with DescribeJobs for the same job and whose status is the state transitioned into.

Note on the positive control in the same check: before any Batch event is used as evidence, the check proves each SQS wiring can deliver by publishing its own event through a throwaway rule and measuring the latency. Only the Batch-sourced events are missing.

## Steps to Reproduce
1. Create an IAM role trusting batch.amazonaws.com with AWSBatchServiceRole attached; wait until GetRole resolves it.
2. batch CreateComputeEnvironment type=UNMANAGED with that service role; poll DescribeComputeEnvironments until status == VALID.
3. batch CreateJobQueue twice (state=ENABLED, priority=10, computeEnvironmentOrder = [that CE]); poll DescribeJobQueues until each is VALID. Keep both jobQueueArns (call them WATCHED and SIBLING).
4. Create two SQS queues; set each queue's Policy to allow Principal {Service: events.amazonaws.com} sqs:SendMessage on its own ARN.
5. events PutRule (default bus, State=ENABLED) with EventPattern {"source":["aws.batch"],"detail-type":["Batch Job State Change"],"detail":{"jobQueue":[WATCHED]}} and PutTargets the first queue; repeat with SIBLING and the second queue. Both PutTargets must return FailedEntryCount == 0.
6. (Control) PutRule a throwaway rule on a source you control, PutEvents one event and confirm it reaches each queue, so the EventBridge->SQS leg is proven; then drain both queues to empty.
7. batch RegisterJobDefinition, then SubmitJob onto WATCHED; poll DescribeJobs until it reports a jobArn.
8. ReceiveMessage from the first queue.
   Expected: an envelope with source "aws.batch", detail-type "Batch Job State Change", resources == [the jobArn] and detail.status == "RUNNABLE", whose detail.jobName/detail.createdAt equal what DescribeJobs reports.
   Actual: nothing with that source/detail-type ever arrives.
9. Hold the second (sibling) queue for a window calibrated on the measured delivery latency: expected empty with ApproximateNumberOfMessages == 0 and ApproximateNumberOfMessagesNotVisible == 0.
10. batch TerminateJob(jobId, reason=<reason>). Expected a second envelope for the same jobId with detail.status == "FAILED" and detail.statusReason containing <reason>; no envelope may ever carry detail.status == "SUBMITTED".
11. SubmitJob onto SIBLING. Expected its RUNNABLE event arrives on the second queue with detail.jobQueue == SIBLING, and the first job's events have still never appeared there.
