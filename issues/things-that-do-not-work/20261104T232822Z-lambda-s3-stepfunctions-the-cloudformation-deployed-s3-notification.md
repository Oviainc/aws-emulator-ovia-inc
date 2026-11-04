Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__stepfunctions.cfn-deploy-s3-notification-to-lambda-positive
Issue-Key: lambda__s3__stepfunctions.cfn-deploy-s3-notification-to-lambda-positive|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: 6ff397d47b7f23b010f8590f84afedb6b986e178

# lambda__s3__stepfunctions: the CloudFormation-deployed S3 notification to Lambda never fires, so the deployed pipeline produces no effect

> _Note: this issue was identified against the code base at commit `6ff397d47b7f23b010f8590f84afedb6b986e178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
Cause 1 of 1: an s3:ObjectCreated notification installed by a CloudFormation stack is accepted but never invokes the Lambda destination.
AWS: recorded behavior only for the end-to-end effect; the configuration is s3#PutBucketNotificationConfiguration with a LambdaFunctionConfigurations entry binding the event list to the function ARN, plus the AWS::Lambda::Permission that admits s3.amazonaws.com. Once the stack is CREATE_COMPLETE, uploading an object to the stack's bucket invokes the function asynchronously and its side effect becomes observable at the destination the handler writes to.
Emulator: no invocation ever happens — the check polls for the deployed pipeline's effect and is cut off by the harness after 60.0 s, with the notification configuration reported as installed and nothing delivered.
Why: a deployed bucket trigger that never fires makes the entire S3→Lambda(→Step Functions) deploy pattern inert while the control plane claims it is wired, so a client has no API-visible way to detect that the pipeline is dead.
Fix: the S3 object-write notification dispatch that must issue the asynchronous Lambda invoke for a matching LambdaFunctionConfigurations entry, and the CloudFormation AWS::S3::Bucket NotificationConfiguration handler that installs it.
The open issue 20261029T183117Z-lambda-s3-sfn-four-net-new-causes-raw.md covers related non-delivery causes in this service but does not carry this check's replay key.
Traces: lambda__s3__stepfunctions.cfn-deploy-s3-notification-to-lambda-positive.

## Evidence
Expected: after the stack reaches CREATE_COMPLETE and an object is uploaded to the stack's bucket, the notification invokes the stack's Lambda and the handler's effect is observable within the check's poll budget.
Actual: no effect ever appears and the check ends with "Error: Check timed out after 60.0s" — the notification is never delivered.

## Steps to Reproduce
Replay lambda__s3__stepfunctions.cfn-deploy-s3-notification-to-lambda-positive: CreateStack from the s3-notification-to-lambda deployment template, wait for CREATE_COMPLETE, PutObject into the stack's bucket and poll for the handler's side effect.
