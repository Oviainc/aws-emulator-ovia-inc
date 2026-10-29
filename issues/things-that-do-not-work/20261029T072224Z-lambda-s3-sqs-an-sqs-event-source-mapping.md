Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.cli-update-event-source-mapping-batch-size-positive
Issue-Key: lambda__s3__sqs.cli-update-event-source-mapping-batch-size-positive|_poll_batch_counts#15|TimeoutError|raise TimeoutError(f"{min_total} processed {marker!r} records: the run budget ended first; not graded")
Issue-Commit: 8fe1b88606c77dfa59d771b1a110885b6dfaed56

# lambda__s3__sqs: an SQS event source mapping never invokes its function, so no batch is ever processed before or after UpdateEventSourceMapping (1 cause)

> _Note: this issue was identified against the code base at commit `8fe1b88606c77dfa59d771b1a110885b6dfaed56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEventSourceMapping

## Description
ONE root cause. 1 CAUSE must be closed before this issue is submitted.

NOTE ON PRIOR ART: the open issue 20261025T022656Z-lambda-s3-sqs-sqs-event-source-mappings-never.md owns a sibling check with the same symptom but does NOT carry this check's replay key, so this failure is un-gated and a fixer closing that issue would never re-replay the UpdateEventSourceMapping arm. Filed so the batch-size-change arm is gated too; close together if one change fixes both.

Cause 1 of 1: messages sent to an SQS queue that is the event source of an enabled Lambda event source mapping are never delivered to the function, so the function writes no per-batch marker object to S3 and the effect of changing the mapping's BatchSize can never be observed. Failing check: lambda__s3__sqs.cli-update-event-source-mapping-batch-size-positive.

AWS: Smithy (services/service-com-amazonaws-lambda-awsgirapiservice) models UpdateEventSourceMapping{UUID required @length 36..36, BatchSize @range 1..10000, ...} -> EventSourceMappingConfiguration{UUID, BatchSize, EventSourceArn, FunctionArn, State, StateTransitionReason, LastProcessingResult, LastModified, Queues, ...} at PUT /2015-03-31/event-source-mappings/{UUID} with success status 202, errors InvalidParameterValueException (400), ResourceConflictException (409), ResourceInUseException (400), ResourceNotFoundException (404), ServiceException (500), TooManyRequestsException (429). The polling/invocation behavior itself is recorded behavior only: an ESM in State "Enabled" over an SQS queue polls the queue and invokes the function with an `event` whose `Records` list holds up to BatchSize messages; BatchSize is an upper bound per invocation, so with N messages in flight the number of invocations (and hence the number of markers the function writes) is bounded below by ceil(N / BatchSize). After UpdateEventSourceMapping changes BatchSize, subsequent polls honour the new bound. The requirement is that messages are delivered at all and that the per-invocation record count respects BatchSize; queue URLs, UUIDs, bucket names and marker keys are per-run values.

Emulator: the function is never invoked. The replay never observes even the first phase's three processed records, dying in `_poll_batch_counts` at line 368 with `TimeoutError: 3 processed 'phase1-' records: the run budget ended first; not graded` (reached from run() line 84). Every control-plane call upstream is accepted — CreateFunction, CreateQueue, CreateEventSourceMapping, SendMessage, and `aws lambda update-event-source-mapping --batch-size` — so the gap is the ESM's poll-and-invoke side effect, not request validation or the mapping record.

Why: SQS-triggered Lambda is one of the core AWS integration patterns. A client sees a mapping in State Enabled, messages accepted by SQS, and then nothing: no invocation, no function output, and no error anywhere. Messages are neither consumed nor visibly retained as unprocessed, so a consumer cannot tell a stalled pipeline from an idle one, and nothing that depends on BatchSize — batching cost models, partial-batch failure handling, FunctionResponseTypes — is observable.

Fix: the event-source-mapping poller in the lambda crate — the SQS poll-and-invoke loop that an Enabled mapping must run (receiving from the mapped queue, assembling up to BatchSize records into one invocation of the mapped function, deleting the messages on success) and the state the UpdateEventSourceMapping handler mutates so a BatchSize change takes effect on subsequent polls.

Traces: lambda__s3__sqs.cli-update-event-source-mapping-batch-size-positive (frames run() line 84 -> `_poll_batch_counts` line 368).

## Evidence
Expected (AWS): with an enabled SQS event source mapping at BatchSize=1 and three messages sent, the function is invoked and writes three 'phase1-' marker objects to the S3 bucket under batches/, each recording a one-record batch; after `aws lambda update-event-source-mapping --uuid <uuid> --batch-size <N>` (202, EventSourceMappingConfiguration echoing the new BatchSize), the next messages are delivered in batches of at most N.

Actual (emulator):
  [error] lambda__s3__sqs.cli-update-event-source-mapping-batch-size-positive
  File "check_lambda__s3__sqs_cli_update_event_source_mapping_batch_size_positive.py", line 84, in run -> counts_phase1, _ = _poll_batch_counts(ctx, bucket, "batches/", min_total=3, marker="phase1-", ...)
  File "...", line 368, in _poll_batch_counts -> raise TimeoutError(f"{min_total} processed {marker!r} records: the run budget ended first; not graded")
  TimeoutError: 3 processed 'phase1-' records: the run budget ended first; not graded
Zero marker objects under batches/ — the function was never invoked at all, so the phase-2 BatchSize comparison is unreachable.

Smithy: UpdateEventSourceMapping PUT /2015-03-31/event-source-mappings/{UUID} -> 202 EventSourceMappingConfiguration{UUID, BatchSize @range 1..10000, State, LastProcessingResult, Queues, ...}; errors InvalidParameterValueException 400, ResourceConflictException 409, ResourceInUseException 400, ResourceNotFoundException 404, ServiceException 500, TooManyRequestsException 429.

## Steps to Reproduce
1. Boot the emulator from mainline.
2. Create an S3 bucket, an SQS queue, and a Lambda function (with an execution role) whose handler writes one object per received record under batches/ keyed by a marker prefix.
3. CreateEventSourceMapping(EventSourceArn=<queue arn>, FunctionName=<fn>, BatchSize=1, Enabled=true).
4. SendMessage three 'phase1-' messages, then poll ListObjectsV2 on batches/: no object ever appears (expected three).
5. aws lambda update-event-source-mapping --uuid <uuid> --batch-size <N> (202), send more messages and poll again: still nothing.
6. Replay: lambda__s3__sqs.cli-update-event-source-mapping-batch-size-positive.
