Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda__s3__stepfunctions.s3-eventbridge-sfn-lambda-ddb-payload-fidelity
Issue-Key: dynamodb__events__lambda__s3__stepfunctions.s3-eventbridge-sfn-lambda-ddb-payload-fidelity|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# S3 -> EventBridge -> Step Functions -> Lambda -> DynamoDB payload fidelity check never completes

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One net-new root cause in the dynamodb__events__lambda__s3__sfn group.

Cause 1 of 1: the S3 -> EventBridge -> Step Functions -> Lambda -> DynamoDB chain never propagates a payload.
AWS: an S3 object creation raises an EventBridge event, the rule starts a state machine execution whose input carries the S3 record verbatim, the Task state invokes the Lambda, and the handler writes the derived item to DynamoDB — the payload's fields survive each hop unchanged.
Emulator: nothing arrives at the end of the chain; the check polls for the DynamoDB item and ends with `Check timed out after 60.0s`, so no hop can be shown to preserve the payload.
Why: the whole event-driven chain is inert, and a client cannot tell at which hop its data was dropped or reshaped.
Fix: the first hop that does not deliver on this path — the s3 EventBridge notification emitter, the eventbridge rule dispatch to sfn, and the sfn Task state's lambda integration.
Traces: dynamodb__events__lambda__s3__sfn.s3-eventbridge-sfn-lambda-ddb-payload-fidelity

## Evidence
[mismatch] dynamodb__events__lambda__s3__sfn.s3-eventbridge-sfn-lambda-ddb-payload-fidelity — Error: Check timed out after 60.0s. (A related open issue, 20261024T043150Z-sfn-5-of-the-6-interpreter-causes-remain-open.md, records that a Task state's 'arn:aws:states:::lambda:invoke' integration is not one the emulator calls; it does not carry this check's replay key.)

## Steps to Reproduce
Boot the emulator and replay dynamodb__events__lambda__s3__sfn.s3-eventbridge-sfn-lambda-ddb-payload-fidelity: it uploads an object, then polls DynamoDB for the item the chain should produce.
