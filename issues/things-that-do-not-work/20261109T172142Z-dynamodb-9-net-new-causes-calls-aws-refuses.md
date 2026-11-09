Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.contributor-insights-disable-while-enabling-rejected dynamodb.ddb-execute-statement-and-transaction-against-absent-table dynamodb.deep-dynamodb-1-ddbvec-update-table-backfills-preexisting-items dynamodb.delete-table-source-region-within-24h-of-replica-add-rejected dynamodb.filter-expression-operator-limit-in-counts-as-one dynamodb.global-table-mrsc-unsupported-configuration-rejected dynamodb.restore-table-from-backup-sse-override-uses-customer-key dynamodb.scan-query-conditional-operator-needs-two-filter-conditions-rejected dynamodb.table-class-update-limit-exceeded-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.contributor-insights-disable-while-enabling-rejected|run#9|AssertionError|assert refused, f"DISABLE was never refused while ENABLING in {_ATTEMPTS} attempts: {tried!r}"
Issue-Key: dynamodb.ddb-execute-statement-and-transaction-against-absent-table|run#13|AssertionError|assert (code, msg) == _CANCELLED, (
Issue-Key: dynamodb.deep-dynamodb-1-ddbvec-update-table-backfills-preexisting-items|run#19|AssertionError|assert now != 'CREATING', (
Issue-Key: dynamodb.delete-table-source-region-within-24h-of-replica-add-rejected|_error_code#11|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: dynamodb.filter-expression-operator-limit-in-counts-as-one|_error#8|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: dynamodb.global-table-mrsc-unsupported-configuration-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException for a 1-replica STRONG request, got {code}"
Issue-Key: dynamodb|RestoreTableFromBackup|NotImplemented
Issue-Key: dynamodb.scan-query-conditional-operator-needs-two-filter-conditions-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: dynamodb.table-class-update-limit-exceeded-rejected|_error#12|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# dynamodb: 9 net-new causes — calls AWS refuses are accepted, and refusals use the wrong error class (ExecuteTransaction, vector search, replica delete-window, table class, ConditionalOperator, resource policy, MRSC, SSE override)

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteTransaction

## Description
Nine distinct causes: either a request AWS refuses is accepted with 200, or the refusal carries the wrong error code/class. Each cause lists its own failing check ids.

Cause 1 of 9: UpdateContributorInsights DISABLE is accepted while the status is ENABLING.
AWS: `UpdateContributorInsights` is refused while the previous action is still in flight — a DISABLE issued while `ContributorInsightsStatus` reads `ENABLING` is rejected rather than applied, and the status only becomes DISABLED after a completed ENABLED state. Smithy: UpdateContributorInsights -> ContributorInsightsStatus enum (ENABLING|ENABLED|DISABLING|DISABLED|FAILED).
Emulator: across 5 attempts, DISABLE was accepted every time immediately after the status read `ENABLING` and settled to `DISABLED`; no refusal is ever produced.
Why: clients that serialise control-plane mutations by catching the in-flight refusal instead race, and the ENABLING state is not a real state.
Fix: the UpdateContributorInsights handler's in-flight state guard.
Traces: contributor-insights-disable-while-enabling-rejected.

Cause 2 of 9: ExecuteTransaction against a missing table returns a top-level ResourceNotFoundException.
AWS: a PartiQL transaction naming a table that does not exist is a cancellation, not a top-level lookup failure: it answers `TransactionCanceledException` with the fixed text `Transaction cancelled, please refer cancellation reasons for specific reasons [ResourceNotFound]` and per-statement CancellationReasons. Smithy: ExecuteTransaction declares both TransactionCanceledException and ResourceNotFoundException; the per-statement resource failure is reported through the former.
Emulator: answers top-level `ResourceNotFoundException: Requested resource not found`.
Why: a client must inspect CancellationReasons to know which statement failed; a top-level error hides the index and changes the retry class.
Fix: ExecuteTransaction's per-statement resolution, which must collect failures into cancellation reasons.
Traces: ddb-execute-statement-and-transaction-against-absent-table.

Cause 3 of 9: searching a vector index that is still CREATING answers ResourceNotFoundException.
AWS: an index that exists but has not finished building is refused with `ValidationException` naming it — fixed text `The table does not have the specified index: <IndexName>` — not with a table-level ResourceNotFoundException.
Emulator: answers `ResourceNotFoundException: Requested resource not found`, so a caller cannot tell a still-building index from a missing table.
Fix: the vector-index search path's index-state check and its error class.
Traces: deep-dynamodb-1-ddbvec-update-table-backfills-preexisting-items.

Cause 4 of 9: DeleteTable on a global table's source region within 24h of adding a replica is accepted.
AWS: refuses the delete while the replication group is inside the protection window. Smithy: DeleteTable errors include ResourceInUseException and LimitExceededException; the call must not return a TableDescription.
Emulator: the call succeeded and returned a TableDescription with TableStatus DELETING, destroying the source table.
Why: this is a data-destruction guard. Accepting the call silently removes the only writable replica.
Fix: DeleteTable's global-table replication-group guard.
Traces: delete-table-source-region-within-24h-of-replica-add-rejected.

Cause 5 of 9: a FilterExpression that exceeds the operator limit is accepted.
AWS: refuses the Scan with ValidationException when the filter expression's operator count exceeds the documented limit (an `IN (...)` list counts as one operator).
Emulator: the Scan succeeded and returned Items/Count/ScannedCount with HTTP 200.
Fix: the filter-expression operator-count validation in Scan/Query.
Traces: filter-expression-operator-limit-in-counts-as-one.

Cause 6 of 9: Scan/Query with ConditionalOperator and only one filter condition is accepted.
AWS: `ConditionalOperator` requires at least two conditions in ScanFilter/QueryFilter and is refused with ValidationException otherwise. Smithy (Scan input): ConditionalOperator enum AND|OR alongside ScanFilter.
Emulator: the Scan succeeded, returning the matching item with HTTP 200.
Fix: the legacy ScanFilter/QueryFilter validation in Scan/Query.
Traces: scan-query-conditional-operator-needs-two-filter-conditions-rejected.

Cause 7 of 9: UpdateTable past the table-class change limit is accepted.
AWS: refuses a table-class change that exceeds the per-table daily limit with `LimitExceededException` (Smithy: UpdateTable errors include LimitExceededException at HTTP 400).
Emulator: the call succeeded, returning a TableDescription with TableStatus UPDATING and a TableClassSummary showing the new class.
Fix: the table-class change counter/limit in UpdateTable.
Traces: table-class-update-limit-exceeded-rejected.

Cause 8 of 9: an unsupported MultiRegionConsistency configuration answers NotImplemented instead of ValidationException.
AWS: `UpdateTable` with `MultiRegionConsistency: STRONG` on a configuration AWS does not support (a single-replica request) is refused with `ValidationException` describing the unsupported configuration. Smithy: UpdateTable input members MultiRegionConsistency (enum) and GlobalTableWitnessUpdates (@length max 1); the modeled errors are ValidationException (common), LimitExceededException, ResourceInUseException, ResourceNotFoundException — `NotImplemented` is not a modeled outcome.
Emulator: the request is refused with `NotImplemented: operation UpdateTable with GlobalTableWitnessUpdates is scaffolded but not implemented in this emulator`, so a validation case is reported as an unimplemented surface.
Fix: UpdateTable's MultiRegionConsistency/witness validation, which must reject the unsupported combination before reaching the unimplemented branch.
Traces: global-table-mrsc-unsupported-configuration-rejected.

Cause 9 of 9: RestoreTableFromBackup with SSESpecificationOverride answers NotImplemented.
AWS: `RestoreTableFromBackup` accepts `SSESpecificationOverride` and restores the table encrypted with the supplied customer key; Smithy: RestoreTableFromBackup input member SSESpecificationOverride -> SSESpecification, with errors BackupInUse/BackupNotFound/LimitExceeded/TableAlreadyExists/TableInUse — `NotImplemented` is not among them.
Emulator: `NotImplemented: operation RestoreTableFromBackup with SSESpecificationOverride is scaffolded but not implemented in this emulator`; the restore cannot be performed with an SSE override at all.
Fix: the SSESpecificationOverride branch of the RestoreTableFromBackup handler.
Traces: restore-table-from-backup-sse-override-uses-customer-key.

## Evidence
Cause 1: AssertionError: DISABLE was never refused while ENABLING in 5 attempts: ["DISABLE accepted after the status read 'ENABLING'; settled 'DISABLED'", x5]
Cause 2: ExecuteTransaction naming a missing table gave 'ResourceNotFoundException': 'Requested resource not found', expected ('TransactionCanceledException', 'Transaction cancelled, please refer cancellation reasons for specific reasons [ResourceNotFound]')
Cause 3: searching a vector index that stayed CREATING answered ('ResourceNotFoundException', 'Requested resource not found'); expected ValidationException 'The table does not have the specified index: <IndexName>'
Cause 4: expected the call to be refused, but it succeeded: TableDescription with TableStatus 'DELETING' and Replicas [{RegionName: us-east-1, ReplicaStatus: ACTIVE}]
Cause 5: real AWS rejects this call, but it SUCCEEDED and returned {'Items': [...], 'Count': 1, 'ScannedCount': 1} at HTTP 200
Cause 6: expected the call to be refused, but it succeeded: {'Items': [{'pk': ..., 'a': ..., 'b': ...}], 'Count': 1} at HTTP 200
Cause 7: real AWS rejects this call, but it SUCCEEDED and returned TableDescription with TableStatus 'UPDATING' and TableClassSummary TableClass 'STANDARD_INFREQUENT_ACCESS'
Cause 8: AssertionError: expected ValidationException for a 1-replica STRONG request, got NotImplemented — botocore ClientError (NotImplemented) when calling UpdateTable: operation UpdateTable with GlobalTableWitnessUpdates is scaffolded but not implemented in this emulator
Cause 9: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RestoreTableFromBackup operation: operation RestoreTableFromBackup with SSESpecificationOverride is scaffolded but not implemented in this emulator

## Steps to Reproduce
Cause 1: UpdateContributorInsights ENABLE, then immediately DISABLE while DescribeContributorInsights reads ENABLING.
Cause 2: ExecuteTransaction with a statement naming a nonexistent table.
Cause 3: UpdateTable adding a vector index, then SearchVectors while the index is CREATING.
Cause 4: CreateTable, UpdateTable adding a replica, then DeleteTable in the source region within 24h.
Cause 5: Scan with a FilterExpression whose operator count exceeds the limit (with an IN list counted as one).
Cause 6: Scan with ConditionalOperator=AND and a single ScanFilter condition.
Cause 7: UpdateTable changing TableClass repeatedly past the per-table daily limit.
Cause 8: UpdateTable with MultiRegionConsistency=STRONG on a single-replica table.
Cause 9: RestoreTableFromBackup with SSESpecificationOverride naming a customer KMS key.
