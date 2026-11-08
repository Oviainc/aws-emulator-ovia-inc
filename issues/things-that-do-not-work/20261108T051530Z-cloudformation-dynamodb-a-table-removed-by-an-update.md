Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb.deploy-dynamodb-2-update-rollback-removes-added-table
Issue-Key: cloudformation__dynamodb.deploy-dynamodb-2-update-rollback-removes-added-table|_client_error#25|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 3778ba40189b37afffee3a954bf30487a96c0bc7

# cloudformation__dynamodb: a table removed by an UPDATE_ROLLBACK is still served by DescribeTable (200, TableStatus DELETING) instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `3778ba40189b37afffee3a954bf30487a96c0bc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTable

## Description
Cause 1 of 1: DescribeTable keeps answering 200 for a table whose delete has been initiated, so a CloudFormation UPDATE_ROLLBACK that removes an added AWS::DynamoDB::Table never becomes observable as the table being gone.
AWS: Smithy: DescribeTable{TableName (required)} -> errors include ResourceNotFoundException (client, HTTP 400). Once a table has been deleted, DescribeTable is refused ResourceNotFoundException with the template "Requested resource not found: Table: <TableName> not found" (fixed text around the table name from the request). A table in the transient DELETING state is still described, but the delete completes and the table then stops resolving; after a CloudFormation update rollback that removes the resource, the rollback is not terminal until the table no longer resolves.
Emulator: DescribeTable answers HTTP 200 with a full Table block whose TableStatus is 'DELETING' and never progresses past it, so the call never becomes ResourceNotFoundException. The deletion that the rollback requested is accepted but never completed.
Why: a client (or CloudFormation itself) that waits for the removed resource to disappear waits forever, and any code that treats a successful DescribeTable as "the table exists" will keep using a table AWS has already removed. The rollback reads as incomplete.
Fix: the DynamoDB table-state component that drives DELETING -> deleted (and DescribeTable's lookup, which must then refuse with ResourceNotFoundException); reached here through CloudFormation's AWS::DynamoDB::Table delete during UPDATE_ROLLBACK.
Traces: cloudformation__dynamodb.deploy-dynamodb-2-update-rollback-removes-added-table

## Evidence
Replay output for cloudformation__dynamodb.deploy-dynamodb-2-update-rollback-removes-added-table:

AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'Table': {... 'TableName': '<generated>', 'TableStatus': 'DELETING', ... 'TableArn': 'arn:aws:dynamodb:us-west-2:000000000000:table/<generated>' ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Expected: HTTP 400, error code ResourceNotFoundException, message 'Requested resource not found: Table: <TableName> not found' (fixed text, the table name taken from the request).
Actual: HTTP 200 with a Table block stuck at TableStatus 'DELETING'.

(The generated table name, request id, timestamps and endpoint in the recorded response are per-run values, not part of the requirement.)

## Steps to Reproduce
1. Create a CloudFormation stack, then UpdateStack adding a second AWS::DynamoDB::Table whose update is made to fail so the stack enters UPDATE_ROLLBACK.
2. Wait for a terminal UPDATE_ROLLBACK_COMPLETE.
3. Call DescribeTable on the table the rollback removed -> expect 400 ResourceNotFoundException 'Requested resource not found: Table: <name> not found'; observe HTTP 200 with TableStatus 'DELETING' that never clears.
Equivalently, without CloudFormation: CreateTable, DeleteTable, then poll DescribeTable -> it must eventually answer ResourceNotFoundException.
