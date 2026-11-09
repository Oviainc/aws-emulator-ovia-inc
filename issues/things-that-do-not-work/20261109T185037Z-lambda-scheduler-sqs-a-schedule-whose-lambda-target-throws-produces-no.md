Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-throwing-lambda-fails-to-onfailure-not-scheduler-dlq
Issue-Key: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-throwing-lambda-fails-to-onfailure-not-scheduler-dlq|_await_message#23|AssertionError|raise AssertionError(f"{description}: nothing arrived within {timeout_s:.0f}s")
Issue-Commit: b4090e4eff86c6645a745aa23fbd4b95671a37a2

# A schedule whose Lambda target throws produces no dead-letter event on the schedule's DLQ

> _Note: this issue was identified against the code base at commit `b4090e4eff86c6645a745aa23fbd4b95671a37a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Cause 1 of 1: when a scheduled Lambda invocation fails inside the function, the barrier schedule's dead-letter event never arrives on the configured DeadLetterConfig queue.

AWS: EventBridge Scheduler retries a failed target invocation per its RetryPolicy and then writes the invocation to the schedule's DeadLetterConfig.Arn queue; a function error routes to the FUNCTION's OnFailure destination rather than the scheduler DLQ, which is exactly the distinction this check draws — and it uses a second ('barrier') schedule whose own dead-letter event must appear to prove the DLQ path works at all.
Emulator: 'the barrier schedule's dead-letter event: nothing arrived within 160s' — the scheduler dead-letter path produces nothing, so neither side of the distinction can be observed.
Why: a client gets no evidence that a scheduled invocation failed: nothing to replay, nothing to alarm on, and no way to tell a schedule that never fired from one whose target failed.
Fix: the EventBridge Scheduler target-invocation failure path (retry, then write to DeadLetterConfig.Arn).
Traces: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-throwing-lambda-fails-to-onfailure-not-scheduler-dlq. Related open issues filed for sibling checks: 20261109T141116Z-lambda-scheduler-sqs-scheduler-never-dead-letters-a-failed-lambda-target.md.

## Evidence
[mismatch] lambda__scheduler__sqs.xsvc-lambda-3-scheduler-throwing-lambda-fails-to-onfailure-not-scheduler-dlq
AssertionError: the barrier schedule's dead-letter event: nothing arrived within 160s

## Steps to Reproduce
1. CreateSchedule with a Lambda target, a RetryPolicy and DeadLetterConfig pointing at an SQS queue (plus a barrier schedule whose target is guaranteed to fail delivery).
2. Let the schedule fire against a function that throws.
3. Poll the DLQ — expect the barrier schedule's dead-letter event; the emulator delivers nothing.
