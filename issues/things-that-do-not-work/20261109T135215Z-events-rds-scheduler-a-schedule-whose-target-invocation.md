Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__rds__scheduler.xsvc-rds-3-scheduler-stop-db-instance-denied-without-permission-then-stops-with-it
Issue-Key: events__rds__scheduler.xsvc-rds-3-scheduler-stop-db-instance-denied-without-permission-then-stops-with-it|_await_message#27|AssertionError|raise AssertionError(f"{description}: nothing arrived within {timeout_s:.0f}s")
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# events__rds__scheduler: a schedule whose target invocation is denied never writes to its DeadLetterConfig queue

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Cause 1 of 1: a permission-denied EventBridge Scheduler invocation produces no dead-letter message.

AWS: when a schedule fires and the target invocation is refused because the schedule's execution role lacks the action (here rds:StopDBInstance), Scheduler retries per its RetryPolicy and then delivers the failed invocation to the queue named in the target's DeadLetterConfig.Arn; the SQS message carries the failed invocation's details. Recorded behavior only for the message body; the delivery itself is the requirement.

Emulator: nothing arrives on the DLQ within the check's 210 s window (the schedule's fire offset plus its delivery budget), so the denied invocation leaves no trace anywhere.

Why: a client that monitors a schedule's DLQ to learn that its target invocations are failing - the only signal Scheduler gives for a denied target - sees an empty queue and concludes the schedule succeeded.

Fix: the scheduler service's invocation path: it must attempt the target call under the schedule's role, classify the refusal, and emit the failed invocation to the target's DeadLetterConfig queue.

Traces: events__rds__scheduler.xsvc-rds-3-scheduler-stop-db-instance-denied-without-permission-then-stops-with-it.

## Evidence
Expected: a message on the schedule's dead-letter queue describing the denied rds:StopDBInstance invocation.
Actual: AssertionError: the denied stopDBInstance reaching the schedule DLQ: nothing arrived within 210s.

## Steps to Reproduce
1. Create an SQS DLQ and a scheduler execution role with no rds:StopDBInstance permission.
2. CreateSchedule with a universal target invoking rds StopDBInstance, that role, and DeadLetterConfig pointing at the DLQ, firing in the near future.
3. Poll the DLQ: no message arrives.
