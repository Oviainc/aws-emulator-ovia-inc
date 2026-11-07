Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__stepfunctions.s3-notification-triggers-lambda-positive lambda__s3__stepfunctions.s3-notification-payload-format-preserved lambda__s3__stepfunctions.s3-event-filter-prefix-negative lambda__s3__stepfunctions.s3-notification-missing-invoke-permission-negative lambda__s3__stepfunctions.cfn-deploy-s3-notification-to-lambda-positive lambda__s3__stepfunctions.s3-trigger-lambda-starts-sfn-pipeline lambda__s3__stepfunctions.lambda-starts-sfn-missing-permission-negative lambda__s3__stepfunctions.sfn-pipeline-lambda-broken-s3-permission-negative
Issue-Key: lambda__s3__stepfunctions.s3-notification-triggers-lambda-positive|_poll_until#12|AssertionError|raise AssertionError(
Issue-Key: lambda__s3__stepfunctions.s3-notification-payload-format-preserved|_poll_until#13|AssertionError|raise AssertionError(
Issue-Key: lambda__s3__stepfunctions.s3-event-filter-prefix-negative|_poll_until#15|AssertionError|raise AssertionError(
Issue-Key: lambda__s3__stepfunctions.s3-notification-missing-invoke-permission-negative|_poll_until#22|AssertionError|raise AssertionError(
Issue-Key: lambda__s3__stepfunctions.cfn-deploy-s3-notification-to-lambda-positive|_poll_until#13|AssertionError|raise AssertionError(
Issue-Key: lambda__s3__stepfunctions.s3-trigger-lambda-starts-sfn-pipeline|_poll_until#23|AssertionError|raise AssertionError(
Issue-Key: lambda__s3__stepfunctions.lambda-starts-sfn-missing-permission-negative|_poll_until#17|AssertionError|raise AssertionError(
Issue-Key: lambda__s3__stepfunctions.sfn-pipeline-lambda-broken-s3-permission-negative|_run_until#26|AssertionError|raise AssertionError(
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# 2 net-new causes: S3 ObjectCreated notifications never invoke the function, and the Lambda runtime's bundled SDK has no s3/stepfunctions client

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Two root causes, both grounded in failing checks this report does NOT mark ALREADY FILED or OWNED. Open issues 20261105T081644Z-lambda-s3-stepfunctions-the-lambda-python-runtime-ships.md and 20261029T183117Z-lambda-s3-sfn-four-net-new-causes-raw.md own OTHER checks of this service and do not carry the replay keys below.

Cause 1 of 2: a bucket's LambdaFunctionConfigurations notification never invokes the function.
AWS: with a LambdaFunctionConfigurations entry for s3:ObjectCreated:* (optionally with a prefix/suffix FilterRule) and lambda:InvokeFunction granted to s3.amazonaws.com with the bucket as SourceArn, PutObject invokes the function asynchronously within seconds with the standard notification envelope (Records[0].eventSource 'aws:s3', eventName 'ObjectCreated:Put', s3.bucket.name, s3.object.key URL-encoded, size, eTag, sequencer). A key outside the configured prefix is not delivered, and a function without the permission is not invoked.
Emulator: nothing is delivered on any path, including the explicitly calibrated positive ones: "condition not met within 90s. Last result: []" / "... Last result: None", "Lambda triggered for matching uploads/ prefix not met within 90s. Last result: []", "Lambda invocation marker after granting invoke permission not met within 90s. Last result: []", "condition not met within 120s. Last result: []" (CloudFormation-deployed wiring).
Why: the most common serverless trigger does nothing, the notification payload shape cannot be verified, and both negatives (prefix filter, missing permission) are unfalsifiable because the matching case never fires either.
Fix: the S3 notification dispatcher's Lambda destination — event matching against the bucket's configurations and FilterRules, the function resource-policy check against the bucket SourceArn, the notification-envelope serializer, and the asynchronous Invoke; plus the CloudFormation path that installs a bucket NotificationConfiguration.
Traces: lambda__s3__stepfunctions.s3-notification-triggers-lambda-positive, .s3-notification-payload-format-preserved, .s3-event-filter-prefix-negative, .s3-notification-missing-invoke-permission-negative, .cfn-deploy-s3-notification-to-lambda-positive, .s3-trigger-lambda-starts-sfn-pipeline.

Cause 2 of 2: function code cannot build clients for s3 or stepfunctions.
AWS: the managed python runtime's boto3 builds a client for any service; sfn.start_execution from a handler returns an executionArn (or raises ClientError AccessDeniedException when states:StartExecution is not granted), and s3.put_object writes the object (or raises AccessDenied).
Emulator: the handler faults before any call with errorType 'UnknownServiceError' and errorMessage "Unknown service: 'stepfunctions'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager" (and the same for 's3'), raised from the bundled client.py _raise_refusal; the state-machine execution consequently ends FAILED with error 'UnknownServiceError'. The enumerated service list is the emulator's own registry and is itself the defect.
Why: no function can start an execution or write an object, and every IAM negative is unfalsifiable because the granted and the denied case both produce the identical UnknownServiceError; a state machine also reports a failure cause AWS never emits.
Fix: the in-process Lambda runtime's bundled AWS SDK shim — its service registry and the client factory that refuses unknown services.
Traces: lambda__s3__stepfunctions.lambda-starts-sfn-missing-permission-negative, .sfn-pipeline-lambda-broken-s3-permission-negative.

## Evidence
Cause 1: expected the function invoked on PutObject with the standard Records[] envelope; got nothing — 'condition not met within 90s. Last result: []' (and []/None on the prefix-filter, permission and CloudFormation-deployed variants, 120s).
Cause 2: expected an executionArn (or AccessDeniedException) from the handler; got errorType 'UnknownServiceError', errorMessage "Unknown service: 'stepfunctions'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager", and the owning execution FAILED with error 'UnknownServiceError'.

## Steps to Reproduce
1) Cause 1: PutBucketNotificationConfiguration with a LambdaFunctionConfigurations entry (with and without a prefix FilterRule), AddPermission for s3.amazonaws.com with the bucket SourceArn, PutObject inside and outside the prefix, and poll for invocations and the record payload.
2) Cause 2: Invoke a handler calling boto3.client('stepfunctions').start_execution and boto3.client('s3').put_object, with the grant present and absent.
