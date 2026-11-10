Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__lambda__pipes__sqs.kinesis-enrichment-failure-dead-letters-to-sqs-with-documented-payload
Issue-Key: kinesis__lambda__pipes__sqs.kinesis-enrichment-failure-dead-letters-to-sqs-with-documented-payload|run#21|TimeoutError|raise _expired(ctx, "a dead letter on the pipe's DLQ", dlq_wait_s, "the DLQ stayed empty")
Issue-Commit: ef9cd1f472238b59a7ccf5edc608de1fa0253c97

# kinesis__lambda__pipes__sqs: an enrichment failure on a Kinesis-source pipe never writes a dead-letter record to the pipe's SQS DLQ

> _Note: this issue was identified against the code base at commit `ef9cd1f472238b59a7ccf5edc608de1fa0253c97`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
One net-new root cause (1 of 1).

Cause 1 of 1: a pipe with a Kinesis source, a Lambda enrichment that fails, and an SQS DeadLetterConfig target produces no dead-letter record at all — the DLQ stays empty for the whole run.

AWS: recorded behavior only for the runtime — Smithy (services/service-com-amazonaws-pipes-pipes) models SourceParameters.KinesisStreamParameters.DeadLetterConfig.Arn and the retry bounds (MaximumRetryAttempts/MaximumRecordAgeInSeconds) but states no runtime semantics. The recording shows that when the enrichment stage keeps failing, the pipe exhausts the configured retries and then writes one dead-letter record to the DeadLetterConfig queue describing the failed batch — a JSON document naming the failure (the pipe's ARN, the stage that failed, the batch's shard id and starting/ending sequence numbers and the error) — and the record is never delivered to the target.

Emulator: the DLQ is never written. The check waits for a dead letter on the pipe's DLQ and ends with TimeoutError: "a dead letter on the pipe's DLQ: the run budget ended first; not graded" — "the DLQ stayed empty".

Why: a failed enrichment is silently swallowed. The customer's only designed-in signal for poison records — the dead-letter record and its documented payload — never arrives, so failures are undetectable and unreplayable.

Fix: the pipes crate's enrichment stage and its failure path (services/service-com-amazonaws-pipes-pipes) — retry exhaustion for a Kinesis source batch and the DeadLetterConfig writer that must publish the batch-failure document to the configured SQS queue.

Related but not covering: open issue 20261109T174004Z-kinesis-lambda-pipes-sqs-an-enrichment-failure-on.md describes the same shape but does not carry this check's replay key.

Traces: kinesis__lambda__pipes__sqs.kinesis-enrichment-failure-dead-letters-to-sqs-with-documented-payload.

## Evidence
Expected: with the enrichment Lambda failing every invocation, the pipe exhausts its retries and the DeadLetterConfig SQS queue receives exactly one dead-letter record whose body documents the failed batch (failed stage, shard id, sequence-number range and the enrichment error); the target receives nothing.
Actual: TimeoutError "a dead letter on the pipe's DLQ: the run budget ended first; not graded" — the DLQ stayed empty for the whole wait (check_kinesis__lambda__pipes__sqs_...py line 87).

## Steps to Reproduce
1. CreateStream (Kinesis), CreateQueue twice (target and DLQ), and deploy a Lambda enrichment that always raises.
2. CreatePipe with Source=<stream arn>, SourceParameters.KinesisStreamParameters{StartingPosition, MaximumRetryAttempts: small, DeadLetterConfig:{Arn:<dlq arn>}}, Enrichment=<failing function arn>, Target=<target queue arn>; StartPipe and wait for RUNNING.
3. PutRecord a payload; poll the DLQ.
4. Nothing arrives. Replay kinesis__lambda__pipes__sqs.kinesis-enrichment-failure-dead-letters-to-sqs-with-documented-payload.
