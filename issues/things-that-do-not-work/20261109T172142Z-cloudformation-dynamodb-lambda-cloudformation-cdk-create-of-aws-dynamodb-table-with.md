Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__lambda.deploy-cdk-table-stream-consumer-copies-inserts-into-a-second-table cloudformation__dynamodb__lambda.deploy-cdk-table-stream-consumer-missing-sink-grant-blocks-propagation
Issue-Key: cloudformation__dynamodb__lambda.deploy-cdk-table-stream-consumer-copies-inserts-into-a-second-table|run#15|AssertionError|assert rc == 0, f"`cdk deploy` exited {rc}; output tail {out[-1200:]!r}"
Issue-Key: cloudformation__dynamodb__lambda.deploy-cdk-table-stream-consumer-missing-sink-grant-blocks-propagation|run#15|AssertionError|assert rc == 0, f"`cdk deploy` exited {rc}; output tail {out[-1200:]!r}"
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# CloudFormation/CDK create of AWS::DynamoDB::Table with StreamSpecification fails: streamSpecification.streamEnabled is sent as null

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the AWS::DynamoDB::Table provider does not derive StreamEnabled from the template's StreamSpecification.

AWS: the CloudFormation resource `AWS::DynamoDB::Table` property `StreamSpecification` has exactly one required sub-property, `StreamViewType`; there is no `StreamEnabled` property in the template schema, and CDK's `dynamodb.Table({stream: StreamViewType.NEW_AND_OLD_IMAGES})` synthesises `StreamSpecification: {StreamViewType: NEW_AND_OLD_IMAGES}`. The provider is responsible for calling `dynamodb:CreateTable` with `StreamSpecification.StreamEnabled = true` alongside the declared view type. Smithy (dynamodb): `StreamSpecification.StreamEnabled` is a required boolean on the CreateTable input shape. The stack reaches CREATE_COMPLETE and the table is created with a stream.

Emulator: the provider passes the template's StreamSpecification straight through, so `StreamEnabled` arrives null and DynamoDB's own validation refuses the create. The resource goes CREATE_FAILED with `Resource handler returned message: "1 validation error detected: Value null at 'streamSpecification.streamEnabled' failed to satisfy constraint: Member must not be null"` and `cdk deploy` exits 1.

Why: every CDK/CloudFormation stack that enables DynamoDB Streams — the standard way to wire a table to a Lambda consumer — fails to deploy. No stream-driven architecture can be stood up through CloudFormation at all.

Fix: the AWS::DynamoDB::Table resource provider's property mapping (template StreamSpecification -> CreateTable StreamSpecification), which must set StreamEnabled=true whenever StreamViewType is present and omit the block entirely otherwise.

Traces: cloudformation__dynamodb__lambda.deploy-cdk-table-stream-consumer-copies-inserts-into-a-second-table, cloudformation__dynamodb__lambda.deploy-cdk-table-stream-consumer-missing-sink-grant-blocks-propagation (both fail on the same CREATE_FAILED on the `Source` table).

## Evidence
Expected: `cdk deploy` of a stack whose table declares StreamSpecification: {StreamViewType: NEW_AND_OLD_IMAGES} exits 0 and the table is created with a stream.
Actual: `cdk deploy` exited 1; CREATE_FAILED | AWS::DynamoDB::Table | Source — Resource handler returned message: "1 validation error detected: Value null at 'streamSpecification.streamEnabled' failed to satisfy constraint: Member must not be null" (HandlerErrorCode: InvalidRequest). Stack ends in rollback.

## Steps to Reproduce
1. Synthesize a CDK stack with `new dynamodb.Table(this, 'Source', { partitionKey: ..., stream: dynamodb.StreamViewType.NEW_AND_OLD_IMAGES })`.
2. `cdk deploy`.
3. Observe the Source table goes CREATE_FAILED with the streamSpecification.streamEnabled null validation error and the deploy exits 1.
