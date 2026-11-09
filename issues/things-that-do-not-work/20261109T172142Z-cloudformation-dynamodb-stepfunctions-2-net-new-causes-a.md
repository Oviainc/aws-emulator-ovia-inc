Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__stepfunctions.deploy-cdk-dynamo-put-item-task-writes-item-execution-reads-it-back cloudformation__dynamodb__stepfunctions.deploy-cdk-dynamo-putitem-denied-fails cloudformation__dynamodb__stepfunctions.deploy-dynamodb-provisioned-billing-without-throughput-rejected-at-stack-create
Issue-Key: cloudformation__dynamodb__stepfunctions|DescribeTable|ResourceNotFoundException
Issue-Key: cloudformation__dynamodb__stepfunctions.deploy-cdk-dynamo-putitem-denied-fails|run#29|AssertionError|assert control["status"] == "SUCCEEDED", (
Issue-Key: cloudformation__dynamodb__stepfunctions.deploy-dynamodb-provisioned-billing-without-throughput-rejected-at-stack-create|run#16|AssertionError|assert got_error == want_error, f"MyTable CREATE_FAILED reason: expected {want_error!r}, got {got_error!r}"
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# cloudformation__dynamodb__stepfunctions: 2 net-new causes — a CDK-deployed table is not addressable after CREATE_COMPLETE, and the CREATE_FAILED reason for PROVISIONED without throughput is not AWS's

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
Two distinct root causes. Each is listed with its own failing check ids.

Cause 1 of 2: a table created by a CDK/CloudFormation stack is not addressable by the physical name the stack reports.
AWS: when a stack containing an `AWS::DynamoDB::Table` reaches CREATE_COMPLETE, the resource's PhysicalResourceId is the table name, and `dynamodb:DescribeTable`/`PutItem` on that name succeed immediately; a Step Functions `dynamodb:putItem` optimized task targeting it also succeeds. Smithy (dynamodb): DescribeTable/PutItem answer `ResourceNotFoundException` only when no such table exists.
Emulator: the stack reports success, but `DescribeTable` on the generated physical name answers `ResourceNotFoundException: Requested resource not found: Table: <stack>-<logicalid>-<suffix> not found`, and the state machine's optimized PutItem task fails the execution with `DynamoDB.ResourceNotFoundException` / `Requested resource not found`. The table the stack claims to have created does not exist under the name the stack published.
Why: every consumer that resolves a resource from stack outputs or `DescribeStackResource` — Step Functions tasks, Lambda env vars, test harnesses — gets a name that resolves to nothing, so a green deploy is followed by universal ResourceNotFound.
Fix: the AWS::DynamoDB::Table provider's physical-id generation and its handoff to the DynamoDB service state (the name recorded on the stack resource must be the name the table is created under).
Traces: deploy-cdk-dynamo-put-item-task-writes-item-execution-reads-it-back, deploy-cdk-dynamo-putitem-denied-fails.

Cause 2 of 2: the CREATE_FAILED reason for a PROVISIONED table without ProvisionedThroughput is not AWS's text.
AWS: a template declaring `BillingMode: PROVISIONED` with no `ProvisionedThroughput` fails that resource at stack create with the CloudFormation provider's own fixed message `Property ProvisionedThroughput cannot be empty` and HandlerErrorCode `InvalidRequest` — this is the provider's schema-level check, which precedes any CreateTable call.
Emulator: the resource fails with `One or more parameter values were invalid: ProvisionedThroughput must be specified when BillingMode is PROVISIONED` (HandlerErrorCode InvalidRequest) — the DynamoDB service's ValidationException text leaks out in place of the provider's message.
Why: template linting and CI assertions key on the provider message; and the two messages tell an operator to look in different places (template schema vs. API parameters).
Fix: the AWS::DynamoDB::Table provider's required-property validation, which must reject before calling CreateTable and emit the provider's own message.
Traces: deploy-dynamodb-provisioned-billing-without-throughput-rejected-at-stack-create.

## Evidence
Cause 1: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTable operation: Requested resource not found: Table: <stack>-TableCD117FA1-<suffix> not found. And: positive control ended 'FAILED', expected SUCCEEDED (error='DynamoDB.ResourceNotFoundException', cause='Requested resource not found (Service: AmazonDynamoDBv2; Status Code: 400; Error Code: ResourceNotFoundException; ...)'). The stack/table names are per-run values; the requirement is that the published physical name resolves.
Cause 2: AssertionError: MyTable CREATE_FAILED reason: expected ('Property ProvisionedThroughput cannot be empty', 'InvalidRequest'), got ('One or more parameter values were invalid: ProvisionedThroughput must be specified when BillingMode is PROVISIONED', 'InvalidRequest')

## Steps to Reproduce
Cause 1: `cdk deploy` a stack with a dynamodb.Table and a Step Functions DynamoPutItem task targeting it; after CREATE_COMPLETE call dynamodb:DescribeTable on the resource's PhysicalResourceId, and start the state machine.
Cause 2: CreateStack with an AWS::DynamoDB::Table whose BillingMode is PROVISIONED and which omits ProvisionedThroughput; read the resource's CREATE_FAILED ResourceStatusReason and HandlerErrorCode.
