Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3.ddb-stream-insert-triggers-lambda
Issue-Key: dynamodb__lambda__s3.ddb-stream-insert-triggers-lambda|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: 450f7a190afe0db2852dff5128e985260e643504

# dynamodb__lambda__s3: a DynamoDB-Streams event source mapping never delivers INSERT records to its Lambda, so the stream-to-Lambda-to-table flow hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `450f7a190afe0db2852dff5128e985260e643504`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: a DynamoDB Streams -> Lambda event source mapping produces no invocation, so nothing the function would write downstream ever appears.

AWS: recorded behavior only — nothing in the Smithy models states the delivery timing, so the requirement is what the recording shows. A stream-enabled DynamoDB table wired to a Lambda function by CreateEventSourceMapping(EventSourceArn=<table stream ARN>, StartingPosition=TRIM_HORIZON) delivers each committed mutation to the function as a Records batch whose entries carry eventName and, for a NEW_AND_OLD_IMAGES/NEW_IMAGE stream, dynamodb.NewImage. In the recorded flow the handler reacts to the INSERT by PutItem-ing a derived item into a second table, and a GetItem probe on that second table finds the item (the check allows up to 150 s for cold start plus first delivery). The flow then tears down in reverse order and each resource reports gone.

Emulator: the check never completes — the harness killed it at its 60 s cap with no assertion reached, so the derived item never appeared in the output table and the stream-to-Lambda hop never fired. Because the kill is a timeout and not an assertion, the exact blocking call is not recorded in the replay output. Hypothesis: the same blocking point as the sibling checks with the identical 60 s-timeout symptom that are already filed — a stream-enabled CreateTable that never becomes visible to DescribeTable, hanging the table_exists waiter before the ESM is even created (see open issues 20261025T011017Z-dynamodb-lambda-stream-enabled-createtable-never-becomes-visible-to-describetable.md and 20261025T011017Z-dynamodb-createtable-with-streamspecification-leaves-the-table-invisible-to.md). That hypothesis must be confirmed by re-running this check; if the table does materialize, the defect is the ESM delivery hop itself. Neither of those issues, nor 20261022T170000Z-dynamodbstreams-item-writes-publish-no-stream-records.md, nor 20261025T022656Z-dynamodb-lambda-s3-an-s3-putobject-never-invokes.md (which is the S3-notification hop, a different source), carries this check's replay key — the suite reported it unowned.

Why: a client relying on AWS's behavior gets no change-data-capture at all. Every serverless pattern that fans a DynamoDB table's mutations out through an event source mapping — aggregation tables, search-index sync, outbox publishing, audit trails — silently does nothing against the emulator: the write succeeds, the function is never invoked, and the downstream table stays empty with no error anywhere for the client to observe.

Fix: the DynamoDB-Streams event-source poller behind lambda CreateEventSourceMapping (the component that opens shard iterators on the source stream ARN and invokes the function with the batch), plus whatever blocks the stream-enabled CreateTable from becoming visible to DescribeTable in the dynamodb table state if the hypothesis above holds.

Traces: dynamodb__lambda__s3.ddb-stream-insert-triggers-lambda

## Evidence
Check dynamodb__lambda__s3.ddb-stream-insert-triggers-lambda — verdict: mismatch.
  Error: Check timed out after 60.0s (no assertion reached; harness cap, not an assertion failure).

Expected (recorded flow): after CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<table stream ARN>, StartingPosition="TRIM_HORIZON") reports Enabled, a PutItem of {"pk":{"S":"trigger-insert"}} into the source table causes the function to run and PutItem {"pk":{"S":"trigger-insert"},"event":{"S":"INSERT"}} into the output table; GetItem(out_table, {"pk":{"S":"trigger-insert"}}) returns that Item within the 150 s the check allows, with Item["event"]["S"] == "INSERT".

Actual (emulator): the run never reaches the probe's success condition and is killed at 60 s. No item is ever written to the output table, so no INSERT record was delivered to the function.

Related-but-not-covering open issues (named per instructions, none carries this check id): 20261025T011017Z-dynamodb-lambda-stream-enabled-createtable-never-becomes-visible-to-describetable.md, 20261025T011017Z-dynamodb-createtable-with-streamspecification-leaves-the-table-invisible-to.md, 20261022T170000Z-dynamodbstreams-item-writes-publish-no-stream-records.md, 20261025T022656Z-dynamodb-lambda-s3-an-s3-putobject-never-invokes.md.

## Steps to Reproduce
1. CreateTable a source table with StreamSpecification enabled (NEW_AND_OLD_IMAGES) and wait for it to exist; CreateTable a second, plain output table.
2. Create an IAM role trusted by lambda.amazonaws.com allowing dynamodb:GetRecords/GetShardIterator/DescribeStream/ListStreams on <source table arn>/stream/* and dynamodb:PutItem on the output table arn.
3. CreateFunction with a handler that, for each record with eventName == "INSERT", PutItem-s {"pk": record.dynamodb.NewImage.pk, "event": "INSERT"} into the output table.
4. CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<source table LatestStreamArn>, StartingPosition="TRIM_HORIZON"); wait until it reports Enabled.
5. PutItem(TableName=<source table>, Item={"pk": {"S": "trigger-insert"}}).
6. Poll GetItem(TableName=<output table>, Key={"pk": {"S": "trigger-insert"}}) -> against the emulator the item never appears and the check is killed at the 60 s cap; against AWS it appears with event == "INSERT".
