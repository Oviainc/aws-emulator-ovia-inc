Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3.deployment-cfn-s3-notification-lambda-dynamodb-stack
Issue-Key: dynamodb__lambda__s3.deployment-cfn-s3-notification-lambda-dynamodb-stack|_poll_until#10|Timeout|raise AssertionError(
Issue-Commit: 5b7d66e77a060af6cd000fd4d7e09192c88c4502

# CloudFormation-deployed S3-notification -> Lambda -> DynamoDB stack never converges: the check hangs past 60s

> _Note: this issue was identified against the code base at commit `5b7d66e77a060af6cd000fd4d7e09192c88c4502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the deployment of a stack that wires an S3 bucket notification to a function that writes DynamoDB never reaches a state the client can observe, so the check runs out of time instead of passing or being refused.
AWS: CreateStack with that template settles in CREATE_COMPLETE within the polling window, the bucket notification is configured, and an upload invokes the function which writes the item - every step either completes or returns an error promptly.
Emulator: "Error: Check timed out after 60.0s" - some step (stack create, notification wiring, or the notification delivery that follows) neither completes nor fails within the deadline. Recorded behavior only: the replay records no response for the step that hangs.
Why: a client gets a hang rather than a result; a hung create cannot be distinguished from a slow one, and nothing downstream (the delivered event, the DynamoDB row) can be observed.
Fix: the CloudFormation create path for the S3 notification + Lambda + DynamoDB resources and the S3 notification delivery that follows it - every step must settle (success or error) rather than block.
Traces: dynamodb__lambda__s3.deployment-cfn-s3-notification-lambda-dynamodb-stack.

## Evidence
Expected (AWS): the stack reaches CREATE_COMPLETE and the uploaded object's notification invokes the function, which writes the DynamoDB item - all within the check's polling window.
Actual (emulator): Error: Check timed out after 60.0s (no terminal stack status or delivery observed).

## Steps to Reproduce
1. Replay the behavioral suite and re-run dynamodb__lambda__s3.deployment-cfn-s3-notification-lambda-dynamodb-stack.
2. CreateStack with AWS::S3::Bucket (NotificationConfiguration -> AWS::Lambda::Function), AWS::Lambda::Permission and AWS::DynamoDB::Table, poll DescribeStacks, then PutObject and poll the table.
