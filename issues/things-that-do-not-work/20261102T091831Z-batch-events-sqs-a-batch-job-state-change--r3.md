Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue
Issue-Key: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue|_drain#47|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# batch__events__sqs: a Batch Job State Change event never reaches the matching rule's queue, so the matched/withheld differential is ungradeable

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
One net-new root cause.

Cause 1 of 1: a submitted AWS Batch job's state changes publish nothing that reaches an EventBridge rule's SQS target, so neither the matched delivery nor the withheld near-miss can be observed.
Failing check: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue

AWS: as a Batch job moves through its statuses, Batch publishes events with source `aws.batch` and detail-type `Batch Job State Change`, whose detail carries the job's jobId, jobName, jobQueue, jobDefinition and status. A rule whose pattern selects that source/detail-type (and, here, scopes to the job's own queue or status) delivers the envelope to its SQS target; a second rule scoped to a different value receives nothing for the same job. Recorded behavior only for the envelope contents (the Batch Smithy model covers SubmitJob/DescribeJobs, not the EventBridge event).
Emulator: the check ends `Check timed out after 60.0s`. The subscribed queue never receives a Batch Job State Change message, so the positive half never lands and the "withheld from the other queue" half is vacuous.
Why: a client that reacts to Batch job completion/failure through EventBridge is never notified, and a negative-delivery assertion ("the other queue must stay empty") passes on the emulator for the wrong reason and fails on AWS.
Fix: the Batch job state machine's event emission in the batch crate — every status transition must publish the aws.batch `Batch Job State Change` envelope onto the default bus with detail fields drawn from the same state DescribeJobs serves — plus the events rule matcher/SQS target delivery for those envelopes. Fixed issues 20261025T002357Z-batch-events-sqs-aws-batch-publishes-no-batch-job-state-change.md and 20261027T214908Z-batch-events-sqs-a-batch-job-state-change--r2.md claim this area; this check's replay key is not covered by either, and the path is still silent.
Traces: batch__events__sqs.batch-job-state-change-event-delivered-to-matching-rule-and-withheld-from-other-queue.

## Evidence
Expected: after SubmitJob, the rule matching the job's source/detail-type (and scope) delivers a Batch Job State Change envelope to its SQS target whose detail.jobId equals the submitted job id, while the second, differently-scoped rule's queue stays empty.
Actual: `Check timed out after 60.0s` — no Batch Job State Change message is delivered to either queue, so the matched half never lands.

## Steps to Reproduce
1. Create a Batch compute environment, job queue and job definition (with a Batch service role).
2. Create two SQS queues, each with a policy admitting events.amazonaws.com, and two default-bus rules on {"source":["aws.batch"],"detail-type":["Batch Job State Change"]} differing only in the scoping value (e.g. the job queue ARN), each targeting its own queue.
3. SubmitJob into the first queue and wait for DescribeJobs to report a terminal status.
4. Poll both queues: on AWS the matching rule's queue receives the state-change envelope and the other stays empty. On the emulator both stay empty and the check times out.
