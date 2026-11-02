Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda.cli-eventbridge-put-events-dynamodb-pipeline-workflow
Issue-Key: dynamodb__events__lambda.cli-eventbridge-put-events-dynamodb-pipeline-workflow|_poll_for_item#27|Timeout|time.sleep(interval_s)
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# dynamodb__events__lambda: a CLI-issued put-events entry never reaches the rule's Lambda target, so nothing is written to DynamoDB

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One net-new root cause.

Cause 1 of 1: an event published with `aws events put-events` matches a rule whose Lambda target should write the event's id and value into DynamoDB, but the target is never invoked, so GetItem never returns the item.
Failing check: dynamodb__events__lambda.cli-eventbridge-put-events-dynamodb-pipeline-workflow

AWS: PutEvents accepts the entry and returns an EventId with FailedEntryCount 0; the rule matching that entry invokes its Lambda target once with the envelope; the function's PutItem stores the detail's id and value; a subsequent GetItem on that key returns the item with the sent value. Smithy (eventbridge): PutEvents.Entries -> PutEventsResultEntry{EventId, ErrorCode, ErrorMessage} with FailedEntryCount, so per-entry acceptance is modeled; the invocation itself is recorded behavior only.
Emulator: the check ends `Check timed out after 60.0s`. The function is never invoked, so no item is written and GetItem keeps answering with no Item until the harness budget expires. The path is identical whether the publish is made with the CLI or an SDK, so the divergence is in the rule->Lambda delivery, not the CLI.
Why: the most common serverless wiring (publish an event, a function persists it) silently does nothing; a client sees an accepted PutEvents with an EventId and then no effect anywhere, with no error to act on.
Fix: the EventBridge rule-target delivery path for Lambda targets in the events crate (match -> resource-policy/role authorization -> Lambda Invoke) and the Lambda invoke path it drives. Open issue 20261029T180026Z-dynamodb-events-lambda-eventbridge-rules-never-invoke-their-lambda-targets-ddb.md owns most of this group's checks but not this replay key; 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md names the Invoke side.
Traces: dynamodb__events__lambda.cli-eventbridge-put-events-dynamodb-pipeline-workflow.

## Evidence
Expected: `aws events put-events` returns FailedEntryCount 0 with an EventId; the matching rule invokes the Lambda target; GetItem on the detail's id returns an item carrying the sent value.
Actual: `Check timed out after 60.0s` — the Lambda target is never invoked and the DynamoDB item never appears.

## Steps to Reproduce
1. Create a DynamoDB table and a Lambda function (execution role allowing dynamodb:PutItem) whose handler writes event.detail.id/value into the table.
2. Create a rule matching the chosen source/detail-type, PutTargets the function, and add a lambda resource-policy statement allowing events.amazonaws.com with SourceArn = the rule ARN.
3. Run `aws events put-events --entries ...` with a matching entry carrying a known id/value.
4. Poll GetItem for that id: on AWS the item appears; on the emulator it never does and the check times out.
