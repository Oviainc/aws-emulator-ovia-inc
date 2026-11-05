Issue-Source: trace-sweep-divergence
Issue-Trace: create-table-rejects-malformed-requests
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: create-table-rejects-malformed-requests dynamodb-csvtraj-single-column-csv-hash-reused-as-range dynamodb-ltr-describe-table-by-arn-forms ddb-sfa-traj-attribute-type-route-and-unknown-type ddb-sfa-traj-mixed-legacy-and-expression-rejected dynamodb-frctraj-filter-expression-literal-and-operator-mistakes dynamodb-frctraj-number-operand-malformed dynamodb-ssbs-traj-projection-request-shape-mistakes scan-filter-begins-with-non-string-operand-rejected dynamodb-csvtraj-gsi-added-after-create-via-update-table dynamodb-ddbstrip-traj-partiql-bulk-update-mistakes dynamodb-execute-statement-non-numeric-n-parameter-rejected dynamodb-execute-statement-out-of-range-n-parameter-rejected dynamodb-partiql-index-qualifier dynamodb-partiql-relational-operators-reject-complex-type-operands dynamodb-partiql-select-gsi-include-projection-filter dynamodb-partiql-select-nested-document-paths dynamodb-partiql-size-and-attribute-type-per-type-sets dynamodb-ddbstrip-traj-request-shape-mistakes dynamodb-legacy-expected-and-attribute-updates-malformed-combinations dynamodb-list-backups-time-range-and-type-filter dynamodb-number-precision-38-significant-digits dynamodb-set-empty-and-duplicate-members-rejected dynamodb-refarch-rekreup-stale-row-read-addressing dynamodb-ddbkds-traj-reference-active-inventory
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|CreateTable|4xx->2xx|$.__type
Issue-Key: dynamodb|CreateTable|4xx->2xx|$.message
Issue-Key: dynamodb|CreateTable|4xx->2xx|status
Issue-Key: dynamodb|DescribeTable|4xx->2xx|$.__type
Issue-Key: dynamodb|DescribeTable|4xx->2xx|$.message
Issue-Key: dynamodb|DescribeTable|4xx->2xx|status
Issue-Key: dynamodb|Scan|4xx->2xx|$.__type
Issue-Key: dynamodb|Scan|4xx->2xx|$.message
Issue-Key: dynamodb|Scan|4xx->2xx|status
Issue-Key: dynamodb|UpdateTable|4xx->2xx|$.__type
Issue-Key: dynamodb|UpdateTable|4xx->2xx|$.message
Issue-Key: dynamodb|UpdateTable|4xx->2xx|status
Issue-Key: dynamodb|ExecuteStatement|4xx->2xx|$.__type
Issue-Key: dynamodb|ExecuteStatement|4xx->2xx|$.message
Issue-Key: dynamodb|ExecuteStatement|4xx->2xx|status
Issue-Key: dynamodb|UpdateItem|4xx->2xx|$
Issue-Key: dynamodb|UpdateItem|4xx->2xx|status
Issue-Key: dynamodb|ListBackups|4xx->2xx|$.__type
Issue-Key: dynamodb|ListBackups|4xx->2xx|$.message
Issue-Key: dynamodb|ListBackups|4xx->2xx|status
Issue-Key: dynamodb|PutItem|4xx->2xx|$
Issue-Key: dynamodb|PutItem|4xx->2xx|status
Issue-Key: dynamodb|GetItem|4xx->2xx|$.__type
Issue-Key: dynamodb|GetItem|4xx->2xx|$.message
Issue-Key: dynamodb|GetItem|4xx->2xx|status
Issue-Key: dynamodb|UpdateKinesisStreamingDestination|4xx->2xx|$.__type
Issue-Key: dynamodb|UpdateKinesisStreamingDestination|4xx->2xx|$.message
Issue-Key: dynamodb|UpdateKinesisStreamingDestination|4xx->2xx|status
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# Invalid DynamoDB requests are accepted and answered 200 without the modeled ValidationException

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Ten net-new root causes in which a request AWS refuses with a 400 ValidationException is accepted by the emulator and answered 200. Each cause is an independent missing validation rule and must be closed separately. Related open issues exist for neighbouring surfaces (20261103T042714Z-dynamodb-createtable-updatetable-accept-schemas-aws-refuses-and.md, 20261103T042714Z-dynamodb-data-plane-limits-are-unenforced-or-wrongly.md, 20261103T065250Z-dynamodb-partiql-and-legacy-parameter-surface-10-root.md); none of these ten steps is owned by them per the replay report.

Cause 1 of 10 — CreateTable: a global secondary index whose Projection.ProjectionType is INCLUDE but which carries no NonKeyAttributes.
AWS: Smithy CreateTableInput.GlobalSecondaryIndexes -> GlobalSecondaryIndexList, whose Projection member documents ProjectionType INCLUDE as 'Only the specified table attributes are projected into the index. The list of projected attributes is in NonKeyAttributes'. The request is refused 400 with __type 'com.amazon.coral.validate#ValidationException' and message 'One or more parameter values were invalid: ProjectionType is INCLUDE, but NonKeyAttributes is not specified' — the whole message is fixed text with no variable part.
Emulator: 200 with a full TableDescription; the table and the malformed index are created.
Why: a client that relies on CreateTable rejecting an under-specified projection builds a table whose index projects nothing it asked for, and the error-handling branch that would correct the request is never exercised.
Fix: CreateTable request validation (the GSI/LSI projection rule), before any state is written.
Traces: create-table-rejects-malformed-requests#task#1 (cluster also covers #task#3, #task#6, #task#8 and dynamodb-csvtraj-single-column-csv-hash-reused-as-range#task#1).

Cause 2 of 10 — DescribeTable answers 200 for a table that does not exist.
AWS: DescribeTable on an absent table is refused 400 with __type 'com.amazonaws.dynamodb.v20120810#ResourceNotFoundException' and message 'Requested resource not found: Table: <tableName> not found', where the fixed text is 'Requested resource not found: Table: ' ... ' not found' and <tableName> is the name from the request.
Emulator: 200 with a Table description. The emulator has state for a table AWS never created, so the not-found path is unreachable.
Why: a client cannot distinguish 'created' from 'rejected'; a wait-for-table loop succeeds against a table that does not exist on AWS.
Fix: DescribeTable handler's resource lookup plus the CreateTable validation in cause 1 that put the row there.
Traces: create-table-rejects-malformed-requests#task#2 (cluster also covers #task#4, #task#7, #task#9, dynamodb-csvtraj-single-column-csv-hash-reused-as-range#task#3, dynamodb-ltr-describe-table-by-arn-forms#task#5).

Cause 3 of 10 — Scan: FilterExpression using attribute_type with a token that is not a DynamoDB type code.
AWS: Smithy ScanInput.FilterExpression -> ConditionExpression. The second operand of attribute_type must be one of the DynamoDB type codes, so a spelled-out type name is refused 400 ValidationException ('com.amazon.coral.validate#ValidationException') with message 'Invalid FilterExpression: Invalid attribute type name found; type: <token>, valid types: {S,SS,N,NS,B,BS,BOOL,NULL,L,M}' — fixed text around the offending token taken from the request.
Emulator: 200 and a result set is returned as if the filter were valid.
Why: a typo'd filter silently returns data instead of being refused, so a client reads rows it never asked for and never sees the diagnostic.
Fix: Scan's FilterExpression validation (attribute_type operand domain) in the expression validator.
Traces: ddb-sfa-traj-attribute-type-route-and-unknown-type#task#6 (cluster covers 11 steps, incl. scan-filter-begins-with-non-string-operand-rejected#task#3 and dynamodb-frctraj-number-operand-malformed#task#4).

Cause 4 of 10 — UpdateTable: creating a GSI without the AttributeDefinitions its key schema needs.
AWS: Smithy UpdateTableInput carries both AttributeDefinitions and GlobalSecondaryIndexUpdates; every attribute named in an index key schema must be defined. AWS refuses 400 ValidationException with message 'One or more parameter values were invalid: AttributeDefinitions is not specified for index: <indexName>' — fixed text with the index name from the request.
Emulator: 200 and the index creation is started.
Why: the client believes an index exists whose key attributes are undeclared; subsequent index reads behave differently from AWS.
Fix: UpdateTable request validation (GlobalSecondaryIndexUpdates vs AttributeDefinitions cross-check).
Traces: dynamodb-csvtraj-gsi-added-after-create-via-update-table#task#5.

Cause 5 of 10 — ExecuteStatement accepts a malformed PartiQL statement.
AWS: Smithy ExecuteStatementInput.Statement is required, length 1..8192, target PartiQLStatement. A statement with trailing garbage after the expression is refused 400 ValidationException with message "Statement wasn't well formed, can't be processed: Unexpected token after expression" (fixed text).
Emulator: 200 with a normal result envelope.
Why: a malformed statement appears to have run; a client never learns its statement was wrong and may conclude a write took effect.
Fix: the PartiQL parser/validation in the ExecuteStatement handler.
Traces: dynamodb-ddbstrip-traj-partiql-bulk-update-mistakes#task#6 (cluster covers 12 steps, incl. dynamodb-partiql-relational-operators-reject-complex-type-operands#task#4 and dynamodb-partiql-index-qualifier#task#8).

Cause 6 of 10 — UpdateItem accepts an empty ExpressionAttributeValues map.
AWS: Smithy UpdateItem input ExpressionAttributeValues -> ExpressionAttributeValueMap. An empty map is refused 400 ValidationException with message '1 validation error detected: ExpressionAttributeValues must not be empty' (fixed text).
Emulator: 200 with an empty body.
Why: a request whose placeholders can never bind is treated as a successful update, so the client records a write that did not happen as AWS would have refused it.
Fix: UpdateItem request validation (empty expression-attribute maps).
Traces: dynamodb-ddbstrip-traj-request-shape-mistakes#task#6 (cluster also covers #task#7 and dynamodb-legacy-expected-and-attribute-updates-malformed-combinations#task#10).

Cause 7 of 10 — ListBackups accepts an inverted time range.
AWS: Smithy ListBackupsInput.TimeRangeLowerBound / TimeRangeUpperBound are timestamps; a lower bound after the upper bound is refused 400 ValidationException with message 'One or more parameter values were invalid: Time range lower bound must be less than or equal to upper bound' (fixed text).
Emulator: 200 with a backup listing.
Why: an inverted window silently returns a list, so a client's range filter is not honoured and the refusal it expects never arrives.
Fix: ListBackups request validation.
Traces: dynamodb-list-backups-time-range-and-type-filter#task#6.

Cause 8 of 10 — PutItem stores a Number with more than 38 significant digits.
AWS: the N attribute domain is capped at 38 significant digits; the write is refused 400 ValidationException with message '1 validation error detected: Attempting to store more than 38 significant digits in a Number' (fixed text).
Emulator: 200 with an empty body and the over-precision number is stored.
Why: the client stores a value AWS would reject, and later reads differ from AWS in precision rather than failing at write time.
Fix: PutItem attribute-value validation (the N domain check) in the item validator.
Traces: dynamodb-number-precision-38-significant-digits#task#13 (cluster also covers dynamodb-set-empty-and-duplicate-members-rejected#task#14).

Cause 9 of 10 — GetItem accepts a TableName ARN belonging to a different region.
AWS: Smithy GetItemInput.TableName targets TableArn (length 1..1024) and its doc allows an ARN in place of a name. An ARN whose region is not the endpoint's region is refused 400 ValidationException with message "1 validation error detected: Value '<arn>' at 'tableName' failed to satisfy constraint: Invalid AWS region" — the ARN is the request value, the rest is fixed.
Emulator: 200, serving the item from the same-named local table.
Why: cross-region addressing mistakes are invisible; the client reads another region's data from this endpoint and the guard rail AWS provides is gone.
Fix: the TableName/ARN parser shared by the data-plane handlers (region component check).
Traces: dynamodb-refarch-rekreup-stale-row-read-addressing#task#6.

Cause 10 of 10 — UpdateKinesisStreamingDestination accepts a no-op precision change.
AWS: re-requesting the precision a destination already has is refused 400 ValidationException with message 'Invalid Request: Precision is already set to the desired value of <precision> for tableId: <tableId>, kdsArn: <streamArn>' — fixed text with the precision from the request and the table id and stream ARN from the emulator's state.
Emulator: 200, as if the update changed something.
Why: an idempotency/duplicate-configuration check the client depends on never fires.
Fix: UpdateKinesisStreamingDestination handler's pre-condition check against the stored destination.
Traces: dynamodb-ddbkds-traj-reference-active-inventory#task#16.

## Evidence
Replay of simparity-wire-aws-v1 (2134/2631 steps passed, 95 divergence clusters). Per cause, expected -> actual:
1. CreateTable create-table-rejects-malformed-requests#task#1: expected 400 + __type com.amazon.coral.validate#ValidationException + message 'One or more parameter values were invalid: ProjectionType is INCLUDE, but NonKeyAttributes is not specified'; actual 200, no __type, no message (5 steps).
2. DescribeTable create-table-rejects-malformed-requests#task#2: expected 400 + com.amazonaws.dynamodb.v20120810#ResourceNotFoundException + 'Requested resource not found: Table: <name> not found'; actual 200 (6 steps).
3. Scan ddb-sfa-traj-attribute-type-route-and-unknown-type#task#6: expected 400 + ValidationException + 'Invalid FilterExpression: Invalid attribute type name found; type: STRING, valid types: {S,SS,N,NS,B,BS,BOOL,NULL,L,M}'; actual 200 (11 steps).
4. UpdateTable dynamodb-csvtraj-gsi-added-after-create-via-update-table#task#5: expected 400 + ValidationException + 'One or more parameter values were invalid: AttributeDefinitions is not specified for index: <index>'; actual 200.
5. ExecuteStatement dynamodb-ddbstrip-traj-partiql-bulk-update-mistakes#task#6: expected 400 + ValidationException + "Statement wasn't well formed, can't be processed: Unexpected token after expression"; actual 200 (12 steps).
6. UpdateItem dynamodb-ddbstrip-traj-request-shape-mistakes#task#6: expected 400 + ValidationException + '1 validation error detected: ExpressionAttributeValues must not be empty'; actual 200 with empty body (3 steps).
7. ListBackups dynamodb-list-backups-time-range-and-type-filter#task#6: expected 400 + ValidationException + 'One or more parameter values were invalid: Time range lower bound must be less than or equal to upper bound'; actual 200.
8. PutItem dynamodb-number-precision-38-significant-digits#task#13: expected 400 + ValidationException + '1 validation error detected: Attempting to store more than 38 significant digits in a Number'; actual 200 empty body (2 steps).
9. GetItem dynamodb-refarch-rekreup-stale-row-read-addressing#task#6: expected 400 + ValidationException + "1 validation error detected: Value '<arn>' at 'tableName' failed to satisfy constraint: Invalid AWS region"; actual 200.
10. UpdateKinesisStreamingDestination dynamodb-ddbkds-traj-reference-active-inventory#task#16: expected 400 + ValidationException + 'Invalid Request: Precision is already set to the desired value of <precision> for tableId: <id>, kdsArn: <arn>'; actual 200.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Representative: replay fixture create-table-rejects-malformed-requests, step task#1 — CreateTable with a GSI whose Projection is {ProjectionType: INCLUDE} and no NonKeyAttributes; the emulator answers 200 instead of 400 ValidationException.
3. For each remaining cause replay the fixture#step listed in trace_ids and compare status, __type and message against the recorded response: an invalid CreateTable/UpdateTable schema, an attribute_type filter with a spelled-out type name, a malformed PartiQL statement, an empty ExpressionAttributeValues map, an inverted ListBackups time range, a 39-significant-digit Number, a cross-region table ARN on GetItem, and a no-op Kinesis precision update.
