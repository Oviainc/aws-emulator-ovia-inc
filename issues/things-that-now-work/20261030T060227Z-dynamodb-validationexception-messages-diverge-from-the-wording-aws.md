Issue-Source: trace-sweep-divergence
Issue-Trace: ddbval-expression-over-4kb-rejected
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ddbval-expression-over-4kb-rejected ddbval-unused-attribute-name-placeholder-rejected dynamodb-validation-message-names-the-violated-concept dynamodb-batch-caps-and-unprocessed dynamodb-legacy-attribute-updates-semantics dynamodb-legacy-expected-conditional-semantics dynamodb-table-billing-mode-update incremental-export-invalid-windows-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|GetItem|4xx->4xx|$.message
Issue-Key: dynamodb|UpdateItem|4xx->4xx|$.message
Issue-Key: dynamodb|PutItem|4xx->4xx|$.message
Issue-Key: dynamodb|BatchWriteItem|4xx->4xx|$.message
Issue-Key: dynamodb|TransactGetItems|4xx->4xx|$.message
Issue-Key: dynamodb|TransactWriteItems|4xx->4xx|$.message
Issue-Key: dynamodb|UpdateTable|4xx->4xx|$.message
Issue-Key: dynamodb|ExportTableToPointInTime|4xx->4xx|$.message
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# DynamoDB ValidationException messages diverge from the wording AWS serves

## Resolution

Causes 1, 2, 3, 7 and 8 are fixed: a refusal about a request's expressions — the 4 KB
expression ceiling and the conflict between the two generations of parameters alike — now
carries the validation envelope exactly when it is about a member a write states its intent
through, an element of a multi-item request states its expression refusals on its own,
`UpdateTable`'s no-change refusal is the fixed sentence the service serves, and
`ExportViewType`'s value set is stated in the order the service publishes.
`tests/all/expression_refusal_families.rs` and `tests/all/enum_value_set_order.rs` pin all
of it over the wire.

Causes 4, 5 and 6 — the `BatchWriteItem` / `TransactGetItems` / `TransactWriteItems`
over-cap messages — are NOT fixed and remain divergent. Their recorded messages echo the
rejected member value in AWS's internal object rendering, over members no public contract
declares (`AttributeValue(s=…, n=null, b=null, iNT=null, dECIMAL…)`,
`PutRequest(item=…, workloadProfileName=null)`, while
`smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeValue")` gives exactly
`S N B SS NS BS M L NULL BOOL`), so no rule reachable from the model or the docs renders
that text; the same violation class is recorded in the no-value, wire-path form for
`BatchGetItem`'s key cap, which this service serves (see "WHICH FORM A CONSTRAINT CLAUSE
TAKES" in `src/service.rs` and the `quoted_map` note in `src/state/multi_item.rs`). Those
three keys still fail replay.

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetItem

## Description
The emulator refuses all of these requests with the right status (400) and the right __type, but the message text is not the text AWS serves. DynamoDB uses two distinct message families — the Coral constraint envelope '<n> validation error detected: ...' for constraint violations on modelled members, and a bare sentence for semantic refusals — and the emulator applies the envelope to the wrong family in both directions, renders the offending value differently, and orders enumerated member/value lists differently. 8 causes, all dynamodb.

Cause 1 of 8: an expression-size refusal is wrapped in the constraint envelope AWS does not use for it.
AWS: ProjectionExpression size is a semantic limit checked after parsing, not a Smithy @length constraint, so AWS serves the bare sentence 'Invalid ProjectionExpression: Expression size has exceeded the maximum allowed size;' (fixed text, including the trailing semicolon; the member name 'ProjectionExpression' is the request's expression parameter).
Emulator: '1 validation error detected: Invalid ProjectionExpression: Expression size has exceeded the maximum allowed size;' — the envelope is prepended.
Why: clients and test suites match on the message to classify the refusal; a prefixed message fails an exact or anchored match, and the envelope falsely claims a modelled-constraint violation.
Fix: the expression validation in the GetItem/Scan/Query read path — emit semantic expression refusals without the '1 validation error detected: ' envelope.
Traces: ddbval-expression-over-4kb-rejected#task#7 (cluster of 3, also ddbval-unused-attribute-name-placeholder-rejected#task#4 and dynamodb-validation-message-names-the-violated-concept#task#8).

Cause 2 of 8: UpdateItem's mixed expression/non-expression refusal is missing the envelope AWS does use.
AWS: the recording shows '1 validation error detected: Can not use both expression and non-expression parameters in the same request: Non-expression parameters: {AttributeUpdates} Expression parameters: {UpdateExpression}'. The envelope and the sentence are fixed text; the two brace lists name the parameters present on the request.
Emulator: the same sentence with the '1 validation error detected: ' envelope omitted.
Why: the inverse of cause 1 — the same client-side classification breaks, in the opposite direction.
Fix: the UpdateItem parameter-conflict validation — this refusal is in the constraint family and must carry the envelope.
Traces: dynamodb-batch-caps-and-unprocessed#task#14 (cluster of 2, also dynamodb-legacy-attribute-updates-semantics#task#15).

Cause 3 of 8: PutItem's Expected/ConditionExpression conflict refusal is likewise missing the envelope.
AWS: '1 validation error detected: Can not use both expression and non-expression parameters in the same request: Non-expression parameters: {Expected} Expression parameters: {ConditionExpression}' — same template as cause 2 with the request's own parameter names in the braces.
Emulator: the sentence without the envelope.
Why: as cause 2.
Fix: the PutItem parameter-conflict validation, same shared helper as cause 2.
Traces: dynamodb-legacy-expected-conditional-semantics#task#20.

Cause 4 of 8: BatchWriteItem's over-cap refusal does not render the rejected RequestItems value.
AWS: this is a modelled @length violation on RequestItems, so AWS serves the constraint envelope with the rejected value rendered in its Coral form: "1 validation error detected: Value '{<table>=[WriteRequest(putRequest=PutRequest(item={pk=AttributeValue(s=k0, n=null, ...)}, workloadProfileName=null), deleteRequest=null), ...]}' at 'requestItems' failed to satisfy constraint: ...". The fixed parts are the envelope, the "Value '...' at '<member>' failed to satisfy constraint:" frame and the member path 'requestItems'; the rendered value and the table name come from the request.
Emulator: a different message for the same refusal (the rejected value is not rendered in the Coral AttributeValue form).
Why: callers that surface the service message to an operator lose the identification of which request item set was rejected.
Fix: the BatchWriteItem input validation / its ValidationException message builder — render the rejected member value in the Coral form inside the constraint envelope.
Traces: dynamodb-batch-caps-and-unprocessed#task#9.

Cause 5 of 8: TransactGetItems' over-cap refusal has the same unrendered message.
AWS: Smithy: TransactGetItems.TransactItems has @length min 1 max 100; exceeding it produces the constraint envelope with the TransactItems list rendered as TransactGetItem(get=Get(key={...}, tableName=..., projectionExpression=null, ...)) entries at member path 'transactItems'.
Emulator: a different message for the same refusal.
Why: as cause 4.
Fix: same message builder, on the TransactGetItems input validation.
Traces: dynamodb-batch-caps-and-unprocessed#task#11.

Cause 6 of 8: TransactWriteItems' over-cap refusal likewise.
AWS: the constraint envelope with the TransactWriteItem(conditionCheck=null, put=Put(item={...}, tableName=..., ...), delete=null, update=null) rendering at the transactItems member path.
Emulator: a different message for the same refusal.
Why: as cause 4.
Fix: same message builder, on the TransactWriteItems input validation.
Traces: dynamodb-batch-caps-and-unprocessed#task#12.

Cause 7 of 8: UpdateTable's no-updatable-member refusal enumerates a different member set than AWS.
AWS: the recording shows the bare sentence 'At least one of ProvisionedThroughput, BillingMode, UpdateStreamEnabled, GlobalSecondaryIndexUpdates, SSESpecification, ReplicaUpdates, MultiAccountReplicaReady, ReplicaTransitRoleArn, MultiRegionConsistency, DeletionProtectionEnabled, OnDemandThroughput, WarmThroughput or TableClass is required'. The whole sentence is fixed text — the member names and their order are part of the message, not request-derived.
Emulator: the same sentence shape listing a different set in a different order (StreamSpecification, GlobalTableWitnessUpdates, VectorIndexUpdates, GlobalTableSettingsReplicationMode, DisassociationProtectionEnabled instead of UpdateStreamEnabled, MultiAccountReplicaReady, ReplicaTransitRoleArn).
Why: the message is the only documentation a caller gets of which members can drive an UpdateTable; naming members AWS does not accept (and omitting ones it does) sends callers down branches the service rejects.
Fix: the UpdateTable validation — fix the enumerated member list and its order to AWS's.
Traces: dynamodb-table-billing-mode-update#task#4.

Cause 8 of 8: an enum-constraint refusal lists the allowed values in the wrong order.
AWS: Smithy: ExportViewType is an enum with values [NEW_IMAGE, NEW_AND_OLD_IMAGES], but the recorded message renders the allowed set as '[NEW_AND_OLD_IMAGES, NEW_IMAGE]': "1 validation error detected: Value 'NEW_AND_OLD_IMAGE' at 'incrementalExportSpecification.exportViewType' failed to satisfy constraint: Member must satisfy enum value set: [NEW_AND_OLD_IMAGES, NEW_IMAGE]". The envelope, the frame and the bracketed set are fixed text; the rejected value and member path come from the request.
Emulator: identical except the set is rendered '[NEW_IMAGE, NEW_AND_OLD_IMAGES]' — it uses the model's declaration order rather than the order AWS serves.
Why: exact-match assertions on the refusal message fail, and the message is the contract for enum refusals across the service, so the ordering rule matters wherever an enum value set is rendered.
Fix: the shared enum-constraint message renderer used by the ExportTableToPointInTime validation.
Traces: incremental-export-invalid-windows-rejected#task#8.

Related open issues: '20261030T023722Z-dynamodb-scan-s-unused-expressionattributenames-validationexception-message-is-wrapped.md' reports the same envelope defect for Scan's unused-ExpressionAttributeNames message; it does not cover the GetItem expression-size path, the two missing-envelope conflicts, the three Coral value renderings, the UpdateTable member list or the enum-set ordering. '20261029T180026Z-dynamodb-createtable-updatetable-putitem-validation-10-missing-or.md' covers CreateTable/UpdateTable/PutItem refusals that are missing outright, not these wordings.

## Evidence
Divergence 15 (GetItem, 400->400): $.message expected 'Invalid ProjectionExpression: Expression size has exceeded the maximum allowed size;', got '1 validation error detected: Invalid ProjectionExpression: Expression size has exceeded the maximum allowed size;' (cluster of 3).
Divergence 22 (UpdateItem, 400->400): expected '1 validation error detected: Can not use both expression and non-expression parameters in the same request: Non-expression parameters: {AttributeUpdates} Expression parameters: {UpdateExpression}', got the same text without the envelope (cluster of 2).
Divergence 26 (PutItem, 400->400): expected '1 validation error detected: Can not use both expression and non-expression parameters ... {Expected} ... {ConditionExpression}', got the same text without the envelope.
Divergence 19 (BatchWriteItem, 400->400): expected "1 validation error detected: Value '{<table>=[WriteRequest(putRequest=PutRequest(item={pk=AttributeValue(s=k0, n=null, b=null, ...)}, workloadProfileName=null), deleteRequest=null), ..." — emulator message differs.
Divergence 20 (TransactGetItems, 400->400): expected "1 validation error detected: Value '[TransactGetItem(get=Get(key={pk=AttributeValue(s=k0, ...)}, tableName=..., projectionExpression=null, ..." — differs.
Divergence 21 (TransactWriteItems, 400->400): expected "1 validation error detected: Value '[TransactWriteItem(conditionCheck=null, put=Put(item={pk=AttributeValue(s=k0, ...)}, tableName=..., ..." — differs.
Divergence 35 (UpdateTable, 400->400): expected 'At least one of ProvisionedThroughput, BillingMode, UpdateStreamEnabled, GlobalSecondaryIndexUpdates, SSESpecification, ReplicaUpdates, MultiAccountReplicaReady, ReplicaTransitRoleArn, MultiRegionConsistency, DeletionProtectionEnabled, OnDemandThroughput, WarmThroughput or TableClass is required', got a list naming StreamSpecification/GlobalTableWitnessUpdates/VectorIndexUpdates/GlobalTableSettingsReplicationMode/DisassociationProtectionEnabled.
Divergence 48 (ExportTableToPointInTime, 400->400): expected '... enum value set: [NEW_AND_OLD_IMAGES, NEW_IMAGE]', got '... enum value set: [NEW_IMAGE, NEW_AND_OLD_IMAGES]'.
Model: smithy_query(shape=ExportViewType) -> [NEW_IMAGE, NEW_AND_OLD_IMAGES]; smithy_query(operation=TransactGetItems) -> TransactItems @length min 1 max 100.

## Steps to Reproduce
1. Replay ddbval-expression-over-4kb-rejected#task#7: GetItem with an over-4KB ProjectionExpression -> message carries an extra '1 validation error detected: ' prefix.
2. Replay dynamodb-batch-caps-and-unprocessed#task#14 and dynamodb-legacy-expected-conditional-semantics#task#20: UpdateItem/PutItem mixing expression and non-expression parameters -> message is missing the '1 validation error detected: ' envelope.
3. Replay dynamodb-batch-caps-and-unprocessed#task#9, #task#11, #task#12: over-cap BatchWriteItem/TransactGetItems/TransactWriteItems -> message does not render the rejected member value in the Coral form.
4. Replay dynamodb-table-billing-mode-update#task#4: UpdateTable with no updatable member -> the 'At least one of ...' list names the wrong members.
5. Replay incremental-export-invalid-windows-rejected#task#8: ExportTableToPointInTime with exportViewType 'NEW_AND_OLD_IMAGE' -> enum value set rendered in the wrong order.
