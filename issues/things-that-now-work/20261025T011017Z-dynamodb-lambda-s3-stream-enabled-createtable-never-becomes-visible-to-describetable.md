Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3.ddb-stream-insert-triggers-lambda
Issue-Key: dynamodb__lambda__s3.ddb-stream-insert-triggers-lambda|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: a0c9c8ccc8bc5abababeef7ae6993b0ce3338eb3

# Stream-enabled CreateTable never becomes visible to DescribeTable, hanging the table_exists waiter before a DynamoDB-stream INSERT can trigger Lambda

> _Note: this issue was identified against the code base at commit `a0c9c8ccc8bc5abababeef7ae6993b0ce3338eb3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
This issue has 1 cause.

Context: same root cause as the dynamodb / dynamodbstreams issues filed in this sweep, reported separately because this check's service token is dynamodb__lambda__s3. It is NOT the already-filed CreateTable NotImplemented refusal (20261023T110802Z-dynamodb-stream-enablement-is-unimplemented-createtable-with-streamspecification.md): CreateTable now answers 200. The suite did not mark this check ALREADY FILED or OWNED.

Cause 1 of 1: a table created with StreamSpecification is not resolvable by DescribeTable afterwards, so the check's table_exists waiter (Delay 2, MaxAttempts 30 = exactly the 60 s harness budget) never completes and the stream-to-Lambda trigger legs never run.
AWS: Smithy (com.amazonaws.dynamodb#DynamoDB_20120810): CreateTable returns 200 with TableDescription and the table is then addressable by DescribeTable (client errors ResourceNotFoundException httpError 400, InvalidEndpointException only). StreamSpecification is an ordinary optional CreateTable member and does not change when the table becomes describable; a stream INSERT record can only exist once a write to that table succeeds.
Emulator: the check's first statement is _create_table(ctx, stream=True), whose helper (lines 1738-1770) calls create_table(... StreamSpecification={StreamEnabled:true, StreamViewType:NEW_AND_OLD_IMAGES}) and then ddb.get_waiter('table_exists').wait(TableName=name, WaiterConfig={'Delay':2,'MaxAttempts':30}); the check dies at exactly the 60 s budget with no API error, i.e. inside that 30x2 s poll. The dynamodbstreams checks in the same run show the mechanism: for the identical CreateTable shape, PutItem/DescribeTable are refused 400 ResourceNotFoundException while DescribeStream/GetShardIterator on the minted stream ARN succeed. Hypothesis: the StreamSpecification path registers the stream but not the table.
Why: the whole DynamoDB-Streams-to-Lambda fan-out is unreachable — a client cannot even wait for the source table, let alone write an item, so no INSERT record is produced and no event-source mapping can deliver one. The failure mode is a hang rather than a clean error, which is the worst shape for provisioning code.
Fix: the DynamoDB CreateTable handler and the table state it writes — register the table (and its item store) on the StreamSpecification path and advance TableStatus to ACTIVE so DescribeTable answers 200 and table_exists completes.
Traces: dynamodb__lambda__s3.ddb-stream-insert-triggers-lambda.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1): Total 0/17 passed (0.0%). Per-service: dynamodb__lambda__s3 0/1.

dynamodb__lambda__s3.ddb-stream-insert-triggers-lambda — [mismatch] 'Check timed out after 60.0s'. Body line 1454: table = _create_table(ctx, stream=True) is the first call; the helper creates the table with StreamSpecification {StreamEnabled:true, StreamViewType:NEW_AND_OLD_IMAGES} and immediately waits table_exists with Delay 2 / MaxAttempts 30 — 60 s, exactly the observed timeout. No ClientError surfaced.
Corroborating, same run: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the PutItem operation: Requested resource not found — on 9 dynamodbstreams checks whose tables were created with the identical StreamSpecification shape, after DescribeStream and GetShardIterator on those tables' stream ARNs had succeeded.
Expected: DescribeTable answers 200 for the just-created stream-enabled table so table_exists completes in seconds.
Actual: the table is not resolvable; the waiter consumes the whole check budget.
Table names, function names and stream ARNs are generated per run and are examples only; the requirement is that CreateTable with StreamSpecification produce a table DescribeTable resolves.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Call DynamoDB CreateTable with KeySchema [{pk,HASH}], AttributeDefinitions [{pk,S}], BillingMode PAY_PER_REQUEST and StreamSpecification {StreamEnabled:true, StreamViewType:NEW_AND_OLD_IMAGES} (200, returns LatestStreamArn).
3. Run boto3's table_exists waiter with Delay 2 / MaxAttempts 30 (or poll DescribeTable) on that TableName. Observe DescribeTable refused 400 ResourceNotFoundException and the waiter never completing; expected 200 with TableStatus CREATING/ACTIVE.
4. Re-replay dynamodb__lambda__s3.ddb-stream-insert-triggers-lambda; it should get past table creation, create the ESM and observe the INSERT-triggered Lambda write.
