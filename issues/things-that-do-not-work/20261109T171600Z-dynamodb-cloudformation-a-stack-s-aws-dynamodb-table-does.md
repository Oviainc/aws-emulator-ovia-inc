Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__events__lambda.cfn-eventbridge-scheduled-lambda-deployment
Issue-Key: dynamodb|Scan|ResourceNotFoundException
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# CloudFormation: a stack's AWS::DynamoDB::Table does not exist after CREATE_COMPLETE — Scan answers ResourceNotFoundException in the EventBridge-scheduled-Lambda deployment

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Scan

## Description
Cause 1 of 1: a table declared by a deployed stack is not present in DynamoDB.
AWS: once a stack containing `AWS::DynamoDB::Table` reaches CREATE_COMPLETE, the table exists and `Scan`/`GetItem` against it succeed (returning an empty `Items` list until something writes). In this scenario a stack-deployed `rate(1 minute)` EventBridge rule invokes a Lambda that writes a marker item, and the client scans the table to observe the invocation.
Emulator: `Scan` on the stack's table raises `botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the Scan operation: Requested resource not found` — the table the stack reports as created is absent from DynamoDB's state, so the deployment-order behavior under test cannot even begin.
Why: a client cannot use any resource a stack says it created; the stack status is a false success, and the error it eventually sees (`ResourceNotFoundException` on the data plane) points at the wrong service.
Fix: the AWS::DynamoDB::Table resource provider's create path — it must create the table in DynamoDB's own state (and only then report CREATE_COMPLETE), rather than recording it in the stack's resource table alone.
Traces: dynamodb__events__lambda.cfn-eventbridge-scheduled-lambda-deployment.

Note: open issues 20261106T051027Z-dynamodb-events-lambda-2-net-new-causes-a.md and 20261104T232517Z-dynamodb-lambda-s3-secretsmanager-a-sam-auto-expand.md carry other dynamodb__events__lambda checks; neither carries this replay key.

## Evidence
`botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the Scan operation: Requested resource not found`, raised against the table declared by the deployed stack.

## Steps to Reproduce
1. CreateStack with `AWS::DynamoDB::Table`, an `AWS::Lambda::Function` and an `AWS::Events::Rule` with `ScheduleExpression: rate(1 minute)` targeting the function.
2. Wait for CREATE_COMPLETE.
3. `dynamodb.scan(TableName=<the stack's table>)` — observe ResourceNotFoundException instead of a 200 with `Items: []`.
4. Replay dynamodb__events__lambda.cfn-eventbridge-scheduled-lambda-deployment.
