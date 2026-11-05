Issue-Source: trace-sweep-divergence
Issue-Trace: create-table-rejects-malformed-requests
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: create-table-rejects-malformed-requests dynamodb-create-and-update-table-throughput-mode-invalid-enum dynamodb-csvtraj-create-table-enum-capacity-and-projection-mistakes dynamodb-csvtraj-every-csv-column-declared-as-attribute dynamodb-csvtraj-gsi-on-undeclared-csv-column dynamodb-csvtraj-table-name-derived-from-object-key dynamodb-multiple-simultaneous-constraint-violations-rejected dynamodb-vvd-traj-create-table-billing-mode-mistakes dynamodb-vvd-traj-create-table-key-schema-mistakes dynamodb-ddbkds-traj-describe-request-validation dynamodb-ddbkds-traj-table-arn-instead-of-name dynamodb-ddbkds-traj-enable-request-validation dynamodb-ddbkds-traj-reference-active-inventory dynamodb-ddbkds-traj-list-tables-limit-bounds dynamodb-ltr-list-tables-page-bounds-rejected dynamodb-batch-request-items-key-not-a-table-name-rejected dynamodb-malformed-n-in-key-top-level-not-cancellation dynamodb-backup-arn-shape-rejected dynamodb-ltr-detail-ops-name-where-arn-required dynamodb-deleteitem-expression-validation-rejected dynamodb-empty-expression-string-rejected dynamodb-expression-attribute-value-empty-rejected dynamodb-rbp-traj-stale-arn-after-table-deletion
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|CreateTable|4xx->4xx|$.message
Issue-Key: dynamodb|DescribeKinesisStreamingDestination|4xx->4xx|$.message
Issue-Key: dynamodb|EnableKinesisStreamingDestination|4xx->4xx|$.message
Issue-Key: dynamodb|ListTables|4xx->4xx|$.message
Issue-Key: dynamodb|BatchGetItem|4xx->4xx|$.message
Issue-Key: dynamodb|DescribeBackup|4xx->4xx|$.message
Issue-Key: dynamodb|DeleteBackup|4xx->4xx|$.message
Issue-Key: dynamodb|ListTagsOfResource|4xx->4xx|$.message
Issue-Key: dynamodb|DeleteItem|4xx->4xx|$.message
Issue-Key: dynamodb|DeleteResourcePolicy|4xx->4xx|$.message
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# DynamoDB refusal text is emulator-authored for required members, ARNs and multi-error ordering

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Ten net-new root causes where the emulator refuses with the right status and error class but a message that is not AWS's. The error message is part of the contract, so each wording is its own fix. The open issue 20261030T060227Z-dynamodb-validationexception-messages-diverge-from-the-wording-aws.md covers a different set of messages (size/nesting limits, enum ordering); none of the ten steps below is owned by it per the replay report.

Cause 1 of 10 — CreateTable with TableName absent.
AWS: Smithy CreateTableInput.TableName is required (target TableArn, length 1..1024). AWS refuses 400 ValidationException with message "The parameter 'TableName' is required but was not present in the request" — all fixed text except the member name, which is the missing member.
Emulator: "1 validation error detected: Value null at 'TableName' failed to satisfy constraint: Member must not be null" — the generic constraint form, which AWS does not use for a wholly absent required member.
Why: clients and test suites that match on AWS's required-parameter wording do not recognise the refusal.
Fix: the shared request-validation message builder (missing-required-member branch).
Traces: create-table-rejects-malformed-requests#task#5 (cluster covers 17 steps, incl. dynamodb-vvd-traj-create-table-key-schema-mistakes#task#1).

Cause 2 of 10 — DescribeKinesisStreamingDestination with TableName absent: the same AWS required-parameter wording, the same emulator substitution.
AWS: "The parameter 'TableName' is required but was not present in the request".
Emulator: "1 validation error detected: Value null at 'TableName' failed to satisfy constraint: Member must not be null".
Why / Fix: as cause 1, reached through the Kinesis-destination handlers.
Traces: dynamodb-ddbkds-traj-describe-request-validation#task#4 (also dynamodb-ddbkds-traj-table-arn-instead-of-name#task#6).

Cause 3 of 10 — EnableKinesisStreamingDestination names the member with the wrong casing.
AWS: when the constraint form IS used, AWS names the member by its lowerCamel wire name: "1 validation error detected: Value null at 'streamArn' failed to satisfy constraint: Member must not be null".
Emulator: the same sentence with 'StreamArn'.
Why: the member path in the message is what a client surfaces to a user; the wrong casing names a member that does not exist in the wire form.
Fix: the validation message builder's member-path rendering (use the wire member name).
Traces: dynamodb-ddbkds-traj-enable-request-validation#task#5 (also dynamodb-ddbkds-traj-reference-active-inventory#task#15).

Cause 4 of 10 — ListTables reports two validation errors in the wrong order.
AWS: when several members fail, the clauses appear in the order AWS validates the input members: '2 validation errors detected: Value <limit> at \'limit\' failed to satisfy constraint: Member must have value greater than or equal to 1; Value <name> at \'exclusiveStartTableName\' failed to satisfy constraint: Member must have length greater than or equal to 3'.
Emulator: the same two clauses with exclusiveStartTableName first.
Why: the message is a single string; a client matching the whole refusal, or reporting 'the first problem', gets a different answer than on AWS.
Fix: the multi-error aggregation order in the request validator.
Traces: dynamodb-ddbkds-traj-list-tables-limit-bounds#task#7 (also dynamodb-ltr-list-tables-page-bounds-rejected#task#2, #task#3).

Cause 5 of 10 — BatchGetItem with a RequestItems key that is not a legal table name blames the wrong member.
AWS: the violated constraint is on the RequestItems map key, so the message quotes the whole map value at 'requestItems' and lists the map-key constraint set: '1 validation error detected: Value \'<requestItems JSON>\' at \'requestItems\' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 255, Member must have length greater than or equal to 3, Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+]'.
Emulator: blames a 'tableName' member and reports only the min-length clause.
Why: the member path a client logs or asserts on does not exist in the BatchGetItem request shape.
Fix: BatchGetItem request validation — validate the map key as a map key, not as a synthesised tableName member.
Traces: dynamodb-batch-request-items-key-not-a-table-name-rejected#task#5 (also dynamodb-malformed-n-in-key-top-level-not-cancellation#task#7).

Cause 6 of 10 — DescribeBackup with a BackupArn that is not a backup ARN.
AWS: Smithy DescribeBackupInput.BackupArn is required, length 37..1024. A value of the wrong ARN class is refused 400 with the fixed message 'Invalid Backup ARN' — no variable part, and the offending value is NOT echoed.
Emulator: 'Invalid BackupArn: Invalid BackupArn provided as input <arn>' — invented text that also leaks the input.
Why: clients matching AWS's short refusal do not recognise it.
Fix: the BackupArn parser's refusal message in the DescribeBackup handler.
Traces: dynamodb-backup-arn-shape-rejected#task#3 (also #task#4, #task#5).

Cause 7 of 10 — DeleteBackup with a BackupArn that is not a backup ARN: the same AWS message 'Invalid Backup ARN', the same invented emulator text.
Why / Fix: as cause 6, in the DeleteBackup handler.
Traces: dynamodb-backup-arn-shape-rejected#task#6 (also #task#7, #task#8).

Cause 8 of 10 — ListTagsOfResource given a bare table name where an ARN belongs.
AWS: 400 ValidationException with message "One or more parameter values were invalid: ARNs must start with 'arn:': <value>" — fixed text plus the request value.
Emulator: 'Invalid TableArn: Invalid ResourceArn provided as input <value>'.
Why: the refusal does not state the rule (ARNs must start with 'arn:'), so a client cannot tell a malformed ARN from an unknown resource.
Fix: the ResourceArn validation message shared by the tagging/resource-policy handlers.
Traces: dynamodb-ltr-detail-ops-name-where-arn-required#task#3.

Cause 9 of 10 — DeleteItem with a reserved word used bare in a ConditionExpression.
AWS: a reserved word in expression position is a parse failure, reported as '1 validation error detected: Invalid ConditionExpression: Syntax error; token: "<EOF>", near: "<token>"' — fixed text with the token taken from the expression.
Emulator: '1 validation error detected: Invalid ConditionExpression: Attribute name is a reserved keyword; reserved keyword: <token>' — a different diagnostic class for the same input.
Why: the client is told to rename an attribute when AWS reports where the expression stopped parsing; automated remediation keyed on AWS's wording misfires.
Fix: the expression parser's error reporting for ConditionExpression (reserved-word handling must fall through to the syntax-error path).
Traces: dynamodb-deleteitem-expression-validation-rejected#task#4 (also #task#5, #task#6, dynamodb-empty-expression-string-rejected#task#5).

Cause 10 of 10 — DeleteResourcePolicy against a deleted table names the wrong resource kind.
AWS: 400 ResourceNotFoundException with message 'Requested resource not found: Table: <tableName> not found' — the resource is named as a Table and identified by its name.
Emulator: 'Requested resource not found: ResourceArn: <arn> not found'.
Why: the message names a resource kind AWS never reports, so client-side parsing of the missing table name fails.
Fix: the resource-policy handlers' not-found message (resolve the ARN to its table name before formatting).
Traces: dynamodb-rbp-traj-stale-arn-after-table-deletion#task#8. The open issue 20261030T060227Z-dynamodb-resource-policy-refusals-name-the-wrong-resource.md covers the Put/Get variants of this message; this DeleteResourcePolicy step is not owned by it.

## Evidence
Replay of simparity-wire-aws-v1. Status and error class matched in every cause below; only $.message diverged (expected -> actual):
1. CreateTable create-table-rejects-malformed-requests#task#5: "The parameter 'TableName' is required but was not present in the request" -> "1 validation error detected: Value null at 'TableName' failed to satisfy constraint: Member must not be null" (17 steps).
2. DescribeKinesisStreamingDestination dynamodb-ddbkds-traj-describe-request-validation#task#4: same pair (2 steps).
3. EnableKinesisStreamingDestination dynamodb-ddbkds-traj-enable-request-validation#task#5: "...at 'streamArn'..." -> "...at 'StreamArn'..." (2 steps).
4. ListTables dynamodb-ddbkds-traj-list-tables-limit-bounds#task#7: limit clause first -> exclusiveStartTableName clause first (3 steps).
5. BatchGetItem dynamodb-batch-request-items-key-not-a-table-name-rejected#task#5: map-key constraint on 'requestItems' -> min-length constraint on 'tableName' (2 steps).
6. DescribeBackup dynamodb-backup-arn-shape-rejected#task#3: 'Invalid Backup ARN' -> 'Invalid BackupArn: Invalid BackupArn provided as input <arn>' (3 steps).
7. DeleteBackup dynamodb-backup-arn-shape-rejected#task#6: same pair (3 steps).
8. ListTagsOfResource dynamodb-ltr-detail-ops-name-where-arn-required#task#3: "One or more parameter values were invalid: ARNs must start with 'arn:': <value>" -> 'Invalid TableArn: Invalid ResourceArn provided as input <value>'.
9. DeleteItem dynamodb-deleteitem-expression-validation-rejected#task#4: 'Invalid ConditionExpression: Syntax error; token: "<EOF>", near: "<token>"' -> 'Invalid ConditionExpression: Attribute name is a reserved keyword; reserved keyword: <token>' (6 steps).
10. DeleteResourcePolicy dynamodb-rbp-traj-stale-arn-after-table-deletion#task#8: 'Requested resource not found: Table: <name> not found' -> 'Requested resource not found: ResourceArn: <arn> not found'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Representative: replay fixture create-table-rejects-malformed-requests, step task#5 — CreateTable with no TableName; compare $.message with the recorded response.
3. For each remaining cause replay the fixture#step in trace_ids and diff $.message only: the Kinesis-destination required-member and member-casing refusals, the ListTables two-error ordering, the BatchGetItem RequestItems map-key refusal, the DescribeBackup/DeleteBackup 'Invalid Backup ARN' refusals, the ListTagsOfResource ARN-prefix refusal, the DeleteItem ConditionExpression parse refusal, and the DeleteResourcePolicy not-found refusal.
