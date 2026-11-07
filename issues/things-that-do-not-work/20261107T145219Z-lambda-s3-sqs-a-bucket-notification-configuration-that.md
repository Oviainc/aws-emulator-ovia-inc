Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.s3-notification-multiple-destinations-fanout-positive
Issue-Key: lambda__s3__sqs.s3-notification-multiple-destinations-fanout-positive|_poll_for_marker#18|Timeout|time.sleep(5.0)
Issue-Commit: 323c298260f641866db820e53cda367b812962dd

# lambda__s3__sqs: a bucket notification configuration that fans out by prefix to both an SQS queue and a Lambda function delivers nothing, so the fan-out check never reaches a verdict

> _Note: this issue was identified against the code base at commit `323c298260f641866db820e53cda367b812962dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
1 net-new root cause.

Cause 1 of 1: one bucket configured with a prefix-scoped QueueConfiguration and a prefix-scoped LambdaFunctionConfiguration emits no ObjectCreated event to either destination, and the scenario is cut off at the harness budget before any assertion fires. Failing check: lambda__s3__sqs.s3-notification-multiple-destinations-fanout-positive.

AWS: recorded behavior only — the model carries no delivery semantics for PutBucketNotificationConfiguration. The recording shows: after a configuration carrying QueueConfigurations=[{QueueArn, Events:["s3:ObjectCreated:*"], Filter.Key.FilterRules:[{Name:prefix, Value:"sqs/"}]}] and LambdaFunctionConfigurations=[{LambdaFunctionArn, Events:["s3:ObjectCreated:*"], Filter.Key.FilterRules:[{Name:prefix, Value:"lambda/"}]}] is accepted (the queue policy grants s3.amazonaws.com SQS:SendMessage for that bucket's source ARN and the function has a lambda:InvokeFunction permission for principal s3.amazonaws.com), a PutObject under the "sqs/" prefix puts exactly one event document on the queue whose Records[].s3.object.key is that key, and a PutObject under the "lambda/" prefix invokes the function exactly once with the same envelope. Each destination sees only the keys its own prefix filter matches, and both legs settle quickly enough for the whole scenario to finish inside the suite's per-check budget.

Emulator: the check never gets past its first collection. The suite reports `Error: Check timed out after 60.0s` with no assertion having fired, i.e. the ReceiveMessage poll for the "sqs/"-prefixed ObjectCreated record never saw it and the Lambda leg was never reached — the two-destination, prefix-filtered notification configuration produces no observable event for either destination within the window. The fan-out and the prefix scoping (no cross-delivery between the two destinations) therefore cannot be observed at all.

Why: a client that uses a single bucket to route uploads by prefix — "sqs/" to a queue for batch work, "lambda/" to a function for inline work — gets a silent dead end: no message, no invocation, no error. Pipelines built on one notification configuration with mixed destination types appear wired (PutBucketNotificationConfiguration succeeds and GetBucketNotificationConfiguration echoes it) but move no data, and the opposite defect (an event leaking to the destination whose prefix does not match) is equally unobservable.

Fix: the S3 object-write notification dispatch path — the component that, on PutObject, evaluates the bucket's stored notification configuration and delivers to each matching destination list (QueueConfigurations and LambdaFunctionConfigurations together, not one or the other), including the Filter.Key prefix-rule matching used to pick destinations.

Related but not owning: open issues 20261104T044826Z-lambda-s3-sqs-s3-bucket-notifications-never-invoke-their-lambda-target.md (Lambda targets never invoked) and 20261104T232822Z-lambda-s3-sqs-2-net-new-root-causes.md (CFN-declared prefix-scoped notifications) cover neighbouring symptoms; neither is marked as owning this check, whose configuration mixes both destination kinds on one bucket.

Traces: lambda__s3__sqs.s3-notification-multiple-destinations-fanout-positive.

## Evidence
Check lambda__s3__sqs.s3-notification-multiple-destinations-fanout-positive, verdict mismatch.

Expected (recorded behavior): PutObject on "sqs/routed-to-queue.txt" yields one queue message whose Records[].s3.object.key == "sqs/routed-to-queue.txt" and no record for the lambda/-prefixed key; PutObject on "lambda/routed-to-function.txt" invokes the function, which logs "S3_EVT::<record>" for that key and never for the sqs/-prefixed key; the whole scenario completes inside the suite's per-check budget.

Actual (emulator): `Error: Check timed out after 60.0s` — no assertion reached. The first blocking collection is the queue poll for the "sqs/" record (ReceiveMessage with WaitTimeSeconds=5 in a loop), so no ObjectCreated event for the sqs/-prefixed upload was ever observable, and the lambda/ leg was never exercised.

## Steps to Reproduce
1. S3: create a bucket.
2. SQS: create a queue and set its Policy to allow Principal {"Service":"s3.amazonaws.com"} SQS:SendMessage on the queue ARN with Condition ArnEquals aws:SourceArn = arn:aws:s3:::<bucket>.
3. IAM/Lambda: create a role trusted by lambda.amazonaws.com, create a python3.12 function that prints "S3_EVT::" + json.dumps(record) for each event record, and AddPermission(Action=lambda:InvokeFunction, Principal=s3.amazonaws.com, SourceArn=arn:aws:s3:::<bucket>).
4. PutBucketNotificationConfiguration on the bucket with BOTH a QueueConfigurations entry (Events ["s3:ObjectCreated:*"], Filter prefix "sqs/") and a LambdaFunctionConfigurations entry (Events ["s3:ObjectCreated:*"], Filter prefix "lambda/").
5. PutObject "sqs/routed-to-queue.txt" and PutObject "lambda/routed-to-function.txt".
6. ReceiveMessage from the queue; then read the function's log.
Expected: the queue holds exactly the sqs/-prefixed ObjectCreated record, and the function log holds exactly the lambda/-prefixed one.
Observed: nothing arrives on either destination; the scenario hangs at the queue poll until the 60 s per-check budget cuts it off.
