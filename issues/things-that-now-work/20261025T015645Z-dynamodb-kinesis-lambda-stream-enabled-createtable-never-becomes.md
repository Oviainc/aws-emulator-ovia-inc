Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kinesis__lambda.xsvc-lambda-2-ddb-stream-esm-on-failure-sqs-receives-batch-info-pointer
Issue-Key: dynamodb__kinesis__lambda.xsvc-lambda-2-ddb-stream-esm-on-failure-sqs-receives-batch-info-pointer|_probe#12|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# dynamodb__kinesis__lambda: stream-enabled CreateTable never becomes visible to DescribeTable, hanging the table_exists waiter before the DynamoDB-stream ESM OnFailure pointer can be graded (1 cause)

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
This issue has 1 cause.

Context: this is the same underlying stream-enabled-CreateTable defect already reported for other service tokens in this corpus (20261025T011017Z-dynamodb-createtable-with-streamspecification-leaves-the-table-invisible-to.md, 20261025T011017Z-dynamodb-lambda-stream-enabled-createtable-never-becomes-visible-to-describetable.md, 20261025T011017Z-dynamodb-lambda-s3-stream-enabled-createtable-never-becomes-visible-to-describetable.md, 20261025T011017Z-dynamodbstreams-a-stream-enabled-table-is-never-registered-on.md), reported separately because this check's service token is dynamodb__kinesis__lambda and the suite marked it neither ALREADY FILED nor OWNED. It is NOT the already-filed CreateTable NotImplemented refusal (20261023T110802Z-dynamodb-stream-enablement-is-unimplemented-createtable-with-streamspecification.md): CreateTable answers 200 with a LatestStreamArn in this run. Note also that although the check declares SERVICES=[dynamodb, kinesis, lambda], its body never constructs a kinesis client, so the pending kinesis bootstrap is not the blocker here — the blocking call is DynamoDB DescribeTable.

Cause 1 of 1: a table created with StreamSpecification is not resolvable by DescribeTable afterwards, so the check's table_exists waiter polls until the harness budget expires and none of the event-source-mapping / OnFailure-destination legs ever run.
AWS: Smithy (com.amazonaws.dynamodb#DynamoDB_20120810): CreateTable returns 200 with a TableDescription, and the named table is then addressable by DescribeTable (modeled client errors: ResourceNotFoundException httpError 400, InvalidEndpointException) and by PutItem/UpdateItem. StreamSpecification is an ordinary optional CreateTable member; supplying it does not change when the table becomes describable. Recorded behavior (the check's own measurement note, us-west-2): shards appear ~8 s after CreateTable, by which point the table is ACTIVE and describable. A DynamoDB-streams ESM then needs both a live table to mutate and a readable stream before MaximumRetryAttempts exhaustion can route a DDBStreamBatchInfo pointer to the SQS OnFailure destination.
Emulator: the check creates a plain audit table (no stream) and PutItem on it succeeds, creates the SQS OnFailure queue, then calls _create_stream_table(), which issues create_table(... StreamSpecification={StreamEnabled:true, StreamViewType:NEW_IMAGE}) and immediately ddb.get_waiter('table_exists').wait(TableName=name, WaiterConfig={'Delay':2,'MaxAttempts':60}) — a 120 s poll. The check is killed at the 60 s harness budget with no ClientError raised, so the run was still inside that waiter. The mechanism is shown outright by the dynamodbstreams checks of the same run: DescribeStream/GetShardIterator on the minted stream ARN succeed while PutItem/DescribeTable on the same TableName are refused 400 ResourceNotFoundException. Hypothesis: the StreamSpecification path registers the stream but not the table.
Why: a client cannot stand up any DynamoDB-stream-sourced Lambda pipeline. The source table a producer writes to never materialises, so retry exhaustion, the DestinationConfig.OnFailure path, the source-specific DDBStreamBatchInfo pointer key (vs KinesisBatchInfo), requestContext.condition=RetryAttemptsExhausted and approximateInvokeCount are all unreachable. Worse, the provisioning step hangs in a waiter instead of failing fast, so the caller sees a timeout rather than an actionable error.
Fix: the DynamoDB CreateTable handler and the table state it writes — the StreamSpecification path must register the table and its item store exactly as the non-stream path does, and advance TableStatus to ACTIVE, so DescribeTable answers 200 and the table_exists waiter completes.
Traces: dynamodb__kinesis__lambda.xsvc-lambda-2-ddb-stream-esm-on-failure-sqs-receives-batch-info-pointer.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1): Total 0/17 passed (0.0%); Mismatched 7, Errored 10, Skipped 0, Unloadable 0. Per-service: dynamodb__kinesis__lambda 0/1 (0%), 1 failed. No bootstrap signal — the emulator was serving (CreateTable, PutItem on a non-stream table, SQS CreateQueue/GetQueueAttributes all answered 200).

dynamodb__kinesis__lambda.xsvc-lambda-2-ddb-stream-esm-on-failure-sqs-receives-batch-info-pointer — [mismatch] 'Check timed out after 60.0s'. Order of calls in the check body: _create_table(ctx, 'audit') (no StreamSpecification) -> 200; ddb.put_item on that table -> 200; _create_queue(ctx) -> 200; then _create_stream_table(ctx), whose helper issues create_table(KeySchema [{pk,HASH}], AttributeDefinitions [{pk,S}], BillingMode PAY_PER_REQUEST, StreamSpecification {StreamEnabled:true, StreamViewType:NEW_IMAGE}) followed immediately by ddb.get_waiter('table_exists').wait(TableName=name, WaiterConfig={'Delay':2,'MaxAttempts':60}). No ClientError surfaced, so execution was inside that waiter when the 60 s budget cut the check off.

Corroborating, same run, against tables created with the identical StreamSpecification shape:
  File "context.py", line 123, in wrapper -> File "client.py", line 1094, in _make_api_call
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the PutItem operation: Requested resource not found
(seen on 9 dynamodbstreams checks in this same suite run).

Expected: DescribeTable answers 200 with TableStatus CREATING/ACTIVE for the just-created stream-enabled table, so table_exists completes within seconds and the check proceeds to CreateEventSourceMapping and the SQS OnFailure assertions.
Actual: the table is not resolvable by DescribeTable; the waiter never completes and consumes the whole check budget.

Table names, function names, role names, queue names, stream ARNs, shard ids and sequence numbers are generated per run and are examples only; the requirement is that CreateTable with StreamSpecification produce a table DescribeTable resolves with 200.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Call DynamoDB CreateTable with KeySchema [{pk,HASH}], AttributeDefinitions [{pk,S}], BillingMode PAY_PER_REQUEST and StreamSpecification {StreamEnabled:true, StreamViewType:NEW_IMAGE}. It answers 200 and returns TableDescription.LatestStreamArn.
3. Run boto3's table_exists waiter (or poll DescribeTable directly) on that TableName. Observe DescribeTable refused 400 ResourceNotFoundException and the waiter never completing; expected 200 with TableStatus CREATING or ACTIVE.
4. For contrast, call DescribeStream and GetShardIterator(TRIM_HORIZON) on the LatestStreamArn from step 2 — both succeed, confirming the stream half is registered while the table half is not.
5. Re-replay dynamodb__kinesis__lambda.xsvc-lambda-2-ddb-stream-esm-on-failure-sqs-receives-batch-info-pointer; after the fix it should get past table creation, create the ESM with MaximumRetryAttempts=1 and DestinationConfig.OnFailure, and reach the DDBStreamBatchInfo pointer assertions on the SQS queue.
