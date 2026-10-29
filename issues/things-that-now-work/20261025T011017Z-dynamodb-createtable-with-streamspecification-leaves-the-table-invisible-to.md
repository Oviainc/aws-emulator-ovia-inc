Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.stream-iterator-at-and-after-sequence-number-resume-exactly
Issue-Key: dynamodb.stream-iterator-at-and-after-sequence-number-resume-exactly|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: a0c9c8ccc8bc5abababeef7ae6993b0ce3338eb3

# CreateTable with StreamSpecification leaves the table invisible to DescribeTable, so the table_exists waiter never completes and sequence-number iterator checks never reach their first stream read

> _Note: this issue was identified against the code base at commit `a0c9c8ccc8bc5abababeef7ae6993b0ce3338eb3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
This issue has 1 cause.

Context: this is the dynamodb-service face of the same stream-enabled-table registration gap reported for dynamodbstreams in this sweep; it is NOT the already-filed CreateTable NotImplemented refusal (20261023T110802Z-dynamodb-stream-enablement-is-unimplemented-createtable-with-streamspecification.md), because CreateTable now answers 200 with a LatestStreamArn. The suite did not mark this check ALREADY FILED or OWNED.

Cause 1 of 1: after CreateTable with StreamSpecification returns 200, DescribeTable on that table name does not report the table, so the standard table_exists waiter never completes and nothing downstream of table creation runs.
AWS: Smithy (com.amazonaws.dynamodb#DynamoDB_20120810): CreateTable returns 200 with TableDescription; DescribeTable's only client errors are ResourceNotFoundException (httpError 400) and InvalidEndpointException. After a successful CreateTable, DescribeTable answers for the table — TableStatus CREATING and then ACTIVE within seconds — which is exactly what every SDK's table_exists waiter polls. StreamSpecification is an ordinary optional CreateTable input member and does not change that lifecycle.
Emulator: the check calls CreateTable(StreamSpecification={StreamEnabled:true,StreamViewType:NEW_AND_OLD_IMAGES}) and then ddb.get_waiter('table_exists').wait(TableName=table) as its very next call; the check never gets past that line and is killed by the 60 s harness budget with no API error reported. In the same suite run, the dynamodbstreams checks show the mechanism directly: for a table created the same way, PutItem/DescribeTable are refused 400 ResourceNotFoundException while DescribeStream and GetShardIterator on the minted stream ARN both succeed. Hypothesis: the stream-enabled CreateTable path registers the stream but not the table, so DescribeTable keeps answering ResourceNotFoundException and the waiter polls until the budget is gone.
Why: the first thing any client does after CreateTable is wait for the table — boto3/CLI/CDK all use the table_exists waiter or poll DescribeTable. With a stream-enabled table that wait never terminates, so provisioning scripts hang rather than failing fast, and AT_SEQUENCE_NUMBER / AFTER_SEQUENCE_NUMBER checkpoint semantics (the only iterator types a real consumer uses to resume) remain completely unexercised.
Fix: the DynamoDB CreateTable handler and the table state it writes — register the table in the store DescribeTable/PutItem read on the StreamSpecification path too, and advance TableStatus to ACTIVE, so DescribeTable answers 200 and table_exists completes.
Traces: dynamodb.stream-iterator-at-and-after-sequence-number-resume-exactly.

## Evidence
Behavioral suite (simparity-behavioral-ga-prod-v1): Total 0/17 passed (0.0%). Per-service: dynamodb 0/2 (the other dynamodb check is OWNED by an existing issue).

dynamodb.stream-iterator-at-and-after-sequence-number-resume-exactly — [mismatch] 'Check timed out after 60.0s'. The check body (lines 307-317) is: create_table(..., StreamSpecification={StreamEnabled:true, StreamViewType:NEW_AND_OLD_IMAGES}) then ddb.get_waiter('table_exists').wait(TableName=table) before any other call. No ClientError surfaced, so the time was spent inside a polling loop, and the only loop reachable before the first assertion is that waiter.
Corroborating evidence from the same run (identical CreateTable shape, different service token): botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the PutItem operation: Requested resource not found — raised on 9 dynamodbstreams checks immediately after DescribeStream/GetShardIterator succeeded for the stream of the table just created.
Expected: DescribeTable answers 200 for the just-created table (TableStatus CREATING then ACTIVE) so table_exists completes in seconds.
Actual: the table is not resolvable by DescribeTable; the waiter never completes.
Table names, stream ARNs and sequence numbers are generated per run and are examples only; the requirement is that DescribeTable resolve a table CreateTable has just accepted, whether or not StreamSpecification was supplied.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. Call DynamoDB CreateTable with TableName=<new>, KeySchema [{pk,HASH}], AttributeDefinitions [{pk,S}], BillingMode PAY_PER_REQUEST, StreamSpecification {StreamEnabled:true, StreamViewType:NEW_AND_OLD_IMAGES}. It answers 200 with a LatestStreamArn.
3. Immediately call DescribeTable on that TableName (or run boto3's table_exists waiter). Observe it is refused 400 ResourceNotFoundException and the waiter never completes; expected 200 with TableStatus CREATING/ACTIVE.
4. Re-replay dynamodb.stream-iterator-at-and-after-sequence-number-resume-exactly; it should get past table creation and exercise TRIM_HORIZON / AT_SEQUENCE_NUMBER / AFTER_SEQUENCE_NUMBER.
