Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodbstreams-shard-iterator-request-validation
Issue-Step: teardown#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodbstreams-shard-iterator-request-validation dynamodbstreams-stream-enable-describe-and-iterate
Issue-Shape-Id: com.amazonaws.dynamodbstreams#DynamoDBStreams_20120810
Issue-Key: dynamodb|DeleteTable|2xx->4xx|$.TableDescription
Issue-Related: 20261022T113745Z-dynamodb-tables-created-with-secondary-indexes-never-materialize-later.md
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# DeleteTable on a stream-enabled table answers 400 instead of 200 with TableDescription

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTable

## Description
This issue has 1 cause; it must be closed.

Cause 1 of 1: a table that was created with a StreamSpecification cannot be deleted - DeleteTable answers 400 instead of 200, and the entire TableDescription is absent from the response.
AWS: Smithy (com.amazonaws.dynamodb#DynamoDB_20120810): DeleteTable takes DeleteTableInput.TableName (required) and returns DeleteTableOutput.TableDescription on 200; the modeled client errors are ResourceNotFoundException, ResourceInUseException, LimitExceededException and InvalidEndpointException (all httpError 400 except InvalidEndpointException 421). ResourceNotFoundException is modeled only for a table that does not exist. The recording shows 200 with a full TableDescription whose TableStatus is DELETING and which echoes TableName, TableArn, TableId, ItemCount 0, TableSizeBytes 0, ProvisionedThroughput (NumberOfDecreasesToday/Read/WriteCapacityUnits), BillingModeSummary (BillingMode PAY_PER_REQUEST), TableThroughputModeSummary, DeletionProtectionEnabled false, StreamSpecification {StreamEnabled true, StreamViewType NEW_IMAGE}, LatestStreamArn and LatestStreamLabel.
Emulator: 400 on the same request, with no TableDescription member at all (the body carries only an error envelope).
Why: a client cannot tear down a stream-enabled table, and cannot distinguish a genuine failure from a table the service never registered; because DeleteTable is the documented way to read back the DELETING transition plus the table's LatestStreamArn, a client that waits for DELETING never proceeds. Any workflow that creates a stream-enabled table and cleans it up leaks the table and then fails.
Hypothesis: the table created with a StreamSpecification is never registered in the table registry (the same shape of failure the already-open issue 20261022T113745Z-dynamodb-tables-created-with-secondary-indexes-never-materialize-later.md describes for index-bearing tables), so DeleteTable reports the table as absent. That open issue does NOT cover this: its replay keys are dynamodb|DeleteTable|2xx->4xx|status, while this divergence additionally loses the whole $.TableDescription payload and arises from a StreamSpecification create, not a secondary-index create.
Fix: the CreateTable handler / table registry in the dynamodb service state (a create carrying StreamSpecification must insert the table), and the DeleteTable handler's response serializer, which must emit the full TableDescription (including StreamSpecification and the stream ARN/label) with TableStatus DELETING.
Traces: dynamodbstreams-shard-iterator-request-validation#teardown#9 (2 steps in cluster; the other is dynamodbstreams-stream-enable-describe-and-iterate#teardown#6).

## Evidence
Representative: fixture dynamodbstreams-shard-iterator-request-validation, step teardown#9, operation DeleteTable, expected 200 / actual 400.
  status: expected 200, got 400
  $.TableDescription: missing - expected a TableDescription with TableStatus DELETING, StreamSpecification {StreamEnabled: true, StreamViewType: NEW_IMAGE}, BillingModeSummary {BillingMode: PAY_PER_REQUEST}, ProvisionedThroughput {NumberOfDecreasesToday: 0, ReadCapacityUnits: 0, WriteCapacityUnits: 0}, ItemCount 0, TableSizeBytes 0, DeletionProtectionEnabled false, TableArn/TableName for the table under test, plus LatestStreamArn and LatestStreamLabel; got None.
Cluster member: dynamodbstreams-stream-enable-describe-and-iterate#teardown#6 (same divergence key dynamodb|DeleteTable|status=200->400|keys=$.TableDescription,status).
Table names, ARNs, table ids, stream labels and timestamps in the recording are environment-specific examples; the requirement is the 200 status and the presence/shape of TableDescription.

## Steps to Reproduce
1. Replay the wire corpus (replay_wire_corpus).
2. Fixture dynamodbstreams-shard-iterator-request-validation: CreateTable with StreamSpecification {StreamEnabled: true, StreamViewType: NEW_IMAGE}, then at teardown#9 call DeleteTable on that table name.
3. Observe 400 with an error envelope instead of 200 with TableDescription.TableStatus = DELETING.
4. The same occurs at dynamodbstreams-stream-enable-describe-and-iterate#teardown#6.
