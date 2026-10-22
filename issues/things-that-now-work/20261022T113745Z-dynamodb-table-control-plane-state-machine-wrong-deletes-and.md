Issue-Source: trace-sweep-divergence
Issue-Trace: ddbctl-delete-table-in-transitional-state-conflict
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ddbctl-delete-table-in-transitional-state-conflict dynamodb-table-billing-mode-update dynamodb-table-control-plane-lifecycle
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|DeleteTable|4xx->2xx|$.__type
Issue-Key: dynamodb|DeleteTable|4xx->2xx|$.message
Issue-Key: dynamodb|DeleteTable|4xx->2xx|status
Issue-Key: dynamodb|UpdateTable|4xx->4xx|$.__type
Issue-Key: dynamodb|UpdateTable|4xx->4xx|$.message
Issue-Key: dynamodb|UpdateTable|2xx->2xx|$.TableDescription.BillingModeSummary.LastUpdateToPayPerRequestDateTime
Issue-Key: dynamodb|UpdateTable|2xx->2xx|$.TableDescription.TableStatus
Issue-Key: dynamodb|UpdateTable|2xx->4xx|$.TableDescription
Issue-Key: dynamodb|UpdateTable|2xx->4xx|status
Issue-Key: dynamodb|DeleteTable|2xx->4xx|$.TableDescription
Issue-Key: dynamodb|DescribeTable|4xx->2xx|$
Issue-Key: dynamodb|DescribeTable|4xx->2xx|$.__type
Issue-Key: dynamodb|DescribeTable|4xx->2xx|$.message
Issue-Key: dynamodb|DescribeTable|4xx->2xx|status
Issue-Related: 20261022T113745Z-dynamodb-table-description-responses-omit-tableid-numberofdecreasestoday-and.md
Issue-Related: 20261022T113745Z-dynamodb-tables-created-with-secondary-indexes-never-materialize-later.md
Issue-Commit: 38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5

# Table control-plane state machine wrong: deletes and updates never settle, transitional conflicts unreported

> _Note: this issue was identified against the code base at commit `38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTable

## Description
The table lifecycle state machine does not match AWS: a table left in a transitional state is not reported as in-use, an UpdateTable leaves the table stuck in UPDATING so the next control-plane call is refused, a no-op UpdateTable is misclassified, and a deleted table never disappears. This bundle has 6 causes; all 6 must be closed.

Cause 1 of 6: a second DeleteTable on a table that is already DELETING succeeds instead of conflicting.
AWS: Smithy: DeleteTable errors include ResourceInUseException (client, httpError 400). The recording shows 400 with __type com.amazonaws.dynamodb.v20120810#ResourceInUseException and a message of the form 'Attempt to change a resource which is still in use: Table is being deleted: <name>'.
Emulator: 200 with a TableDescription, as if the delete were accepted again.
Why: a client that relies on ResourceInUseException to serialize control-plane changes believes two concurrent deletes both succeeded and never backs off.
Fix: the DeleteTable handler's state-machine guard plus the table state's status tracking (DELETING must be a terminalish state that refuses further mutation).
Traces: ddbctl-delete-table-in-transitional-state-conflict#task#6.

Cause 2 of 6: an UpdateTable with no updatable member is misclassified as ResourceInUseException. AWS: Smithy: UpdateTable errors include ResourceInUseException and the service-common ValidationException; the recording shows 400 with __type com.amazon.coral.validate#ValidationException and a message listing the alternatives ('At least one of ProvisionedThroughput, BillingMode, ... or TableClass is required'). Emulator: 400 with __type 'ResourceInUseException' ('Table is being updated'). Why: the client treats a permanently invalid request as a transient conflict and retries forever. Fix: UpdateTable input validation (reject an empty update before the in-use check), and the state machine that wrongly believes the table is still updating. Traces: dynamodb-table-billing-mode-update#task#4.

Cause 3 of 6: UpdateTable reports TableStatus UPDATING where AWS reports ACTIVE. AWS: Smithy: TableDescription.TableStatus; the recording of this update shows ACTIVE in the UpdateTable response. Emulator: 'UPDATING' (and TableId, NumberOfDecreasesToday, BillingModeSummary.LastUpdateToPayPerRequestDateTime, TableThroughputModeSummary and WarmThroughput missing). Why: a client that waits for ACTIVE blocks, and the table is then treated as busy by the emulator's own guards (Causes 2 and 4). Fix: UpdateTable's status transition in table state + its TableDescription serializer. Traces: dynamodb-table-control-plane-lifecycle#task#10.

Cause 4 of 6: the next UpdateTable is refused with 400 because the table is stuck UPDATING. AWS: Smithy: UpdateTable -> 200 with TableDescription (recording: TableStatus ACTIVE, full description). Emulator: 400, TableDescription absent. Why: consecutive control-plane changes - exactly what a provisioning tool does - fail. Fix: the status transition from Cause 3 (an update must settle to ACTIVE). Traces: dynamodb-table-control-plane-lifecycle#task#11.

Cause 5 of 6: DeleteTable of that table answers 400 instead of 200 with a DELETING description. AWS: Smithy: DeleteTable -> 200 TableDescription; the recording shows TableStatus DELETING. Emulator: 400, TableDescription absent. Why: the table can never be torn down. Fix: same stuck-state root; DeleteTable guard. Traces: dynamodb-table-control-plane-lifecycle#teardown#12.

Cause 6 of 6: after DeleteTable, DescribeTable still returns an ACTIVE table forever. AWS: Smithy: DescribeTable errors include ResourceNotFoundException (400); the recording shows 400 with __type com.amazonaws.dynamodb.v20120810#ResourceNotFoundException once the delete completes. Emulator: 200 with the table still ACTIVE (and DeletionProtectionEnabled true) through the whole 60s poll. Why: a client cannot observe deletion completion, and a recreate of the same name will collide. Fix: DeleteTable must remove the table from state (and deletion protection must be rejected at delete time rather than silently preserving the table). Traces: dynamodb-table-control-plane-lifecycle#teardown#13.

## Evidence
Representative: fixture ddbctl-delete-table-in-transitional-state-conflict step task#6, DeleteTable, expected 400 / actual 200:
  status: expected 400, got 200
  $.__type: missing (expected 'com.amazonaws.dynamodb.v20120810#ResourceInUseException', got None)
  $.message: missing (example: 'Attempt to change a resource which is still in use: Table is being deleted: <name>')
Other causes: dynamodb-table-billing-mode-update#task#4 expected __type com.amazon.coral.validate#ValidationException, got 'ResourceInUseException'; dynamodb-table-control-plane-lifecycle#task#10 $.TableDescription.TableStatus expected 'ACTIVE' got 'UPDATING'; #task#11 UpdateTable 200->400 with TableDescription missing; #teardown#12 DeleteTable 200->400; #teardown#13 DescribeTable expected 400 ResourceNotFoundException, got 200 with TableStatus ACTIVE after 13 poll attempts. Table names/timestamps are examples; statuses and error codes are the requirement.

## Steps to Reproduce
1. Replay the wire corpus.
2. Fixture ddbctl-delete-table-in-transitional-state-conflict, step task#6: DeleteTable twice in a row; the second call returns 200 instead of 400 ResourceInUseException.
3. Fixture dynamodb-table-control-plane-lifecycle, steps task#10/task#11: UpdateTable, observe TableStatus UPDATING in the response and a 400 on the following UpdateTable.
4. Same fixture, teardown#12/teardown#13: DeleteTable returns 400 and DescribeTable keeps reporting an ACTIVE table instead of ResourceNotFoundException.
5. Fixture dynamodb-table-billing-mode-update, step task#4: UpdateTable with no updatable member returns ResourceInUseException instead of ValidationException.
