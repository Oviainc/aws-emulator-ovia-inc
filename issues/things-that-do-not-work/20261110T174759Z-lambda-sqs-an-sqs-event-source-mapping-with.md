Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs.batch-item-failures-ignored-without-functionresponsetypes
Issue-Key: lambda__sqs.batch-item-failures-ignored-without-functionresponsetypes|_receive#17|Timeout|got = sqs.receive_message(
Issue-Commit: ef9cd1f472238b59a7ccf5edc608de1fa0253c97

# lambda__sqs: an SQS event source mapping with ReportBatchItemFailures never returns the named message, so the poison message never reaches the source queue's redrive target

> _Note: this issue was identified against the code base at commit `ef9cd1f472238b59a7ccf5edc608de1fa0253c97`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause (1 of 1).

Cause 1 of 1: the control arm of the batchItemFailures check never completes. An SQS event source mapping created with FunctionResponseTypes=['ReportBatchItemFailures'] over a source queue with RedrivePolicy maxReceiveCount=3 and VisibilityTimeout=10 must return only the message named in the handler's batchItemFailures response, redeliver it until maxReceiveCount is reached and then let SQS move it to the redrive target. Nothing reaches that redrive target, so the whole comparison against the twin arm (an identical mapping WITHOUT FunctionResponseTypes, whose identical response body must be inert) is unobservable.

AWS: recorded behavior only for the polling runtime — Smithy (services/service-com-amazonaws-lambda-awsgirapiservice) models CreateEventSourceMapping with FunctionResponseTypes (enum ReportBatchItemFailures), BatchSize and State, and states no polling semantics. The recording shows: with ReportBatchItemFailures declared, the records NOT named in batchItemFailures are deleted (each seen exactly once) and the named record's visibility is returned so it is redelivered at least once and at most maxReceiveCount times, after which SQS redrives exactly that one message to the DLQ; without FunctionResponseTypes the identical response body is inert and all five records are deleted after one delivery each.

Emulator: the check is killed at the harness's 60s budget ("Error: Check timed out after 60.0s") while waiting for arm A's poison message on its redrive target — a wait the check allows 420s for. Neither the per-record sighting tally nor the redrive is ever produced, so the mapping's partial-batch handling and the queue's redrive do not progress.

Why: partial-batch responses are the standard way to avoid reprocessing a whole batch for one bad record. A client gets neither the selective return nor the eventual redrive, so poison messages are neither isolated nor dead-lettered and the difference FunctionResponseTypes is supposed to make is invisible.

Fix: the lambda crate's SQS event-source poller (services/service-com-amazonaws-lambda-awsgirapiservice) — the handling of the handler's batchItemFailures response (delete the unnamed records, return the named one's visibility) and its interaction with the source queue's RedrivePolicy/maxReceiveCount redrive in the sqs crate.

Related but not covering: open issue 20261109T185037Z-lambda-sqs-3-net-new-causes-dotnet-go-java-ruby.md covers the same scenario for the .NET/Go/Java/Ruby handlers and carries those replay keys; 20261107T014900Z-lambda-sqs-the-asynchronous-invocation-plane-is-inert.md is about the async invocation plane. Neither carries this check's key.

Traces: lambda__sqs.batch-item-failures-ignored-without-functionresponsetypes.

## Evidence
Expected: arm A (FunctionResponseTypes=['ReportBatchItemFailures']) — each of the four ok bodies seen exactly once, the poison body seen at least twice and at most maxReceiveCount=3 times, and the redrive target receiving exactly the poison message; arm B (no FunctionResponseTypes) — all five bodies seen exactly once and its DLQ empty.
Actual: "Check timed out after 60.0s" — the wait for the poison message on arm A's redrive target (budgeted 420s in the check) never completes, so no tally or redrive was ever observed.

## Steps to Reproduce
1. Create five queues: a counting queue, two DLQs, and two source queues each with RedrivePolicy {maxReceiveCount: 3} to their own DLQ and VisibilityTimeout 10.
2. Deploy two identical python functions that forward every record to the counting queue and return {"batchItemFailures":[{"itemIdentifier": <poison messageId>}]}.
3. CreateEventSourceMapping for arm A with FunctionResponseTypes=['ReportBatchItemFailures'] and for arm B without it, BatchSize 5; wait for both State=Enabled.
4. SendMessage four ok bodies and one 'poison' body to each source queue; poll the counting queue and arm A's DLQ.
5. The poison message never reaches arm A's DLQ. Replay lambda__sqs.batch-item-failures-ignored-without-functionresponsetypes.
