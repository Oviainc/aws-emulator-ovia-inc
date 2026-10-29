Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.execute-statement-invalid-partiql-rejected dynamodb.partiql-delete-returning-all-new-rejected dynamodb.partiql-update-and-delete-returning-images dynamodb.get-resource-policy-malformed-arn-rejected dynamodb.put-resource-policy-oversized dynamodb.table-arn-accepted-wherever-table-name-is dynamodb.x-amz-crc32-matches-body-on-success-and-error dynamodb.stream-records-iterator-and-images dynamodb.transact-write-items-consumes-twice-a-plain-write
Issue-Key: dynamodb.execute-statement-invalid-partiql-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: dynamodb.partiql-delete-returning-all-new-rejected|run#12|AssertionError|assert message == _EXPECTED_MESSAGE, (
Issue-Key: dynamodb|ExecuteStatement|ConditionalCheckFailedException
Issue-Key: dynamodb.get-resource-policy-malformed-arn-rejected|run#32|AssertionError|assert message_missing == expected_missing, (
Issue-Key: dynamodb.put-resource-policy-oversized|run#18|AssertionError|assert code == 'ValidationException', (
Issue-Key: dynamodb.table-arn-accepted-wherever-table-name-is|run#20|AssertionError|assert message == expected_message, (
Issue-Key: dynamodb.x-amz-crc32-matches-body-on-success-and-error|last#10|AssertionError|assert value is not None, f"{op} {status}: no x-amz-crc32 header among {sorted(lowered)!r}"
Issue-Key: dynamodb.stream-records-iterator-and-images|run#45|AssertionError|assert seqs[0] > shard_start[history_shard], (
Issue-Key: dynamodb.transact-write-items-consumes-twice-a-plain-write|run#10|AssertionError|assert tx.get('WriteCapacityUnits') == tx['CapacityUnits'], (
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# DynamoDB PartiQL, resource policy, ARN addressing, CRC32 header and transaction metering: 9 causes

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Nine net-new root causes, all in the `dynamodb` service. Each needs its own fix.

Cause 1 of 9: malformed PartiQL statement message.
AWS: `ValidationException` with `Statement wasn't well formed, can't be processed: Expected data manipulation` — fixed text. Smithy: ExecuteStatement.Statement -> PartiQLStatement @length 1..8192; errors include ValidationException (service-common).
Emulator: `Invalid Statement: statement wasn't well formed, could not be parsed`.
Why: PartiQL clients key on the service's parse-error text.
Fix: ExecuteStatement parser error message.
Traces: dynamodb.execute-statement-invalid-partiql-rejected

Cause 2 of 9: PartiQL DELETE ... RETURNING ALL NEW refusal message.
AWS: `Invalid returning clause: RETURNING ALL NEW *. Only RETURNING ALL OLD * is allowed in DELETE statements.` — fixed text; the echoed clause comes from the statement.
Emulator: `Invalid Statement: a DELETE can only return the item as it stood before it: ALL OLD or MODIFIED OLD`.
Why: invented prose for a documented refusal.
Fix: ExecuteStatement RETURNING-clause validation message.
Traces: dynamodb.partiql-delete-returning-all-new-rejected

Cause 3 of 9: PartiQL UPDATE/DELETE returning images is refused as a failed condition.
AWS: the statements in this check succeed and return the requested OLD/NEW images. Smithy: ExecuteStatement models ConditionalCheckFailedException, but only for a statement carrying a condition that actually fails.
Emulator: answers `ConditionalCheckFailedException` (HTTP 400) `The conditional request failed` to an ExecuteStatement that should have been served.
Why: a client cannot use PartiQL returning-images at all; a legal statement is refused.
Fix: ExecuteStatement UPDATE/DELETE path (condition evaluation and returned images).
Traces: dynamodb.partiql-update-and-delete-returning-images

Cause 4 of 9: GetResourcePolicy not-found message names the ARN instead of the table.
AWS: `Requested resource not found: Table: <table name> not found` — the fixed frame names `Table:` and the table name extracted from the ResourceArn.
Emulator: `Requested resource not found: ResourceArn: arn:aws:dynamodb:<region>:<account>:table/<table name> not found`.
Why: message shape differs for a routine not-found; callers scraping the resource name get an ARN.
Fix: GetResourcePolicy not-found message construction.
Traces: dynamodb.get-resource-policy-malformed-arn-rejected

Cause 5 of 9: oversized PutResourcePolicy returns the wrong refusal.
AWS: an over-limit policy document is rejected as a size-limit violation in AWS's wording for that class. Smithy: PutResourcePolicy.Policy -> ResourcePolicy; modeled errors LimitExceededException, PolicyNotFoundException, ResourceInUseException, ResourceNotFoundException (+ ValidationException).
Emulator: `LimitExceededException` with `Policy size exceeds the maximum supported size of 20480 bytes for resource <resource ARN>: <actual bytes> bytes` — an emulator-authored message that the check does not accept as the size-limit rejection AWS returns.
Why: the caller sees a different error class/text than AWS for an over-size policy.
Fix: PutResourcePolicy size validation (error classification and message).
Traces: dynamodb.put-resource-policy-oversized

Cause 6 of 9: the ARN-format constraint message omits AWS's resource-name clause.
AWS: `1 validation error detected: Value '<supplied ARN>' at 'tableName' failed to satisfy constraint: Valid ARN format is 'arn:<awsPartition>:<vendor>:<region>:<subscriber>:resourceType/resourceName', where Resource name must have length less than or equal to 255, Resource name must have length greater than or equal to 3, Resource name must satisfy regular expression pattern: [a-zA-Z0-9_.-]+` — all of that is fixed text; only the quoted ARN comes from the request. Smithy: TableName targets TableArn @length 1..1024, so an ARN is accepted wherever a name is.
Emulator: `... Valid ARN format is 'arn:<awsPartition>:<vendor>:<region>:<awsAccountId>:<resourceType>/<resourceId>'` — different placeholder names and the entire resource-name clause missing.
Why: every ARN-addressing refusal on this member reports a different contract than AWS states.
Fix: the ARN parser's constraint message (shared by table-name/ARN resolution). Note: the open issue 20261027T053218Z-dynamodb-4-net-new-root-causes-awsjson-error.md owns the sibling check dynamodb.table-arn-addresses-a-table-wherever-a-name-does; it does not cover this one.
Traces: dynamodb.table-arn-accepted-wherever-table-name-is

Cause 7 of 9: no x-amz-crc32 header on any response.
AWS: DynamoDB stamps `x-amz-crc32` (the CRC32 of the response body) on success and on error responses; botocore validates it.
Emulator: the header is absent — a CreateTable 200 carried only content-length, content-type, date.
Why: SDKs that verify the checksum cannot validate any response, and the retry-on-corruption path is unreachable. (The already-open 20261027T053218Z-dynamodb-4-net-new-root-causes-awsjson-error.md covers the error-body case only; success responses are this cause.)
Fix: the DynamoDB response serializer (header emission for all responses).
Traces: dynamodb.x-amz-crc32-matches-body-on-success-and-error

Cause 8 of 9: stream record sequence numbers do not advance past the shard's StartingSequenceNumber.
AWS: a record's SequenceNumber sorts strictly after its own shard's StartingSequenceNumber.
Emulator: the record's sequence number equals the shard's StartingSequenceNumber (both reported as the same value).
Why: consumers that use sequence numbers to de-duplicate or to resume after a checkpoint cannot order or advance.
Fix: the stream shard/record state that mints sequence numbers.
Traces: dynamodb.stream-records-iterator-and-images

Cause 9 of 9: TransactWriteItems ConsumedCapacity omits the write arm.
AWS: with ReturnConsumedCapacity requested, a transactional write reports the write component (WriteCapacityUnits) beside CapacityUnits; a transactional write costs twice a plain write. Smithy: TransactWriteItems.ConsumedCapacity -> ConsumedCapacityMultiple.
Emulator: returns only `{'TableName': ..., 'CapacityUnits': 2.0}` with no WriteCapacityUnits member.
Why: cost attribution and capacity-planning clients cannot tell reads from writes. (Related open issue 20261025T020208Z-cloudwatch-dynamodb-dynamodb-transactwriteitems-consumedcapacity-omits-writecapacityunits-and-no-aws.md is in a different service group and does not carry this check's replay key.)
Fix: TransactWriteItems ConsumedCapacity construction.
Traces: dynamodb.transact-write-items-consumes-twice-a-plain-write

## Evidence
Replay details, one per cause:
- execute-statement-invalid-partiql-rejected: expected message "Statement wasn't well formed, can't be processed: Expected data manipulation", got "Invalid Statement: statement wasn't well formed, could not be parsed".
- partiql-delete-returning-all-new-rejected: expected 'Invalid returning clause: RETURNING ALL NEW *. Only RETURNING ALL OLD * is allowed in DELETE statements.', got 'Invalid Statement: a DELETE can only return the item as it stood before it: ALL OLD or MODIFIED OLD'.
- partiql-update-and-delete-returning-images: botocore.errorfactory.ConditionalCheckFailedException: An error occurred (ConditionalCheckFailedException) when calling the ExecuteStatement operation: The conditional request failed.
- get-resource-policy-malformed-arn-rejected: expected 'Requested resource not found: Table: <table> not found', got 'Requested resource not found: ResourceArn: arn:aws:dynamodb:us-west-2:000000000000:table/<table> not found'.
- put-resource-policy-oversized: expected a size-limit rejection for an oversized policy, got LimitExceededException ({'Message': 'Policy size exceeds the maximum supported size of 20480 bytes for resource arn:aws:dynamodb:...:table/<table>: 500253 bytes', 'Code': 'LimitExceededException'}).
- table-arn-accepted-wherever-table-name-is: expected "... Valid ARN format is 'arn:<awsPartition>:<vendor>:<region>:<subscriber>:resourceType/resourceName', where Resource name must have length less than or equal to 255, Resource name must have length greater than or equal to 3, Resource name must satisfy regular expression pattern: [a-zA-Z0-9_.-]+", got "... Valid ARN format is 'arn:<awsPartition>:<vendor>:<region>:<awsAccountId>:<resourceType>/<resourceId>'".
- x-amz-crc32-matches-body-on-success-and-error: AssertionError: CreateTable 200: no x-amz-crc32 header among ['content-length', 'content-type', 'date'].
- stream-records-iterator-and-images: AssertionError: a record must sequence after its own shard's StartingSequenceNumber (100000000000000000000), got 100000000000000000000.
- transact-write-items-consumes-twice-a-plain-write: AssertionError: TransactWriteItems reports the write arm beside the total, got {'TableName': ..., 'CapacityUnits': 2.0}.

## Steps to Reproduce
Boot the emulator and replay: dynamodb.execute-statement-invalid-partiql-rejected, dynamodb.partiql-delete-returning-all-new-rejected, dynamodb.partiql-update-and-delete-returning-images, dynamodb.get-resource-policy-malformed-arn-rejected, dynamodb.put-resource-policy-oversized, dynamodb.table-arn-accepted-wherever-table-name-is, dynamodb.x-amz-crc32-matches-body-on-success-and-error, dynamodb.stream-records-iterator-and-images, dynamodb.transact-write-items-consumes-twice-a-plain-write.
