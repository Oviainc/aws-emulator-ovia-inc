Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__iam__stepfunctions.sfn-no-permission-to-write-ddb
Issue-Key: dynamodb__iam__stepfunctions.sfn-no-permission-to-write-ddb|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# Step Functions execution denied DynamoDB write never produces an outcome

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One net-new root cause in the dynamodb__iam__sfn group.

Cause 1 of 1: a state machine whose role lacks `dynamodb:PutItem` neither fails the execution nor writes the item.
AWS: the Task state's DynamoDB integration call is refused for the execution role, the execution ends `FAILED` with an access-denied cause, and no item is written.
Emulator: the check gets no outcome at all — `Check timed out after 60.0s` — so the execution neither fails nor succeeds observably.
Why: an authorization failure inside a workflow is invisible: the caller cannot distinguish a denied step from a hung execution, and cannot rely on the execution status to detect the misconfiguration.
Fix: the sfn Task-state DynamoDB integration (perform the call as the execution role and surface the refusal as the execution's failure).
Traces: dynamodb__iam__sfn.sfn-no-permission-to-write-ddb

## Evidence
[mismatch] dynamodb__iam__sfn.sfn-no-permission-to-write-ddb — Error: Check timed out after 60.0s.

## Steps to Reproduce
Boot the emulator and replay dynamodb__iam__sfn.sfn-no-permission-to-write-ddb: it starts an execution whose role cannot PutItem and waits for the execution to end.
