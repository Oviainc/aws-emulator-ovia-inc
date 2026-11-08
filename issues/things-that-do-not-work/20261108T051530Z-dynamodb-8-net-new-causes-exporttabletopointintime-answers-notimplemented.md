Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.export-client-token-conflict dynamodb.export-conflict-duplicate-rejected dynamodb.export-invalid-export-time-rejected dynamodb.export-invalid-time dynamodb.export-to-point-in-time-invalid-time-rejected dynamodb.ddb-frc-specific-attributes-without-a-projection-rejected dynamodb.select-all-projected-attributes-requires-an-index-rejected dynamodb.ddb-ss-all-projected-attributes-on-a-base-table-scan-rejected dynamodb.legacy-queryfilter-scanfilter-keyconditions-argument-rules dynamodb.update-table-delete-of-an-absent-gsi-rejected dynamodb.update-table-gsi-attribute-definitions-track-live-keys dynamodb.ddb-wf-warm-throughput-defaults-applied
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.export-client-token-conflict|run#13|AssertionError|assert code == ERR_EXPORT_CONFLICT, f'expected {ERR_EXPORT_CONFLICT}, got {code}'
Issue-Key: dynamodb.export-conflict-duplicate-rejected|run#19|AssertionError|assert code == "ExportConflictException", f"expected ExportConflictException, got {code}"
Issue-Key: dynamodb.export-invalid-export-time-rejected|run#21|AssertionError|assert message == "Export Time is invalid", f"expected message 'Export Time is invalid', got {message!r}"
Issue-Key: dynamodb.export-invalid-time|run#18|AssertionError|assert message == "Export Time is invalid", (
Issue-Key: dynamodb.export-to-point-in-time-invalid-time-rejected|run#18|AssertionError|assert message == "Export Time is invalid", f"expected message 'Export Time is invalid', got {message!r}"
Issue-Key: dynamodb.ddb-frc-specific-attributes-without-a-projection-rejected|run#10|AssertionError|assert message == (
Issue-Key: dynamodb.select-all-projected-attributes-requires-an-index-rejected|run#16|AssertionError|assert message == "1 validation error detected: Must specify the AttributesToGet or ProjectionExpression when choosing to get SPECIFIC_ATTRIBUTES", (
Issue-Key: dynamodb.ddb-ss-all-projected-attributes-on-a-base-table-scan-rejected|run#5|AssertionError|assert message == (
Issue-Key: dynamodb.legacy-queryfilter-scanfilter-keyconditions-argument-rules|run#7|AssertionError|assert message == want, f'KeyConditions/{op}: expected AWS exact message, got {message!r}'
Issue-Key: dynamodb.update-table-delete-of-an-absent-gsi-rejected|run#9|AssertionError|assert (code, msg) == want, f"deleting the absent GSI {_ABSENT!r} gave {code!r}: {msg!r}"
Issue-Key: dynamodb.update-table-gsi-attribute-definitions-track-live-keys|_error#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: dynamodb.ddb-wf-warm-throughput-defaults-applied|run#10|AssertionError|assert gsi is not None, (
Issue-Commit: 3778ba40189b37afffee3a954bf30487a96c0bc7

# dynamodb: 8 net-new causes — ExportTableToPointInTime answers NotImplemented for a ClientToken conflict and misworded InvalidExportTime, Select/legacy refusal messages diverge, and UpdateTable mis-handles absent-GSI delete, no-op throughput and GSI activation

> _Note: this issue was identified against the code base at commit `3778ba40189b37afffee3a954bf30487a96c0bc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportTableToPointInTime

## Description
Eight net-new root causes across DynamoDB's export surface, its Select/legacy-parameter validation and UpdateTable.

Cause 1 of 8: a ClientToken replayed with a CHANGED parameter answers NotImplemented.
AWS: Smithy: ExportTableToPointInTime{TableArn (required), S3Bucket (required), ClientToken (@idempotencyToken), ExportFormat, ExportTime} -> errors ExportConflictException (client, HTTP 400), InvalidExportTimeException, LimitExceededException, PointInTimeRecoveryUnavailableException, TableNotFoundException, InternalServerError. Replaying the token with IDENTICAL parameters returns the original export; replaying it with a different ExportFormat is refused ExportConflictException, HTTP 400, fixed text "Export conflict: Duplicate request detected with conflicting parameters", and the original export remains the only one tracked.
Emulator: answers error code "NotImplemented" for the conflicting replay — not a modelled error of this operation at all, so the idempotency-conflict path is simply absent.
Why: a client retrying an export with a token cannot distinguish a safe replay from a conflicting one; NotImplemented is unretryable and uninterpretable, and botocore surfaces it as an unknown error class.
Fix: ExportTableToPointInTime's handler — the ClientToken idempotency/conflict branch and its ExportConflictException serialization.
Traces: dynamodb.export-client-token-conflict, dynamodb.export-conflict-duplicate-rejected

Cause 2 of 8: the InvalidExportTimeException message.
AWS: InvalidExportTimeException, HTTP 400, fixed text with no variable part: "Export Time is invalid". It is raised for an ExportTime before the table existed, for a future ExportTime, and for an ExportTime outside the recovery window alike.
Emulator: returns "The specified ExportTime is outside of the point in time recovery window" — an emulator-authored sentence that also narrows the reason to one of the three cases.
Why: the message is the requirement; a client matching AWS's text, or distinguishing export-time classes, never matches.
Fix: ExportTableToPointInTime's ExportTime validation and its InvalidExportTimeException message.
Traces: dynamodb.export-invalid-export-time-rejected, dynamodb.export-invalid-time, dynamodb.export-to-point-in-time-invalid-time-rejected

Cause 3 of 8: Select=SPECIFIC_ATTRIBUTES with no projection.
AWS: ValidationException, HTTP 400, fixed text "1 validation error detected: Must specify the AttributesToGet or ProjectionExpression when choosing to get SPECIFIC_ATTRIBUTES" — AWS names BOTH acceptable members.
Emulator: 400 ValidationException with "1 validation error detected: Must specify the ProjectionExpression when choosing to get SPECIFIC_ATTRIBUTES" — drops "the AttributesToGet or".
Why: the message is the only place AWS tells the caller that legacy AttributesToGet also satisfies the requirement; the emulator's text sends them to the wrong fix.
Fix: the Scan/Query Select validation shared by both operations.
Traces: dynamodb.ddb-frc-specific-attributes-without-a-projection-rejected, dynamodb.select-all-projected-attributes-requires-an-index-rejected

Cause 4 of 8: Select=ALL_PROJECTED_ATTRIBUTES on a base-table Scan.
AWS: ValidationException, HTTP 400, fixed text "1 validation error detected: ALL_PROJECTED_ATTRIBUTES can be used only when Querying using an IndexName" — AWS says "Querying" even when the refused call is a Scan.
Emulator: 400 ValidationException with "...can be used only when Scanning using an IndexName" — substitutes the calling operation's own verb.
Why: byte-exact message divergence on a documented refusal.
Fix: the Scan Select validation's message (it must not interpolate the operation verb).
Traces: dynamodb.ddb-ss-all-projected-attributes-on-a-base-table-scan-rejected

Cause 5 of 8: legacy KeyConditions/QueryFilter/ScanFilter operator arity.
AWS: ValidationException, HTTP 400, template "One or more parameter values were invalid: Invalid number of argument(s) for the <ComparisonOperator> ComparisonOperator" — fixed text around the operator name from the request. It applies to KeyConditions, QueryFilter and ScanFilter alike, for every operator whose AttributeValueList length is wrong (EQ needs 1, BETWEEN needs 2, NULL needs 0).
Emulator: for NULL in KeyConditions it answers "Unsupported operator on KeyConditions: NULL" — it treats a wrong-arity NULL as an unsupported operator rather than an arity violation, with an emulator-authored message.
Why: the arity rule and the supported-operator rule are different refusals; a client reading AWS's arity text to correct its AttributeValueList is told instead that the operator does not exist.
Fix: the legacy-parameter validation for KeyConditions/QueryFilter/ScanFilter (operator arity must be checked, and reported, independently of operator support).
Traces: dynamodb.legacy-queryfilter-scanfilter-keyconditions-argument-rules

Cause 6 of 8: UpdateTable deleting a GSI the table does not have.
AWS: Smithy: UpdateTable.GlobalSecondaryIndexUpdates -> errors include ResourceNotFoundException (client, HTTP 400). Deleting an absent index is ResourceNotFoundException with the template "Requested resource not found: Index <IndexName> for table <TableName>" — fixed text around the index and table names from the request — and the existing GSI is left in place.
Emulator: answers ValidationException with "Attempting to delete an index which does not exist: <IndexName>" — the wrong error CODE (ValidationException instead of ResourceNotFoundException) as well as an emulator-authored message.
Why: a client branching on ResourceNotFoundException to treat an already-deleted index as idempotent instead sees a ValidationException, which SDK retry/idempotency logic classifies differently.
Fix: UpdateTable's GlobalSecondaryIndexUpdates Delete branch — error classification and message.
Traces: dynamodb.update-table-delete-of-an-absent-gsi-rejected

Cause 7 of 8: a no-op ProvisionedThroughput UpdateTable is ACCEPTED.
AWS: ValidationException, HTTP 400, when the requested ReadCapacityUnits/WriteCapacityUnits equal the current values: "The provisioned throughput for the table will not change. The requested value equals the current value. Current ReadCapacityUnits provisioned for the table: <current>. Requested ReadCapacityUnits: <requested>. Current WriteCapacityUnits provisioned for the table: <current>. Requested WriteCapacityUnits: <requested>. Refer to the Amazon DynamoDB Developer Guide for current limits and how to request higher limits." — the four numbers are values from the request and the table's state; the rest is fixed text.
Emulator: returns 200 with a TableDescription in TableStatus UPDATING, so the no-op is accepted.
Why: a client that relies on the refusal to detect a redundant scaling call (and to avoid burning one of the day's decreases) instead sees a successful update; AWS never transitions the table for a no-op.
Fix: UpdateTable's provisioned-throughput validation.
Traces: dynamodb.update-table-gsi-attribute-definitions-track-live-keys

Cause 8 of 8: a GSI created by UpdateTable on an already-ACTIVE on-demand table does not reach ACTIVE.
AWS: a GlobalSecondaryIndexUpdates Create on an ACTIVE PAY_PER_REQUEST table backfills and then reports IndexStatus ACTIVE, with the documented WarmThroughput default {ReadUnitsPerSecond: 12000, WriteUnitsPerSecond: 4000, Status: ACTIVE}.
Emulator: the check's polling budget is consumed before the index is ever observed ACTIVE — the preceding CreateTable/waiter/UpdateTable sequence ate the whole 180 s check budget, leaving 1 s to poll, and DescribeTable had not produced an ACTIVE WorkflowIndex. The index's WarmThroughput defaults are therefore never observable.
Why: a client that adds a GSI and waits for it cannot proceed; nothing reports the index as usable.
Fix: UpdateTable's GlobalSecondaryIndexUpdates Create branch and the index-state component that drives CREATING -> ACTIVE, including the WarmThroughput defaults DescribeTable serves for it.
Traces: dynamodb.ddb-wf-warm-throughput-defaults-applied

No open issue covers these eight causes; the open dynamodb bundles listed in the suite report own other checks.

## Evidence
Replay output, expected (real AWS) vs actual (emulator):

1. export-client-token-conflict / export-conflict-duplicate-rejected — AssertionError: 'expected ExportConflictException, got NotImplemented'. The conflicting replay (same ClientToken, ExportFormat changed to ION) should be 400 ExportConflictException with message 'Export conflict: Duplicate request detected with conflicting parameters'.

2. export-invalid-export-time-rejected / export-invalid-time / export-to-point-in-time-invalid-time-rejected — expected InvalidExportTimeException message 'Export Time is invalid'; actual 'The specified ExportTime is outside of the point in time recovery window'.

3. ddb-frc-specific-attributes-without-a-projection-rejected / select-all-projected-attributes-requires-an-index-rejected — expected '1 validation error detected: Must specify the AttributesToGet or ProjectionExpression when choosing to get SPECIFIC_ATTRIBUTES'; actual '1 validation error detected: Must specify the ProjectionExpression when choosing to get SPECIFIC_ATTRIBUTES'.

4. ddb-ss-all-projected-attributes-on-a-base-table-scan-rejected — expected '1 validation error detected: ALL_PROJECTED_ATTRIBUTES can be used only when Querying using an IndexName'; actual '1 validation error detected: ALL_PROJECTED_ATTRIBUTES can be used only when Scanning using an IndexName'.

5. legacy-queryfilter-scanfilter-keyconditions-argument-rules — 'KeyConditions/NULL: expected AWS exact message, got "Unsupported operator on KeyConditions: NULL"'; expected 'One or more parameter values were invalid: Invalid number of argument(s) for the NULL ComparisonOperator'.

6. update-table-delete-of-an-absent-gsi-rejected — expected ('ResourceNotFoundException', 'Requested resource not found: Index by-nothing for table <table>'); actual ('ValidationException', 'Attempting to delete an index which does not exist: by-nothing').

7. update-table-gsi-attribute-definitions-track-live-keys — AssertionError: 'expected the call to be rejected, but it succeeded'; UpdateTable re-requesting the already-current 7/7 ReadCapacityUnits/WriteCapacityUnits returned HTTP 200 with TableStatus 'UPDATING' instead of 400 ValidationException.

8. ddb-wf-warm-throughput-defaults-applied — AssertionError: 'WorkflowIndex did not reach ACTIVE within 1s'; the index added by UpdateTable to an already-ACTIVE PAY_PER_REQUEST table was never observed ACTIVE, so its WarmThroughput {12000, 4000, ACTIVE} default could not be read.

## Steps to Reproduce
Replay the behavioral suite and re-run the listed dynamodb checks, or by hand against the emulator:
1. Create a table, enable PITR, create a bucket, call ExportTableToPointInTime with a ClientToken, then call it again with the same token and ExportFormat='ION' -> expect 400 ExportConflictException; observe NotImplemented.
2. ExportTableToPointInTime with ExportTime in the future (and with one before the table existed) -> compare the InvalidExportTimeException message to 'Export Time is invalid'.
3. Scan with Select='SPECIFIC_ATTRIBUTES' and no ProjectionExpression/AttributesToGet -> compare the message.
4. Scan with Select='ALL_PROJECTED_ATTRIBUTES' and no IndexName -> compare the message.
5. Query with KeyConditions containing a NULL ComparisonOperator whose AttributeValueList has 1 element -> compare the message.
6. Create a table with one GSI, then UpdateTable with GlobalSecondaryIndexUpdates=[{Delete:{IndexName:'by-nothing'}}] -> expect ResourceNotFoundException.
7. Create a PROVISIONED table, UpdateTable to 7/7, wait, then UpdateTable to 7/7 again -> expect 400 ValidationException.
8. Create an ACTIVE PAY_PER_REQUEST table, UpdateTable adding a GSI, then poll DescribeTable -> the index must reach IndexStatus ACTIVE and report WarmThroughput {ReadUnitsPerSecond:12000, WriteUnitsPerSecond:4000, Status:'ACTIVE'}.
