Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.cli-update-event-source-mapping-batch-size-positive lambda__s3__sqs.sqs-esm-batch-size-affects-invocation-count-positive
Issue-Key: lambda__s3__sqs.cli-update-event-source-mapping-batch-size-positive|_poll_batch_counts#15|Timeout|time.sleep(3)
Issue-Key: lambda__s3__sqs.sqs-esm-batch-size-affects-invocation-count-positive|_poll_reports#11|Timeout|time.sleep(interval_s)
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# lambda__s3__sqs: SQS event source mappings never consume, so BatchSize has no observable effect (1 cause)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEventSourceMapping

## Description
Cause 1 of 1: an SQS event source mapping never consumes messages, so neither the CLI BatchSize update nor the effect of BatchSize on invocation count can be observed; both checks are killed at the harness 60 s cap.

AWS: an enabled ESM on a standard queue polls it and invokes the function with up to BatchSize records per invocation, so sending N messages with BatchSize=k yields roughly ceil(N/k) invocations; UpdateEventSourceMapping(BatchSize=...) changes that grouping for subsequent polls. Recorded behavior only for the poller delay.

Emulator: no message is ever consumed and no invocation happens — both checks report 'Error: Check timed out after 60.0s'. Messages stay in the queue and the mapping's configuration has no observable consequence.

Why: the whole SQS-to-Lambda integration is inert: a client sees a healthy mapping and a filling queue, no invocations, and no error anywhere that explains it. Tuning BatchSize is meaningless.

Fix: the SQS event-source-mapping poller in the lambda service (receive, batch per BatchSize, invoke, delete on success). The open issue 20261024T043150Z-lambda-sqs-event-source-mapping-never-consumes-messages.md records this cause for other checks; neither of these two is listed there.

Traces: lambda__s3__sqs.cli-update-event-source-mapping-batch-size-positive, lambda__s3__sqs.sqs-esm-batch-size-affects-invocation-count-positive.

## Evidence
Expected: with BatchSize=k, N queued messages produce ceil(N/k) invocations, and an UpdateEventSourceMapping BatchSize change is reflected in subsequent batching.
Actual: Error: Check timed out after 60.0s (both checks) — nothing is consumed.

## Steps to Reproduce
1. Create a queue and a function; CreateEventSourceMapping with BatchSize=1 and wait for Enabled.
2. SendMessage several messages and poll for invocations.
3. UpdateEventSourceMapping(BatchSize=10) and repeat.
4. Observe no invocations in either case.
