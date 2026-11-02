Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda__sqs__stepfunctions.eventbridge-fanout-lambda-sqs-sfn
Issue-Key: dynamodb__events__lambda__sqs__stepfunctions.eventbridge-fanout-lambda-sqs-sfn|_poll_until#25|AssertionError|raise AssertionError(
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# EventBridge fan-out never invokes its Lambda target: the consumer function is never run, so its DynamoDB write never appears

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 1: a rule that fans one matched event out to a Lambda target (alongside SQS and Step Functions targets) never produces the Lambda target's effect.
AWS: recorded behavior only — one PutEvents entry matching the rule is delivered to every configured target; the Lambda consumer is invoked with the EventBridge envelope and its handler's DynamoDB write is readable within seconds.
Emulator: the probe for the consumer's DynamoDB item never observes anything — AssertionError: "Lambda fanout consumer wrote to DDB not met within 30s. Last result: None". Hypothesis: either EventBridge does not deliver to Lambda targets, or the delivery reaches Lambda and the invocation is refused (the emulator's Lambda Invoke is reported elsewhere to answer NotImplemented); the check cannot tell those apart because a failed target invocation surfaces nothing to the publisher.
Why: fan-out is silently partial. A client gets FailedEntryCount 0 from PutEvents and concludes the event was handled, while the Lambda leg of the fan-out did nothing — no error, no DLQ signal in the observed window.
Fix: the EventBridge target-delivery path for Lambda-function targets (invoke + resource-policy check), and the Lambda invoke path it calls.
Traces: dynamodb__events__lambda__sfn__sqs.eventbridge-fanout-lambda-sqs-sfn

Related open issues that do NOT carry this check's replay key: 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md, 20261023T140640Z-cloudfront-events-...md.

## Evidence
AssertionError: Lambda fanout consumer wrote to DDB not met within 30s. Last result: None
(raised from the check's _poll_until / _wait_for_effect probe after PutEvents of a single matching entry against a rule with Lambda, SQS and Step Functions targets.)

## Steps to Reproduce
1. Create a DynamoDB table and a Lambda function whose handler writes one item per received EventBridge envelope; add the lambda:InvokeFunction resource policy for events.amazonaws.com.
2. PutRule with a pattern, then PutTargets adding the Lambda function (plus an SQS queue and a state machine) as targets.
3. PutEvents one matching entry.
4. Poll Scan/GetItem on the table for the consumer's item: expected within seconds; emulator never produces it (30 s probe exhausted).
