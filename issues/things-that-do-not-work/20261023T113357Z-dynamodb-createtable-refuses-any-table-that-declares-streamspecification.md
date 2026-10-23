Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.ddb-esm-batch-size-one-record-per-invocation-positive apigateway__dynamodb__lambda__logs.ddb-esm-destination-on-failure-missing-config-negative apigateway__dynamodb__lambda__logs.ddb-esm-lambda-error-logged-to-cloudwatch-positive apigateway__dynamodb__lambda__logs.ddb-esm-lambda-missing-logs-permission-negative apigateway__dynamodb__lambda__logs.ddb-esm-parallelization-factor-multiple-shards-positive apigateway__dynamodb__lambda__logs.ddb-esm-tumbling-window-aggregation-positive
Issue-Key: apigateway__dynamodb__lambda__logs|CreateTable|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# DynamoDB CreateTable refuses any table that declares StreamSpecification with NotImplemented, so DynamoDB Streams and every stream event-source mapping are unreachable

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Cause 1 of 1: CreateTable with a StreamSpecification is a scaffold stub.
AWS: CreateTable accepts StreamSpecification and enables DynamoDB Streams on the new table, returning the TableDescription with LatestStreamArn/LatestStreamLabel and StreamSpecification echoed back. Smithy (dynamodb): StreamSpecification = {StreamEnabled (required boolean), StreamViewType (enum KEYS_ONLY|NEW_IMAGE|OLD_IMAGE|NEW_AND_OLD_IMAGES)}; CreateTable's modeled errors are ResourceInUseException, LimitExceededException, InternalServerError etc. - NotImplemented is not modeled, and the member is not optional-to-honour.
Emulator: answers ClientError code 'NotImplemented' with message 'operation CreateTable with StreamSpecification is scaffolded but not implemented in this emulator'; the table is not created at all.
Why: a stream-enabled table cannot be created, so no Lambda event-source mapping on a DynamoDB stream can exist and none of the stream semantics (batch size, parallelization factor, tumbling windows, on-failure destinations) can be exercised. A client's table definition that works on AWS fails outright here.
Fix: honour StreamSpecification in the CreateTable handler of the dynamodb service crate - persist the stream configuration, mint LatestStreamArn/LatestStreamLabel, and feed the dynamodbstreams shard state.
Traces: the six ddb-esm-* checks in trace_ids.

## Evidence
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateTable operation: operation CreateTable with StreamSpecification is scaffolded but not implemented in this emulator. Expected: 200 with TableDescription including StreamSpecification and LatestStreamArn.

## Steps to Reproduce
aws dynamodb create-table --table-name t --attribute-definitions AttributeName=pk,AttributeType=S --key-schema AttributeName=pk,KeyType=HASH --billing-mode PAY_PER_REQUEST --stream-specification StreamEnabled=true,StreamViewType=NEW_AND_OLD_IMAGES  -> NotImplemented.
