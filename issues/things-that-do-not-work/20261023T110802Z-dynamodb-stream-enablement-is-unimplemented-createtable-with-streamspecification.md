Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.stream-iterator-at-and-after-sequence-number-resume-exactly dynamodb.stream-records-iterator-and-images dynamodb.deploy-cfn-stream-arn-captures-writes dynamodb__kinesis__lambda.xsvc-lambda-2-ddb-stream-esm-on-failure-sqs-receives-batch-info-pointer dynamodb__lambda.ddb-stream-esm-maximum-record-age-discards-aged-records dynamodb__lambda__s3.ddb-stream-insert-triggers-lambda dynamodb__pipes__sqs.pipes-dynamodb-stream-source-preserves-per-key-change-order dynamodbstreams.closed-shard-get-records-ends-with-a-null-next-shard-iterator dynamodbstreams.concurrent-writes-each-appear-exactly-once dynamodbstreams.describe-stream-shard-pagination-covers-all-shards dynamodbstreams.disable-closes-shards-and-reenable-mints-new-stream dynamodbstreams.foreign-sequence-number-rejected dynamodbstreams.get-records-limit-pages-one-shard-without-gaps-or-repeats dynamodbstreams.iterator-types-select-expected-records dynamodbstreams.keys-only-view-omits-both-images dynamodbstreams.malformed-shard-iterator-rejected dynamodbstreams.new-image-view-omits-old-image dynamodbstreams.nonexistent-stream-arn-rejected dynamodbstreams.old-image-view-omits-new-image dynamodbstreams.shard-record-order-matches-write-order dynamodbstreams.stream-record-creation-time-tracks-the-live-clock
Issue-Key: dynamodb|CreateTable|NotImplemented
Issue-Key: dynamodb.deploy-cfn-stream-arn-captures-writes|_deploy_cfn#3|KeyError|return {"stack_name": stack_name, "outputs": {o["OutputKey"]: o["OutputValue"] for o in outputs}}
Issue-Key: dynamodb__kinesis__lambda|CreateTable|NotImplemented
Issue-Key: dynamodb__lambda|CreateTable|NotImplemented
Issue-Key: dynamodb__lambda__s3|CreateTable|NotImplemented
Issue-Key: dynamodb__pipes__sqs|CreateTable|NotImplemented
Issue-Key: dynamodbstreams|CreateTable|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# DynamoDB stream enablement is unimplemented: CreateTable with StreamSpecification answers NotImplemented, and a CFN stack exporting Table.StreamArn returns an Output with no OutputValue (2 causes)

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
This bundle has 2 causes; both must be closed before submitting.

Cause 1 of 2: CreateTable refuses any request that carries StreamSpecification, answering a non-modeled NotImplemented error instead of creating the table.
AWS: Smithy (com.amazonaws.dynamodb#DynamoDB_20120810): CreateTableInput has an optional member StreamSpecification -> com.amazonaws.dynamodb#StreamSpecification, a structure with required StreamEnabled (boolean) and StreamViewType (enum KEYS_ONLY | NEW_IMAGE | OLD_IMAGE | NEW_AND_OLD_IMAGES). CreateTable's modeled errors are ResourceInUseException, LimitExceededException, InternalServerError and InvalidEndpointException -- NotImplemented is not among them, and StreamSpecification is an ordinary optional input member, not a conditionally-supported one. A successful CreateTable returns 200 with TableDescription echoing StreamSpecification and, once the stream is active, LatestStreamArn / LatestStreamLabel; the stream is then addressable through the dynamodbstreams endpoint (DescribeStream, GetShardIterator, GetRecords).
Emulator: every CreateTable call that includes StreamSpecification fails with botocore ClientError '(NotImplemented) ... operation CreateTable with StreamSpecification is scaffolded but not implemented in this emulator'. CreateTable without StreamSpecification is served normally, so this is a per-member refusal inside an otherwise-implemented operation. The report did not mark these checks ALREADY FILED, and the one open related issue (20261022T150920Z-dynamodbstreams-...-liststreams.md, 'DynamoDB Streams are not registered or validated') is about ListStreams returning an empty Streams list and GetRecords misclassifying a malformed iterator -- it presumes CreateTable with a stream SUCCEEDS, so it does not cover this refusal.
Why: a client cannot create a stream-enabled table at all. Every change-data-capture topology built on DynamoDB Streams is unreachable: dynamodbstreams consumers, Lambda event-source mappings over a table stream, EventBridge Pipes with a DynamoDB stream source, and Kinesis/S3 fan-out all fail at table-creation time with an error code no SDK models, so retry and error-handling logic sees an unclassifiable fault rather than a clean unsupported-feature signal. 20 of the 21 behavioral checks in this run never reached their first assertion because of this one call.
Fix: the DynamoDB CreateTable handler plus the table state that backs it -- accept StreamSpecification, persist it on the table, echo it in TableDescription, mint LatestStreamArn/LatestStreamLabel, and register the stream so the dynamodbstreams surface can resolve it (UpdateTable's StreamSpecification path needs the same support, since several checks enable/disable a stream after creation).
Traces: dynamodb.stream-iterator-at-and-after-sequence-number-resume-exactly, dynamodb.stream-records-iterator-and-images, dynamodb__kinesis__lambda.xsvc-lambda-2-ddb-stream-esm-on-failure-sqs-receives-batch-info-pointer, dynamodb__lambda.ddb-stream-esm-maximum-record-age-discards-aged-records, dynamodb__lambda__s3.ddb-stream-insert-triggers-lambda, dynamodb__pipes__sqs.pipes-dynamodb-stream-source-preserves-per-key-change-order, and all 14 dynamodbstreams.* checks (closed-shard-get-records-ends-with-a-null-next-shard-iterator, concurrent-writes-each-appear-exactly-once, describe-stream-shard-pagination-covers-all-shards, disable-closes-shards-and-reenable-mints-new-stream, foreign-sequence-number-rejected, get-records-limit-pages-one-shard-without-gaps-or-repeats, iterator-types-select-expected-records, keys-only-view-omits-both-images, malformed-shard-iterator-rejected, new-image-view-omits-old-image, nonexistent-stream-arn-rejected, old-image-view-omits-new-image, shard-record-order-matches-write-order, stream-record-creation-time-tracks-the-live-clock).

Cause 2 of 2: a CloudFormation stack whose template declares Outputs over an AWS::DynamoDB::Table reaches CREATE_COMPLETE, but DescribeStacks returns Output elements that carry OutputKey with NO OutputValue member at all.
AWS: Smithy (com.amazonaws.cloudformation#CloudFormation): DescribeStacks returns Stack.Outputs, a list of com.amazonaws.cloudformation#Output with members OutputKey, OutputValue, Description and ExportName. For a stack in CREATE_COMPLETE, each declared Output is returned with both OutputKey and its resolved OutputValue; an output expression that cannot be resolved (here !Ref Table and !GetAtt Table.StreamArn) fails the stack with CREATE_FAILED rather than being reported as a resolved output with an absent value. Recorded behavior: the check drives create_stack, waits on the stack_create_complete waiter (which succeeded), then reads DescribeStacks Stacks[0].Outputs and indexes o["OutputValue"].
Emulator: the waiter reports CREATE_COMPLETE and DescribeStacks answers 200, but at least one Outputs element has no OutputValue key, so a client building {OutputKey: OutputValue} raises KeyError: 'OutputValue'. Note this is also internally inconsistent with Cause 1: the same table definition that CreateTable refuses as unimplemented is reported by CloudFormation as created successfully.
Why: stack outputs are the documented hand-off between a deployment and the code that consumes it. Any client -- the AWS CLI, CDK, Terraform's aws_cloudformation_stack, or plain boto3 -- reads outputs as key/value pairs, so an element without OutputValue makes a successful deployment unusable and, worse, hides the real failure behind a CREATE_COMPLETE status. A client cannot distinguish 'deployed and resolved' from 'silently unresolved'.
Fix: CloudFormation's output resolution and the DescribeStacks serializer -- resolve Ref/GetAtt against the provisioned resource and always emit OutputValue for a returned Output; when an output expression cannot be resolved, surface the stack as CREATE_FAILED with the resource error instead of returning a valueless output. The GetAtt attribute StreamArn on AWS::DynamoDB::Table must resolve to the table's LatestStreamArn, which depends on Cause 1.
Traces: dynamodb.deploy-cfn-stream-arn-captures-writes.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1): Total 0/21 passed (0.0%), Mismatched 0, Errored 21, Skipped 0, Unloadable 0. No failing check was marked ALREADY FILED or OWNED by an open issue. No bootstrap signal was reported: the emulator was serving (CloudFormation create_stack and the stack_create_complete waiter both succeeded against it).

Cause 1 -- identical error on 20 of 21 checks, raised by the first CreateTable call of each check:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateTable operation: operation CreateTable with StreamSpecification is scaffolded but not implemented in this emulator
  Expected: 200 with TableDescription echoing StreamSpecification {StreamEnabled: true, StreamViewType: <one of KEYS_ONLY|NEW_IMAGE|OLD_IMAGE|NEW_AND_OLD_IMAGES>} and a LatestStreamArn once the stream is active.
  Actual: a NotImplemented error code that CreateTable does not model.
  Per-service pass rates, all blocked here: dynamodb 0/3, dynamodb__kinesis__lambda 0/1, dynamodb__lambda 0/1, dynamodb__lambda__s3 0/1, dynamodb__pipes__sqs 0/1, dynamodbstreams 0/14.

Cause 2 -- dynamodb.deploy-cfn-stream-arn-captures-writes:
  File "check_dynamodb_deploy_cfn_stream_arn_captures_writes.py", line 124, in _deploy_cfn
    return {"stack_name": stack_name, "outputs": {o["OutputKey"]: o["OutputValue"] for o in outputs}}
  KeyError: 'OutputValue'
  Expected: DescribeStacks on a CREATE_COMPLETE stack returns each declared Output with both OutputKey and OutputValue (here TableName from !Ref Table and StreamArn from !GetAtt Table.StreamArn).
  Actual: Outputs elements present but missing the OutputValue member, while the stack still reports CREATE_COMPLETE.

Stack names, table names, stream ARNs, stream labels, shard ids and sequence numbers in these checks are generated per run and are examples only; the requirements are the successful CreateTable with StreamSpecification echoed back, and an Output element that always carries OutputValue.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Cause 1: call DynamoDB CreateTable with AttributeDefinitions [{pk, S}], KeySchema [{pk, HASH}], BillingMode PAY_PER_REQUEST and StreamSpecification {StreamEnabled: true, StreamViewType: NEW_AND_OLD_IMAGES}. Observe the request fail with error code NotImplemented ('operation CreateTable with StreamSpecification is scaffolded but not implemented in this emulator') instead of 200 with a TableDescription. Re-replay any of the 20 check ids in trace_ids other than dynamodb.deploy-cfn-stream-arn-captures-writes to confirm; each one fails on its very first CreateTable.
3. Cause 2: replay dynamodb.deploy-cfn-stream-arn-captures-writes. It calls CloudFormation CreateStack with a template declaring an AWS::DynamoDB::Table (StreamSpecification StreamViewType NEW_AND_OLD_IMAGES) and Outputs TableName: !Ref Table and StreamArn: !GetAtt Table.StreamArn, waits for stack_create_complete (which succeeds), then calls DescribeStacks and reads Stacks[0].Outputs. Observe an Outputs element with OutputKey but no OutputValue, raising KeyError: 'OutputValue'.
4. After fixing, re-replay the full trace_ids list; the 20 stream checks cannot be validated until Cause 1 lands, and the CFN GetAtt StreamArn output in Cause 2 depends on it.
