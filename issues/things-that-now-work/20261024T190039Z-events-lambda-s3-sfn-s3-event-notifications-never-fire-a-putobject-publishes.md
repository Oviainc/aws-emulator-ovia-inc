Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.s3-write-triggers-eventbridge-rule-to-stepfunctions events__lambda__s3__stepfunctions.s3-upload-triggers-lambda-then-putevents-starts-sfn
Issue-Key: events__lambda__s3__stepfunctions.s3-write-triggers-eventbridge-rule-to-stepfunctions|_poll_for_new_execution#18|Timeout|time.sleep(3)
Issue-Key: events__lambda__s3__stepfunctions.s3-upload-triggers-lambda-then-putevents-starts-sfn|_fire_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# S3 event notifications never fire: a PutObject publishes no aws.s3 'Object Created' event to EventBridge and never invokes a LambdaFunctionConfiguration target (2 causes)

> _Resolution: S3's notification emitter is implemented (`services/service-com-amazonaws-s3-amazons3/src/state/notifications.rs`), so an object change is delivered to every destination its bucket's configuration names and published onto the default event bus when EventBridge delivery is enabled. Cause 1's check passes. Cause 2's S3 half — the `LambdaFunctionConfiguration` branch and its asynchronous invoke — is implemented and tested, but its check still times out downstream: Lambda's `Invoke` has no in-process runtime for a deployment package, so the recorded handler's body never runs and issues no `PutEvents`. That remaining blocker is tracked by `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`._

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Two causes, both in S3's notification emitter. PutBucketNotificationConfiguration is ACCEPTED in each case; the configured notification simply never produces an event.

Cause 1 of 2: with EventBridgeConfiguration enabled on the bucket, a PutObject publishes no aws.s3 / 'Object Created' event to the default event bus.
AWS: recorded behavior only — after PutBucketNotificationConfiguration with {"EventBridgeConfiguration": {}}, each PutObject emits an event on the default bus with source 'aws.s3', detail-type 'Object Created', detail.bucket.name and detail.object.key; a rule matching that pattern starts its target state machine, whose execution input is that envelope.
Emulator: no execution ever appears. The check re-uploads a fresh object every round for up to 150 s and polls ListExecutions each time, and exhausts the suite's 60 s budget with nothing started (Error: Check timed out after 60.0s). This isolates S3 -> EventBridge cleanly: the target state machine's definition is Pass-only, so no Task-state interpreter gap is involved.
Why: the entire S3-driven EventBridge estate is inert, and silently so — PutObject succeeds, so a client has no signal that the downstream pipeline never ran.
Fix: S3's notification emitter — the EventBridgeConfiguration branch that must publish an 'Object Created' event onto the default bus on object creation.
Traces: events__lambda__s3__sfn.s3-write-triggers-eventbridge-rule-to-stepfunctions

Cause 2 of 2: a bucket notification with a LambdaFunctionConfiguration never invokes the function on PutObject.
AWS: recorded behavior only — with a LambdaFunctionConfiguration for s3:ObjectCreated:* and the s3.amazonaws.com invoke permission added to the function, a PutObject invokes the handler with the Records[0].s3 envelope; the handler's own PutEvents then matches a rule and starts a state machine execution.
Emulator: nothing downstream ever appears and the check hits the 60 s cap. Hypothesis: the S3 -> Lambda notification is not delivered (or the invocation it attempts is refused — the emulator's Lambda Invoke is reported elsewhere to answer NotImplemented); either way the function body never runs, so no PutEvents is issued and no execution is started.
Why: the most common serverless trigger in AWS does nothing here, with the write reported successful.
Fix: S3's notification emitter — the LambdaFunctionConfiguration branch and its invoke of the configured function.
Traces: events__lambda__s3__sfn.s3-upload-triggers-lambda-then-putevents-starts-sfn

Related open issues that do NOT carry these checks' replay keys: 20261024T033759Z-dynamodb-lambda-rekognition-s3-...md (CloudFormation does not install an AWS::S3::Bucket NotificationConfiguration — here the configuration IS installed via the API and still emits nothing), 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md.

## Evidence
Cause 1 — Error: Check timed out after 60.0s. Check does: PutBucketNotificationConfiguration(NotificationConfiguration={"EventBridgeConfiguration": {}}), PutRule(pattern {"source":["aws.s3"],"detail-type":["Object Created"],"detail":{"bucket":{"name":[<bucket>]}}}), PutTargets(state machine with definition {"StartAt":"Done","States":{"Done":{"Type":"Pass","End":true}}}), then repeatedly PutObject and poll ListExecutions. Assertion it never reaches: "no Step Functions execution was started from the S3 event".

Cause 2 — Error: Check timed out after 60.0s. Check does: create function (handler calls events.put_events), AddPermission for s3.amazonaws.com, PutBucketNotificationConfiguration with a LambdaFunctionConfiguration, PutRule on the handler's custom source with a state-machine target, then PutObject repeatedly and poll ListExecutions. Assertion it never reaches: "no Step Functions execution was started by the S3->Lambda->EventBridge chain".

## Steps to Reproduce
Cause 1: CreateBucket; PutBucketNotificationConfiguration with {"EventBridgeConfiguration": {}}; PutRule matching source aws.s3 / detail-type 'Object Created' / detail.bucket.name = the bucket; PutTargets with a Pass-only state machine and an events.amazonaws.com role allowing states:StartExecution; PutObject; poll ListExecutions on the state machine. Expected: a new execution whose input is the S3 envelope. Actual: none.
Cause 2: CreateBucket; create a Lambda function that calls PutEvents; lambda AddPermission for s3.amazonaws.com with the bucket as SourceArn; PutBucketNotificationConfiguration with a LambdaFunctionConfiguration for s3:ObjectCreated:*; PutRule on the handler's event source with a state-machine target; PutObject; poll ListExecutions. Expected: an execution started via the handler's PutEvents. Actual: none.
