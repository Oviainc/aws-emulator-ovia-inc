Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue
Issue-Key: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue|run#46|Timeout|time.sleep(10)
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# AWS Batch publishes no 'Batch Job State Change' event to EventBridge, so a matching rule's queue stays empty and the near-miss negative is ungradeable (1 cause)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Cause 1 of 1 — a Batch job's state transitions emit no event on the default event bus.
AWS: as a job moves through SUBMITTED -> PENDING -> RUNNABLE -> STARTING -> RUNNING -> SUCCEEDED/FAILED, AWS Batch publishes `aws.batch` events with detail-type "Batch Job State Change" to the account's default event bus, carrying the job id, job name, job queue, job definition and the new status. A rule matching those fields delivers each transition to its target, and a rule that differs (a different job queue or status) receives none of them.
Emulator: no event is ever delivered. The check completes CreateComputeEnvironment/CreateJobQueue/RegisterJobDefinition, CreateQueue, SetQueueAttributes, PutRule and PutTargets (FailedEntryCount 0) and SubmitJob, then polls the matching rule's queue; the message never arrives and the replay cuts the check off at 60 s. With no positive control, the companion assertion that the non-matching rule's queue stays empty is satisfied for the wrong reason and the check fails.
Why: Batch job completion is almost always consumed through EventBridge rather than by polling DescribeJobs. A client gets a job it can see progressing and a rule reporting healthy, with no notification in between, and no way to tell correct rule filtering from total non-delivery.
Fix: the AWS Batch job-state-change event publisher — the code that must emit an `aws.batch` "Batch Job State Change" event on each job status transition in services/service-com-amazonaws-batch-awsbatchv20160810 — and the EventBridge default-bus ingestion of those events.
Traces: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue.

## Evidence
"Check timed out after 60.0s". The preceding calls all returned 2xx: the compute environment, job queue and job definition were created, two SQS queues were created with policies admitting their rule ARNs, PutRule and PutTargets returned FailedEntryCount 0, and SubmitJob returned a jobId. The terminal read — ReceiveMessage on the matching rule's queue — never returns a message. Expected: at least one `aws.batch` envelope with detail-type "Batch Job State Change" naming the submitted job on the matching rule's queue, and none on the other queue.

## Steps to Reproduce
1. CreateComputeEnvironment, CreateJobQueue(jq), RegisterJobDefinition(jd).
2. CreateQueue(qMatch) and CreateQueue(qOther); SetQueueAttributes on both with a policy admitting their rule ARNs.
3. PutRule(Name=rMatch, EventPattern='{"source":["aws.batch"],"detail-type":["Batch Job State Change"],"detail":{"jobQueue":[<jq arn>]}}') + PutTargets to qMatch; PutRule(Name=rOther) with a non-matching jobQueue + PutTargets to qOther.
4. SubmitJob(jobQueue=jq, jobDefinition=jd, jobName=n) -> ReceiveMessage(qMatch) never returns a message.
