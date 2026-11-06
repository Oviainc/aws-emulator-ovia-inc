Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__events__lambda.xsvc-cloudformation-3-rollback-complete-status-event-drives-lambda-dynamodb-write cloudformation__dynamodb__events__lambda.xsvc-cloudformation-3-update-rollback-complete-rejected-by-exact-rollback-complete-rule
Issue-Key: cloudformation__dynamodb__events__lambda.xsvc-cloudformation-3-rollback-complete-status-event-drives-lambda-dynamodb-write|_wait_item#20|Timeout|time.sleep(3.0)
Issue-Key: cloudformation__dynamodb__events__lambda.xsvc-cloudformation-3-update-rollback-complete-rejected-by-exact-rollback-complete-rule|_poll_item#23|Timeout|time.sleep(3.0)
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# cloudformation__dynamodb__events__lambda: CloudFormation ROLLBACK_COMPLETE / UPDATE_ROLLBACK_COMPLETE status events never drive their EventBridge rule, so both checks hang

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause behind 2 failing checks.

Cause 1 of 1: a CloudFormation stack reaching a rollback terminal status emits no EventBridge `CloudFormation Stack Status Change` event that a rule can match, so the downstream Lambda/DynamoDB write never happens and the checks exhaust their 60 s budget.
AWS: when a stack enters `ROLLBACK_COMPLETE` (or `UPDATE_ROLLBACK_COMPLETE`), CloudFormation publishes an event on the default bus with `source` = `aws.cloudformation`, `detail-type` = `CloudFormation Stack Status Change` and a detail whose `status-details.status` is exactly that status string; a rule whose pattern names the exact status matches only that status and no other. Any rule pattern naming `UPDATE_ROLLBACK_COMPLETE` must NOT match a plain `ROLLBACK_COMPLETE` event. Targets then fire (here a Lambda that writes a DynamoDB item). Model: EventBridge PutEvents/PutRule/PutTargets are modelled in services/service-com-amazonaws-eventbridge-awsevents; the CloudFormation status-change event envelope itself is recorded behavior only — the recording shows the rule matching and the target running.
Emulator: both checks end `Error: Check timed out after 60.0s`. No status-change event is ever observable, so neither the positive delivery nor the exact-status negative discrimination can be graded.
Why: a client that reacts to stack rollbacks through EventBridge — the standard way to alarm on failed deployments — never gets notified, and an exact-status rule cannot be shown to be exact, so over-broad rules go undetected.
Fix: the CloudFormation stack state machine's event emission (publishing Stack Status Change events to the default bus on terminal transitions) and the EventBridge rule-matching/target-invocation path for them.
Traces: the two failing checks in trace_ids, under `### cloudformation__dynamodb__events__lambda`.

## Evidence
Expected (AWS): on ROLLBACK_COMPLETE an `aws.cloudformation` / `CloudFormation Stack Status Change` event is delivered to the matching rule within seconds and the Lambda target writes its DynamoDB item; a rule keyed on the exact string `UPDATE_ROLLBACK_COMPLETE` does not fire for `ROLLBACK_COMPLETE`.

Actual (emulator):
- cloudformation__dynamodb__events__lambda.xsvc-cloudformation-3-rollback-complete-status-event-drives-lambda-dynamodb-write -> Error: Check timed out after 60.0s
- cloudformation__dynamodb__events__lambda.xsvc-cloudformation-3-update-rollback-complete-rejected-by-exact-rollback-complete-rule -> Error: Check timed out after 60.0s

## Steps to Reproduce
1. PutRule on the default bus with pattern {"source":["aws.cloudformation"],"detail-type":["CloudFormation Stack Status Change"],"detail":{"status-details":{"status":["ROLLBACK_COMPLETE"]}}} and a Lambda target that writes to DynamoDB.
2. CreateStack with a template that fails, so the stack rolls back to ROLLBACK_COMPLETE.
3. Poll the DynamoDB table. Expected: an item within seconds. Observed: nothing, the check runs out its 60 s budget.
