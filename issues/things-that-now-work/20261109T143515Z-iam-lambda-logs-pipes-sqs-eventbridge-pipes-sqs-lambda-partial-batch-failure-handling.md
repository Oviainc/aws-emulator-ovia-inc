Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__logs__pipes__sqs.sqs-source-lambda-target-batch-item-failures-redeliver-only-reported-items iam__lambda__logs__pipes__sqs.sqs-source-lambda-target-unrecognized-item-identifier-redelivers-whole-batch
Issue-Key: iam__lambda__logs__pipes__sqs.sqs-source-lambda-target-batch-item-failures-redeliver-only-reported-items|_collect_tally#14|Timeout|return {idx: frozenset(v) for idx, v in tally.items()}
Issue-Key: iam__lambda__logs__pipes__sqs.sqs-source-lambda-target-unrecognized-item-identifier-redelivers-whole-batch|_collect_tally#13|Timeout|return {idx: frozenset(v) for idx, v in tally.items()}
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# EventBridge Pipes SQS→Lambda partial-batch-failure handling never settles: both redelivery checks time out

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Cause 1 of 1: a pipe with an SQS source and a Lambda target never completes the batch-item-failure cycle the checks wait on.
AWS: a pipe reading an SQS queue and invoking a Lambda target honours the handler's batchItemFailures report — only the item identifiers the handler reports are redelivered, and an unrecognized identifier causes the whole batch to be redelivered (Smithy pipes CreatePipe with Source=SQS, Target=Lambda).
Emulator: neither behaviour is ever observed; both checks end with 'Check timed out after 180.0s', i.e. the pipe never delivers, never invokes, or never redelivers within the budget.
Why: a client cannot rely on (or test) partial batch failure semantics; messages are either lost or endlessly stuck with no observable outcome.
Fix: the pipe's SQS poller and Lambda target invocation path (batchItemFailures handling and message visibility) in the pipes crate.
Traces: both checks in trace_ids.

## Evidence
Expected: only reported item identifiers are redelivered; an unrecognized identifier redelivers the whole batch. Actual: 'Error: Check timed out after 180.0s' for both checks.

## Steps to Reproduce
Replay iam__lambda__logs__pipes__sqs.sqs-source-lambda-target-batch-item-failures-redeliver-only-reported-items and .sqs-source-lambda-target-unrecognized-item-identifier-redelivers-whole-batch.
