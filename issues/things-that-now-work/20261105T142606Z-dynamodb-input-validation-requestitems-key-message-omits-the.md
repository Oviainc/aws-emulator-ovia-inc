Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-batch-request-items-key-not-a-table-name-rejected
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-batch-request-items-key-not-a-table-name-rejected dynamodb-malformed-n-in-key-top-level-not-cancellation dynamodb-create-and-update-table-throughput-mode-invalid-enum dynamodb-csvtraj-create-table-enum-capacity-and-projection-mistakes dynamodb-csvtraj-every-csv-column-declared-as-attribute dynamodb-csvtraj-gsi-on-undeclared-csv-column dynamodb-csvtraj-single-column-csv-hash-reused-as-range dynamodb-csvtraj-table-name-derived-from-object-key dynamodb-multiple-simultaneous-constraint-violations-rejected dynamodb-vvd-traj-create-table-billing-mode-mistakes dynamodb-vvd-traj-create-table-key-schema-mistakes dynamodb-deleteitem-expression-validation-rejected dynamodb-expression-attribute-value-empty-rejected dynamodb-partiql-select-nested-document-paths
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|BatchGetItem|4xx->4xx|$.message
Issue-Key: dynamodb|CreateTable|4xx->4xx|$.message
Issue-Key: dynamodb|DeleteItem|4xx->4xx|$.message
Issue-Key: dynamodb|ExecuteStatement|4xx->2xx|$.__type
Issue-Key: dynamodb|ExecuteStatement|4xx->2xx|$.message
Issue-Key: dynamodb|ExecuteStatement|4xx->2xx|status
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# DynamoDB input validation: RequestItems key message omits the offending value, BillingMode enum unchecked, ExpressionAttributeNames-without-expression mis-worded, PartiQL duplicate select identifiers accepted

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetItem

## Description
Four net-new request-validation root causes in the DynamoDB emulator. In each one the emulator either serves an error whose text/class is not the one AWS serves for that defect class, or does not validate the defect at all. This bundle contains 4 causes; all four must be closed.

Cause 1 of 4 — BatchGetItem: the RequestItems map-key constraint violation message omits the rejected value.
AWS: Smithy: BatchGetItem.RequestItems is required, target BatchGetRequestMap (@length min 1 max 100), whose KEY targets TableArn; the table-name map key is validated by the Coral input validator and the refusal is a 400 ValidationException. The recorded response shows AWS's message template for a map-key constraint failure: the fixed text is "1 validation error detected: Value '" + <value> + "' at 'requestItems' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 255, Member must have length greater than or equal to 3, Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+]". The only variable part is <value>: the JSON rendering of the submitted requestItems map taken from the request (in the recording a short table name keyed map). The constraint list, the member path 'requestItems' and the error-count prefix are fixed.
Emulator: 400 with the same prefix and constraint list but with the value elided — "1 validation error detected: Value at 'requestItems' failed to satisfy constraint: Map keys must satisfy constraint: [...]" — i.e. the "Value '<rendered request value>'" segment is dropped.
Why: a client (or an SDK/CLI user) debugging a rejected batch cannot tell WHICH request-items key was rejected, because AWS's message is the only place the offending value is reported; message-matching test suites and log scrapers that key on the `Value '...' at '...'` shape also fail to parse it.
Fix: the shared Coral-style input-validation message builder used by the BatchGetItem/BatchWriteItem request-shape validation — the value-rendering branch for map-key constraint failures.
Traces: dynamodb-batch-request-items-key-not-a-table-name-rejected#task#5; dynamodb-malformed-n-in-key-top-level-not-cancellation#task#7 (2 steps in cluster).

Cause 2 of 4 — CreateTable/UpdateTable: an invalid BillingMode enum value is never enum-validated, so a different (semantic) error is served instead.
AWS: Smithy: CreateTable.BillingMode targets the enum BillingMode with values [PROVISIONED, PAY_PER_REQUEST]. An unmodeled value is refused by input validation BEFORE any capacity/throughput semantics are evaluated, as 400 ValidationException with message "1 validation error detected: Value '<supplied value>' at 'tableThroughputMode' failed to satisfy constraint: Member must satisfy enum value set: [PROVISIONED, PAY_PER_REQUEST]". Fixed text: the count prefix, " at 'tableThroughputMode' failed to satisfy constraint: Member must satisfy enum value set: [PROVISIONED, PAY_PER_REQUEST]" (note the wire member BillingMode is reported under its service-internal member path tableThroughputMode). Variable: the rejected value copied from the request.
Emulator: 400 but with a completely different, semantic message — "One or more parameter values were invalid: Both ReadCapacityUnits and WriteCapacityUnits must be specified when BillingMode is PROVISIONED". The enum is not validated; the unknown value falls through and is treated as (or defaulted to) PROVISIONED, so the later capacity-presence check fires and masks the real defect. Related open issue 20261104T061030Z-dynamodb-createtable-and-updatetable-drop-the-throughput-mode.md covers the throughput-mode member being dropped from responses; it does not cover this enum-validation/precedence gap.
Why: a client that mistypes or sends a newer BillingMode is told to supply capacity units; following that advice does not fix the request, and the client cannot discover the member or the legal value set from the error. Error-precedence-sensitive clients also see the wrong first defect when several are present.
Fix: CreateTable/UpdateTable input validation — enum validation of BillingMode must run in the request-shape validation pass, before the provisioned-capacity consistency check.
Traces: dynamodb-create-and-update-table-throughput-mode-invalid-enum#task#3 and #task#5; dynamodb-vvd-traj-create-table-billing-mode-mistakes#task#1..#3; dynamodb-csvtraj-create-table-enum-capacity-and-projection-mistakes#task#3..#4 (16 steps in cluster).

Cause 3 of 4 — DeleteItem: ExpressionAttributeNames supplied with NO expression is refused with the wrong message (the 'unused names' message instead of the 'only with expressions' message).
AWS: Smithy: DeleteItem.ExpressionAttributeNames targets ExpressionAttributeNameMap and is optional; DeleteItem has ConditionExpression as its only expression member. When ExpressionAttributeNames is present and the request carries no expression at all, AWS refuses with 400 ValidationException and the entirely fixed message "1 validation error detected: ExpressionAttributeNames can only be specified when using expressions" (no variable parts). This is a distinct defect class from "names supplied but not referenced by an expression that IS present", which is what AWS words as "Value provided in ExpressionAttributeNames unused in expressions: keys: {#x}".
Emulator: 400 but with the other class's message — "Value provided in ExpressionAttributeNames unused in expressions: keys: {#s}" — so the two defect classes are collapsed onto the unused-names message.
Why: the two messages mean different things to a client: one says remove the names, the other says add the expression. A client told its names are "unused in expressions" when it supplied no expression at all gets a misleading diagnosis, and suites asserting the documented wording fail.
Fix: DeleteItem's request validation (shared item-op expression-parameter validation): branch on whether ANY expression member is present before emitting the unused-names diagnostic.
Traces: dynamodb-deleteitem-expression-validation-rejected#task#5 and #task#6; dynamodb-expression-attribute-value-empty-rejected#task#6 and #task#7 (4 steps in cluster).

Cause 4 of 4 — ExecuteStatement (PartiQL): a SELECT clause that names the same identifier twice is executed instead of refused.
AWS: Smithy: ExecuteStatement.Statement is required, target PartiQLStatement (@length 1..8192); statement-level semantic validation is prose-only in the model, so the requirement here is the recorded behavior: AWS refuses the statement with HTTP 400, __type "com.amazon.coral.validate#ValidationException" and message "Duplicate identifiers in select clause: <identifier>". Fixed text: "Duplicate identifiers in select clause: "; variable: the duplicated projection identifier(s) taken from the submitted statement.
Emulator: HTTP 200 with a result set — no __type and no message at all; the duplicate projection is silently accepted and the statement is evaluated.
Why: the client is served data for a statement AWS rejects, so a query bug that fails in production passes against the emulator, and the shape of the returned Items (duplicate identifier collapsed) is not something AWS ever produces. Error-path code is never exercised.
Fix: the PartiQL statement parser/validator for ExecuteStatement (SELECT projection-list validation), raising ValidationException before execution.
Traces: dynamodb-partiql-select-nested-document-paths#task#9 (1 step in cluster).

## Evidence
Cause 1 — BatchGetItem, fixture dynamodb-batch-request-items-key-not-a-table-name-rejected step task#5 (cluster of 2):
  expected 400, actual 400
  $.message expected: "1 validation error detected: Value '{\"ab\":{\"Keys\":[{\"pk\":{\"S\":\"a\"}}]}}' at 'requestItems' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 255, Member must have length greater than or equal to 3, Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+]"
  $.message actual:   "1 validation error detected: Value at 'requestItems' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 255, Member must have length greater than or equal to 3, Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+]"
  (the JSON blob shown is this recording's request value; the requirement is that the submitted value is rendered there, not that exact blob)

Cause 2 — CreateTable, fixture dynamodb-create-and-update-table-throughput-mode-invalid-enum step task#3 (cluster of 16):
  expected 400, actual 400
  $.message expected: "1 validation error detected: Value 'BOGUS_MODE' at 'tableThroughputMode' failed to satisfy constraint: Member must satisfy enum value set: [PROVISIONED, PAY_PER_REQUEST]"
  $.message actual:   "One or more parameter values were invalid: Both ReadCapacityUnits and WriteCapacityUnits must be specified when BillingMode is PROVISIONED"
  ('BOGUS_MODE' is this recording's supplied value; the requirement is the template with the request's value)

Cause 3 — DeleteItem, fixture dynamodb-deleteitem-expression-validation-rejected step task#5 (cluster of 4):
  expected 400, actual 400
  $.message expected: "1 validation error detected: ExpressionAttributeNames can only be specified when using expressions"
  $.message actual:   "Value provided in ExpressionAttributeNames unused in expressions: keys: {#s}"

Cause 4 — ExecuteStatement, fixture dynamodb-partiql-select-nested-document-paths step task#9 (cluster of 1):
  expected 400, actual 200
  $.__type expected: "com.amazon.coral.validate#ValidationException", actual: absent
  $.message expected: "Duplicate identifiers in select clause: b", actual: absent
  ('b' is this recording's duplicated identifier)

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1 against the emulator built from this checkout (replay_wire_corpus), then:

Cause 1: fixture dynamodb-batch-request-items-key-not-a-table-name-rejected, step task#5 — BatchGetItem whose RequestItems map key violates the table-name constraints (here a 2-character key); compare $.message with the recorded response. Also dynamodb-malformed-n-in-key-top-level-not-cancellation#task#7.

Cause 2: fixture dynamodb-create-and-update-table-throughput-mode-invalid-enum, steps task#3 and task#5 — CreateTable/UpdateTable with BillingMode set to a value outside [PROVISIONED, PAY_PER_REQUEST] and no ProvisionedThroughput; expect the enum ValidationException, observe the capacity-units message. Also dynamodb-vvd-traj-create-table-billing-mode-mistakes#task#1.

Cause 3: fixture dynamodb-deleteitem-expression-validation-rejected, steps task#5/task#6 — DeleteItem with ExpressionAttributeNames present and no ConditionExpression (no expression member at all); expect "ExpressionAttributeNames can only be specified when using expressions".

Cause 4: fixture dynamodb-partiql-select-nested-document-paths, step task#9 — ExecuteStatement with a SELECT clause naming the same identifier twice; expect 400 ValidationException "Duplicate identifiers in select clause: <id>", observe 200 with Items.
