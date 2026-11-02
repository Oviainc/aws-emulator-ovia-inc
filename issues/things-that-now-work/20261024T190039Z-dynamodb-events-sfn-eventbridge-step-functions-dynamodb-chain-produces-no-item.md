Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__stepfunctions.eventbridge-to-sfn-to-ddb-chain
Issue-Key: dynamodb__events__stepfunctions.eventbridge-to-sfn-to-ddb-chain|run#28|Timeout|time.sleep(3)
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# EventBridge -> Step Functions -> DynamoDB chain produces no item: the dynamodb:putItem Task state is never executed, so the chain's far-end effect never appears

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: the optimized DynamoDB integration Task state (arn:aws:states:::dynamodb:putItem) is accepted at CreateStateMachine but never run, so an EventBridge-started execution writes nothing and the chain's observable effect never materialises.
AWS: recorded behavior only — a matched event on a custom bus is delivered to the state-machine target with an assumable role holding states:StartExecution; the execution then runs the dynamodb:putItem Task, whose Parameters map $.detail.order_id into the item key, and the item becomes readable with GetItem within seconds.
Emulator: the check polls for the written item and exhausts the suite's 60 s budget with nothing in the table (Error: Check timed out after 60.0s). The same definition shape elsewhere in this suite reports error='States.Runtime' with cause "state '<name>' is of type 'Task', which this emulator does not execute, so nothing was run for it", which is the Hypothesis for the missing write here; a second possibility is that the rule never delivered to the state-machine target at all.
Why: every AWS SDK / optimized-integration workflow is a no-op against this emulator — a client that asserts on the downstream side effect (the DynamoDB row, not the execution record) sees nothing, with no error surfaced to the event publisher, which is the expensive silent-failure direction.
Fix: the Step Functions interpreter's Task-state dispatch for the aws-sdk / optimized service integrations, and the EventBridge rule target-delivery path it depends on.
Traces: dynamodb__events__sfn.eventbridge-to-sfn-to-ddb-chain

Related open issue that does NOT carry this check's replay key: 20261024T043150Z-sfn-step-functions-interpreter-does-not-run-the-workflow.md.

## Evidence
Error: Check timed out after 60.0s — the check creates a table, a state machine whose only state is {"Type":"Task","Resource":"arn:aws:states:::dynamodb:putItem","Parameters":{"TableName":<table>,"Item":{"pk":{"S.$":"$.detail.order_id"},...}}}, a custom bus, a rule matching source=sim2real.test / detail-type=OrderPlaced with the state machine as target and an events.amazonaws.com role granted states:StartExecution, then PutEvents and polls DynamoDB for the item. Expected: the item with pk = detail.order_id and status 'received'. Actual: no item within the budget.

## Steps to Reproduce
1. CreateTable; CreateStateMachine with a single Task state Resource arn:aws:states:::dynamodb:putItem writing pk from $.detail.order_id.
2. CreateEventBus, PutRule with pattern {"source":["sim2real.test"],"detail-type":["OrderPlaced"]}, PutTargets with the state machine ARN and an events.amazonaws.com role allowing states:StartExecution on it.
3. PutEvents a matching event with detail.order_id set.
4. Poll GetItem for that pk: expected the item within seconds; emulator never writes it.
