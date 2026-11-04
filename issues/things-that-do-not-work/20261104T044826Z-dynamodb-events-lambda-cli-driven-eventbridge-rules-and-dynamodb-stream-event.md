Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda.cli-create-eventbridge-rule-lambda-target-workflow dynamodb__events__lambda.cli-eventbridge-rule-missing-permission-negative-workflow dynamodb__events__lambda.cli-lambda-esm-dynamodb-stream-workflow
Issue-Key: dynamodb__events__lambda.cli-create-eventbridge-rule-lambda-target-workflow|run#20|AssertionError|assert found, f"expected marker {token} to appear in Lambda logs after EventBridge delivery"
Issue-Key: dynamodb__events__lambda.cli-eventbridge-rule-missing-permission-negative-workflow|_poll_logs_for_marker#23|Timeout|time.sleep(interval_s)
Issue-Key: dynamodb__events__lambda.cli-lambda-esm-dynamodb-stream-workflow|_poll_logs_for_marker#21|Timeout|time.sleep(interval_s)
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# CLI-driven EventBridge rules and DynamoDB-stream event source mappings never invoke their Lambda target

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Two net-new root causes. (Open issue 20261029T180026Z-dynamodb-events-lambda-eventbridge-rules-never-invoke-their-lambda-targets-ddb.md owns sibling checks; it does not carry these three replay keys.)

Cause 1 of 2: an EventBridge rule whose target is a Lambda never invokes it, and the missing-permission twin is therefore ungradable.
AWS: recorded behavior only — the recording creates the rule with PutRule, attaches the function with PutTargets, grants lambda:InvokeFunction to events.amazonaws.com with a SourceArn of the rule, calls PutEvents with a matching event, and then finds a per-run marker in the function's /aws/lambda/<name> log group.
Emulator: PutRule/PutTargets/AddPermission/PutEvents are all accepted, but the function is never invoked: the check fails with "AssertionError: expected marker <marker> to appear in Lambda logs after EventBridge delivery", and the missing-permission twin merely times out after 60s because its positive control never fires either.
Why: a client that wires an event-driven handler through EventBridge sees every control-plane call succeed and no handler ever run; the denial path is indistinguishable from the delivery path, so an IAM mistake cannot be diagnosed.
Fix: the EventBridge PutEvents dispatcher that matches rules and invokes Lambda targets, and the resource-policy evaluation on that invoke.
Traces: dynamodb__events__lambda.cli-create-eventbridge-rule-lambda-target-workflow, .cli-eventbridge-rule-missing-permission-negative-workflow.

Cause 2 of 2: a DynamoDB-stream event source mapping created through the CLI never polls.
AWS: recorded behavior only — the recording enables a stream on the table, creates the mapping with CreateEventSourceMapping, writes an item and then observes the handler's record log line.
Emulator: the check is cut off with "Error: Check timed out after 60.0s" — the mapping is created and reported Enabled, but no batch is ever delivered, so the poll never terminates.
Why: a client's stream consumer silently processes nothing while the mapping reports itself healthy.
Fix: the Lambda event-source-mapping poller for DynamoDB Streams and the stream shard iterator it reads from.
Traces: dynamodb__events__lambda.cli-lambda-esm-dynamodb-stream-workflow.

## Evidence
Expected (AWS): the EventBridge marker appears in the target function's log group; the DynamoDB-stream mapping delivers the written item to the handler.
Actual (emulator): 'AssertionError: expected marker marker-<id> to appear in Lambda logs after EventBridge delivery'; the other two checks end with 'Error: Check timed out after 60.0s'.

## Steps to Reproduce
1. Create a Lambda, PutRule with an event pattern, PutTargets naming the function, AddPermission for events.amazonaws.com with the rule ARN as SourceArn.
2. PutEvents with a matching event and poll the function's log group for a marker. Observe it never appears.
3. Enable a DynamoDB stream, CreateEventSourceMapping to a Lambda, PutItem, and poll the handler's logs. Observe nothing is delivered within 60s.
