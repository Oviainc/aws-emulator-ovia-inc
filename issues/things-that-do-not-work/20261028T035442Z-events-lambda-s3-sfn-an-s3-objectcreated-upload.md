Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.s3-upload-triggers-lambda-then-putevents-starts-sfn
Issue-Key: events__lambda__s3__stepfunctions.s3-upload-triggers-lambda-then-putevents-starts-sfn|_fire_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: b46de20c42d1d13a6a9554983f2e031495be43a2

# events__lambda__s3__sfn: an S3 ObjectCreated upload never reaches its Lambda notification target, so the S3 -> Lambda -> PutEvents -> rule -> Step Functions chain starts no execution and the check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `b46de20c42d1d13a6a9554983f2e031495be43a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Cause 1 of 1: S3 accepts a bucket notification configuration with a LambdaFunctionConfigurations entry for s3:ObjectCreated:*, but no object-created event is ever delivered to that Lambda, so every downstream hop of the chain (Lambda -> events:PutEvents -> matching EventBridge rule -> states:StartExecution) never happens and the state machine has zero executions. Failing check: events__lambda__s3__sfn.s3-upload-triggers-lambda-then-putevents-starts-sfn.

AWS: Smithy (s3 PutBucketNotificationConfiguration) models PUT /{Bucket}?notification with a required NotificationConfiguration httpPayload carrying LambdaFunctionConfigurations{LambdaFunctionArn, Events}, and no modelled errors; the model describes only the configuration call, so the delivery contract itself is recorded/documented behavior only. On real AWS, once such a configuration is installed and the function grants lambda:InvokeFunction to the s3.amazonaws.com principal for that bucket ARN, a successful PutObject asynchronously invokes the named function with an `Records[0].s3` event carrying bucket.name and object.key; the handler's events:PutEvents entry is then matched by a rule whose pattern selects its Source, and the rule's Step Functions target starts an execution whose input is the full EventBridge envelope (top-level `source` equal to the event's Source and `detail` equal to the published Detail object).

Emulator: the configuration call and all setup calls succeed (bucket, role, function reaching Active, add_permission for s3.amazonaws.com, put_rule, put_targets with FailedEntryCount 0, create_state_machine), and the PutObject succeeds, but sfn ListExecutions for the target state machine keeps returning an empty `executions` list. The check re-fires the upload on a 10 s interval and polls, and nothing ever appears: the replay ends with `Error: Check timed out after 60.0s`, with no error code or non-2xx from any call. Nothing in the chain is refused — it simply produces no effect.

Why: a client that wires an S3 object-created notification to a Lambda, and fans the result out through EventBridge to Step Functions, observes a fully healthy control plane (every describe/list reports the notification, permission, rule, target and state machine as configured) while no work is ever performed. Object-arrival-driven pipelines never run, so against the emulator they are indistinguishable from a pipeline that is mis-wired, and a test that waits for the resulting execution hangs instead of failing with a diagnosable error.

Fix: the S3 object-write path's event-publication step — the notification dispatcher that should, after a successful PutObject commits, match the bucket's stored LambdaFunctionConfigurations against the s3:ObjectCreated:* event type and perform the asynchronous Lambda invoke (and from there the EventBridge rule-match and StartExecution target dispatch). It belongs with the bucket's notification state and the S3 write handler, not in the PutBucketNotificationConfiguration validation, which already stores the configuration correctly.

Related open issues describe the same missing S3 notification dispatch on other destination paths, but none carries this check's replay key: 20261025T022656Z-dynamodb-lambda-s3-an-s3-putobject-never-invokes.md, 20261025T121424Z-lambda-opensearch-s3-an-s3-objectcreated-notification-with.md, 20261025T002357Z-events-s3-sqs-s3-produces-no-aws-s3-events-on-either.md and 20261025T032458Z-s3-sqs-s3-notification-destinations-are-never-validated.md. This check is the four-hop S3 -> Lambda -> EventBridge -> Step Functions variant and is still unowned.

Traces: events__lambda__s3__sfn.s3-upload-triggers-lambda-then-putevents-starts-sfn.

## Evidence
Expected (AWS): after PutObject on a bucket whose notification configuration names the Lambda for s3:ObjectCreated:*, the function is invoked, its events:PutEvents entry matches the rule, and the rule's Step Functions target starts an execution. sfn ListExecutions(stateMachineArn) then returns >= 1 execution; DescribeExecution reports status SUCCEEDED and its `input` is the EventBridge envelope, with input["source"] equal to the published Source and input["detail"]["bucket"]/["key"] equal to the uploaded bucket and key.

Actual (emulator): every setup and the PutObject return 2xx, but sfn ListExecutions(stateMachineArn) returns {"executions": []} indefinitely. The check re-fires the upload and re-polls until the harness cap and the replay reports:

  [mismatch] events__lambda__s3__sfn.s3-upload-triggers-lambda-then-putevents-starts-sfn
  Error: Check timed out after 60.0s

No error code, no non-2xx and no refusal message is produced anywhere in the chain, so the only observable divergence is the absent effect. The assertion the check would otherwise reach is "no Step Functions execution was started by the S3->Lambda->EventBridge chain".

## Steps to Reproduce
1. Create a bucket (with CreateBucketConfiguration.LocationConstraint outside us-east-1).
2. Create an IAM role trusted by lambda.amazonaws.com with an inline policy allowing events:PutEvents on *, and create a python3.12 function whose handler reads Records[0].s3.bucket.name / .object.key and calls events:PutEvents with a unique Source, DetailType "ObjectProcessed" and a Detail of {"bucket": ..., "key": ...}. Wait for the function to become Active.
3. lambda AddPermission: Action lambda:InvokeFunction, Principal s3.amazonaws.com, SourceArn arn:aws:s3:::<bucket>, SourceAccount <account>.
4. s3 PutBucketNotificationConfiguration with NotificationConfiguration.LambdaFunctionConfigurations = [{LambdaFunctionArn: <function arn>, Events: ["s3:ObjectCreated:*"]}].
5. Create a role trusted by states.amazonaws.com and a STANDARD state machine {"StartAt":"Echo","States":{"Echo":{"Type":"Pass","End":true}}}.
6. events PutRule with EventPattern {"source": ["<unique source>"]}, State ENABLED, then PutTargets with a Step Functions target (Arn = state machine ARN, RoleArn = a role trusted by events.amazonaws.com allowing states:StartExecution on it). Confirm FailedEntryCount is 0.
7. s3 PutObject into the bucket.
8. Poll sfn ListExecutions(stateMachineArn=<state machine ARN>). Expected: at least one execution, SUCCEEDED, with the EventBridge envelope as its input. Observed: the list stays empty and the check runs out the 60 s harness cap.
