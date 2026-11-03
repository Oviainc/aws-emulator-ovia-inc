Issue-Severity: HIGH
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# DynamoDB Kinesis streaming destinations: invalid ARNs are refused synchronously instead of settling to ENABLE_FAILED, a second destination is accepted, precision is ignored and no record is ever delivered (6 causes)

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableKinesisStreamingDestination

## Description
Six net-new root causes in the DynamoDB -> Kinesis change-data-capture surface. Open issues 20261103T022510Z-dynamodb-kinesis-streaming-destinations-invalid-arns-are-refused.md and 20261027T001720Z-dynamodb-kinesis-enablekinesisstreamingdestination-omits-enablekinesisstreamingconfiguration-from-its-response.md describe neighbouring ground; the suite did not mark any of the checks below as owned by them.

=== Cause 1 of 6: a StreamArn that clears the length floor but names no reachable stream is refused synchronously instead of settling to ENABLE_FAILED ===
AWS: Smithy models EnableKinesisStreamingDestination.StreamArn as StreamArn with @length min 37 max 1024, and the operation's modeled errors are InternalServerError, InvalidEndpointException, LimitExceededException, ResourceInUseException and ResourceNotFoundException — the shape is validated on the wire, but a StreamArn that clears the length floor and is syntactically accepted is taken ASYNCHRONOUSLY: the call returns DestinationStatus ENABLING and the destination then settles to ENABLE_FAILED, which DescribeKinesisStreamingDestination reports and which the ACTIVE-only filter excludes. This holds for a malformed-but-long-enough ARN and for a cross-region stream ARN.
Emulator: EnableKinesisStreamingDestination answers ValidationException 'Invalid StreamArn: Invalid StreamArn provided as input <value>' synchronously, so the destination never exists and ENABLE_FAILED is never reachable; the two checks that wait for that terminal state hang to the 60 s harness cap, as does the ACTIVE-filter check that needs an ENABLE_FAILED sibling.
Why: a client that enables a destination and then polls DescribeKinesisStreamingDestination for the terminal status never sees ENABLE_FAILED, and the ARN rejection is not an error AWS serves for this input.
Fix: the EnableKinesisStreamingDestination handler must accept a length-valid StreamArn, record the destination, and drive it to ENABLE_FAILED in the destination state machine rather than refusing at the door.
Traces: ddbkds-malformed-stream-arn-settles-to-enable-failed, ddbkds-cross-region-stream-arn-settles-to-enable-failed, ddbkds-enable-failed-destination-is-excluded-by-the-active-filter.

=== Cause 2 of 6: the re-enable refusal names the stream instead of the state rule ===
AWS: enabling a destination that is already enabled is refused with the fixed sentence "Table is not in a valid state to enable Kinesis Streaming Destination: EnableKinesisStreamingDestination must be DISABLED or ENABLE_FAILED to perform ENABLE operation." — it names the STATE PRECONDITION, not the stream or table.
Emulator: 'Table is not in a valid state to enable Kinesis Streaming Destination: Streaming destination with streamArn: <arn> is already enabled for table with tableName: <table>'.
Why: the message is part of the contract and the emulator's version discloses identifiers AWS withholds while omitting the precondition the caller must satisfy.
Fix: the state-precondition check's message in the EnableKinesisStreamingDestination handler.
Traces: ddbkds-re-enabling-the-active-stream-rejected.

=== Cause 3 of 6: a SECOND streaming destination is accepted while one is already ACTIVE ===
AWS: a table may carry one Kinesis streaming destination; enabling a second while one is ACTIVE is refused.
Emulator: the call succeeds and returns the second StreamArn with DestinationStatus ENABLING.
Why: the table ends up with a destination topology AWS cannot produce, so record delivery and DescribeKinesisStreamingDestination output diverge from AWS for the rest of the table's life.
Fix: the per-table destination-cardinality check in the EnableKinesisStreamingDestination handler/state.
Traces: ddbkds-second-stream-while-one-is-active-rejected.

=== Cause 4 of 6: ApproximateCreationDateTimePrecision does not govern the unit of a delivered record's creation time ===
AWS: EnableKinesisStreamingConfiguration.ApproximateCreationDateTimePrecision sets the unit (MILLISECOND vs MICROSECOND) of ApproximateCreationDateTime on every record the table publishes, and an UpdateKinesisStreamingDestination that changes it switches the unit for records written AFTER the change while leaving earlier records alone.
Emulator: no record ever reaches the Kinesis stream, so neither the initial precision nor the switch is observable; both checks hang to the 60 s cap.
Why: a consumer reading ApproximateCreationDateTime cannot be developed or tested at all, and a precision change cannot be verified.
Fix: the DynamoDB -> Kinesis record publisher and its ApproximateCreationDateTime rendering, driven by the destination's configured precision.
Traces: ddbkds-destination-precision-sets-the-unit-of-each-record-creation-time, ddbkds-precision-update-switches-the-creation-time-unit-of-later-records.

=== Cause 5 of 6: deleting a streaming table does not leave the stream and its already-delivered records ===
AWS: DeleteTable on a table with an ACTIVE Kinesis destination removes the table but leaves the Kinesis stream and the records already delivered to it intact and readable.
Emulator: the check never reaches a verdict, hanging to the 60 s cap — with no records ever delivered (Cause 4) the post-delete read has nothing to find.
Why: an operator cannot verify that CDC data survives the source table, which is the whole point of streaming to Kinesis.
Fix: the record publisher (Cause 4) plus the DeleteTable path's handling of an attached destination.
Traces: ddbkds-deleting-a-streaming-table-leaves-its-stream-and-delivered-records.

=== Cause 6 of 6: a CloudFormation-deployed streaming table does not exist afterwards ===
AWS: a stack that declares the streaming table reaches CREATE_COMPLETE only once the table exists, and DescribeTable then answers for it.
Emulator: the lifecycle's deploy phase fails with ResourceNotFoundException 'Requested resource not found: Table: <stack-table> not found' after the deploy, so the all-regions listing lifecycle cannot start. Open issue 20261025T015306Z-cloudformation-cannot-deploy-large-parts-of-the-resource.md describes the same CloudFormation gap for other resources but does not carry this replay key.
Why: every deploy-shaped lifecycle over a streaming table is unreachable.
Fix: the CloudFormation AWS::DynamoDB::Table provisioner must create the native table (and its streaming destination) before reporting the resource complete.
Traces: e2e-ddbkds-list-streaming-tables-all-regions-lifecycle.

## Evidence
Cause 1 — ddbkds-malformed-stream-arn-settles-to-enable-failed: botocore ClientError (ValidationException) when calling EnableKinesisStreamingDestination: 'Invalid StreamArn: Invalid StreamArn provided as input this-is-not-an-arn-but-clears-the-length-floor'. ddbkds-cross-region-stream-arn-settles-to-enable-failed and ddbkds-enable-failed-destination-is-excluded-by-the-active-filter: Check timed out after 60.0s.

Cause 2 — ddbkds-re-enabling-the-active-stream-rejected: expected message 'Table is not in a valid state to enable Kinesis Streaming Destination: EnableKinesisStreamingDestination must be DISABLED or ENABLE_FAILED to perform ENABLE operation.', got 'Table is not in a valid state to enable Kinesis Streaming Destination: Streaming destination with streamArn: arn:aws:kinesis:us-west-2:<acct>:stream/<name>-cdc is already enabled for table with tableName: <table>'.

Cause 3 — ddbkds-second-stream-while-one-is-active-rejected: the call succeeded; real AWS rejects it. It returned {'TableName': '<table>', 'StreamArn': 'arn:aws:kinesis:us-west-2:<acct>:stream/<name>-second', 'DestinationStatus': 'ENABLING'} at HTTP 200.

Cause 4 — ddbkds-destination-precision-sets-the-unit-of-each-record-creation-time and ddbkds-precision-update-switches-the-creation-time-unit-of-later-records: Check timed out after 60.0s.

Cause 5 — ddbkds-deleting-a-streaming-table-leaves-its-stream-and-delivered-records: Check timed out after 60.0s.

Cause 6 — e2e-ddbkds-list-streaming-tables-all-regions-lifecycle: _PhaseError: [phase 1/4 deploy] ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTable operation: Requested resource not found: Table: <stack>-home-streaming not found.

## Steps to Reproduce
1. Create a table and a Kinesis stream.
2. ddb.enable_kinesis_streaming_destination(TableName=t, StreamArn='this-is-not-an-arn-but-clears-the-length-floor') — expect HTTP 200 with DestinationStatus ENABLING, then poll describe_kinesis_streaming_destination until ENABLE_FAILED; observe ValidationException 'Invalid StreamArn: ...'.
3. Enable a real stream, wait for ACTIVE, then enable it again and read Error.Message — expect the 'must be DISABLED or ENABLE_FAILED to perform ENABLE operation.' sentence.
4. With one destination ACTIVE, enable a SECOND stream on the same table — expect a refusal, observe DestinationStatus ENABLING.
5. Enable with EnableKinesisStreamingConfiguration ApproximateCreationDateTimePrecision=MICROSECOND, put items, and read the records off the Kinesis stream; then update_kinesis_streaming_destination to MILLISECOND, put more items and re-read.
6. Delete the table and re-read the Kinesis stream for the records already delivered.
7. Deploy the streaming-table stack and call describe_table on the created table.
Then replay the dynamodb__kinesis checks named in trace_ids.
