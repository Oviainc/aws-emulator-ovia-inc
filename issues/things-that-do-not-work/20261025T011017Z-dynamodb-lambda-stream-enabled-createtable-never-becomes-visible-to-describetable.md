Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.ddb-stream-esm-maximum-record-age-discards-aged-records
Issue-Key: dynamodb__lambda.ddb-stream-esm-maximum-record-age-discards-aged-records|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: a0c9c8ccc8bc5abababeef7ae6993b0ce3338eb3

# Stream-enabled CreateTable never becomes visible to DescribeTable, hanging the table_exists waiter before a DynamoDB-stream Lambda event-source mapping can be built

> _Note: this issue was identified against the code base at commit `a0c9c8ccc8bc5abababeef7ae6993b0ce3338eb3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
This issue has 1 cause.

Context: same root cause as the dynamodb / dynamodbstreams issues filed in this sweep, reported separately because this check's service token is dynamodb__lambda. It is NOT the already-filed CreateTable NotImplemented refusal (20261023T110802Z-dynamodb-stream-enablement-is-unimplemented-createtable-with-streamspecification.md): CreateTable now answers 200. The suite did not mark this check ALREADY FILED or OWNED.

Cause 1 of 1: a table created with StreamSpecification is not resolvable by DescribeTable afterwards, so the check's table_exists waiter (Delay 2, MaxAttempts 60) polls until the harness budget expires and the Lambda event-source-mapping legs never run.
AWS: Smithy (com.amazonaws.dynamodb#DynamoDB_20120810): CreateTable returns 200 with TableDescription and the table is then addressable by DescribeTable (client errors ResourceNotFoundException httpError 400, InvalidEndpointException only) and by PutItem/UpdateItem. StreamSpecification is an ordinary optional CreateTable member; supplying it does not change when the table becomes describable. A DynamoDB-stream ESM (lambda CreateEventSourceMapping with MaximumRecordAgeInSeconds) then needs both a live table to mutate and a readable stream.
Emulator: the check creates a plain audit table (no stream) and writes to it successfully, then calls _create_table(stream=True), whose next statement is ddb.get_waiter('table_exists').wait(TableName=name, WaiterConfig={'Delay':2,'MaxAttempts':60}); the check is then killed at the 60 s harness budget with no API error. The same CreateTable shape in the dynamodbstreams checks of this run shows the mechanism outright: PutItem/DescribeTable on the created table are refused 400 ResourceNotFoundException while DescribeStream/GetShardIterator on its minted stream ARN succeed. Hypothesis: the StreamSpecification path registers the stream but not the table.
Why: no DynamoDB-stream-sourced Lambda pipeline can be stood up — the table a producer writes to never materialises, so MaximumRecordAgeInSeconds discard behaviour, retry exhaustion and the on-failure path are all unreachable, and a client's provisioning step hangs instead of failing fast.
Fix: the DynamoDB CreateTable handler and the table state it writes — register the table (and its item store) on the StreamSpecification path and advance TableStatus to ACTIVE so DescribeTable answers 200 and table_exists completes.
Traces: dynamodb__lambda.ddb-stream-esm-maximum-record-age-discards-aged-records.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1): Total 0/17 passed (0.0%). Per-service: dynamodb__lambda 0/1.

dynamodb__lambda.ddb-stream-esm-maximum-record-age-discards-aged-records — [mismatch] 'Check timed out after 60.0s'. Body: sink = _create_table(ctx, 'audit') (no stream) and ddb.put_item on it both succeed, then subject = _create_table(ctx, 'src-aged-out', stream=True), whose helper (lines 1273-1294) issues create_table(... StreamSpecification={StreamEnabled:true, StreamViewType:NEW_AND_OLD_IMAGES}) and immediately ddb.get_waiter('table_exists').wait(TableName=name, WaiterConfig={'Delay':2,'MaxAttempts':60}) — a 120 s poll, cut off at the 60 s budget. No ClientError was raised, so the run was inside that waiter.
Corroborating, same run: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the PutItem operation: Requested resource not found — on 9 dynamodbstreams checks, against tables created with the identical StreamSpecification shape.
Expected: DescribeTable answers 200 for the just-created stream-enabled table so table_exists completes in seconds.
Actual: the table is not resolvable; the waiter exhausts the check budget.
Table names, function names, role names and stream ARNs are generated per run and are examples only; the requirement is that CreateTable with StreamSpecification produce a table DescribeTable resolves.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Call DynamoDB CreateTable with KeySchema [{pk,HASH}], AttributeDefinitions [{pk,S}], BillingMode PAY_PER_REQUEST and StreamSpecification {StreamEnabled:true, StreamViewType:NEW_AND_OLD_IMAGES} (200, returns LatestStreamArn).
3. Run boto3's table_exists waiter (or poll DescribeTable) on that TableName. Observe DescribeTable refused 400 ResourceNotFoundException and the waiter never completing; expected 200 with TableStatus CREATING/ACTIVE.
4. Re-replay dynamodb__lambda.ddb-stream-esm-maximum-record-age-discards-aged-records; it should get past table creation and on to CreateEventSourceMapping with MaximumRecordAgeInSeconds.
