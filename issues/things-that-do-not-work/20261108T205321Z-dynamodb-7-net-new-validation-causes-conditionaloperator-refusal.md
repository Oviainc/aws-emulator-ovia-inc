Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.conditionaloperator-enum-error-precedes-nesting-depth-in-legacy-expected dynamodb.scan-filter-conditional-operator-document-value-equality dynamodb.scan-query-conditional-operator-needs-two-filter-conditions-rejected dynamodb.write-conditional-operator-needs-two-expected-conditions-rejected dynamodb.execute-statement-missing-clause-rejected dynamodb.execute-transaction-index-select-top-level-validation-not-cancellation dynamodb.null-false-batchget-mixed-projection-and-query-keyconditions-with-expressions dynamodb.scan-query-legacy-and-expression-mixing-names-every-parameter-rejected dynamodb.zero-with-out-of-range-exponent-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.conditionaloperator-enum-error-precedes-nesting-depth-in-legacy-expected|run#16|AssertionError|assert message == MSG_BADOP, f'combo: expected the ConditionalOperator enum message, got {message!r}'
Issue-Key: dynamodb.scan-filter-conditional-operator-document-value-equality|run#25|AssertionError|assert message == (
Issue-Key: dynamodb.scan-query-conditional-operator-needs-two-filter-conditions-rejected|run#9|AssertionError|assert (code, msg) == ("ValidationException", want), f"{label} gave {code!r}: {msg!r}"
Issue-Key: dynamodb.write-conditional-operator-needs-two-expected-conditions-rejected|run#11|AssertionError|assert (code, msg) == ("ValidationException", want), f"{label} gave {code!r}: {msg!r}"
Issue-Key: dynamodb.execute-statement-missing-clause-rejected|_assert_rejected#13|AssertionError|assert message == expected_message, f"unexpected message {message!r}"
Issue-Key: dynamodb.execute-transaction-index-select-top-level-validation-not-cancellation|run#17|AssertionError|assert message == expected, f"unexpected message: {message!r}"
Issue-Key: dynamodb.null-false-batchget-mixed-projection-and-query-keyconditions-with-expressions|_error#6|AssertionError|raise AssertionError(f'expected the call to be refused, but it succeeded: {result!r}')
Issue-Key: dynamodb.scan-query-legacy-and-expression-mixing-names-every-parameter-rejected|run#11|AssertionError|assert (code, msg) == ("ValidationException", want), f"{label} gave {code!r}: {msg!r}"
Issue-Key: dynamodb.zero-with-out-of-range-exponent-rejected|run#20|AssertionError|assert message == _OVERFLOW_MSG, f"expected AWS's exact overflow message, got {message!r}"
Issue-Commit: db4c6069cd883a188788d98b6996561329db7d32

# dynamodb: 7 net-new validation causes — ConditionalOperator refusal text, PartiQL statement/transaction messages, {"NULL": false} accepted, mixed-parameter ordering and the Long.MAX_VALUE exponent class

> _Note: this issue was identified against the code base at commit `db4c6069cd883a188788d98b6996561329db7d32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Seven net-new root causes in the DynamoDB data-plane validation surface. Open issue 20261103T042714Z-dynamodb-legacy-parameters-expected-scanfilter-queryfilter-conditionaloperator-attributestoget.md describes neighbouring ground for Causes 1, 2 and 5, and 20261106T170939Z-dynamodb-2-net-new-executestatement-partiql-rejection-message.md / 20261103T022510Z-dynamodb-expression-and-partiql-diagnostics-refusals-carry-emulator.md for Causes 3 and 4, but the suite marked NONE of the checks below as owned by them, so they are filed here.

=== Cause 1 of 7: the invalid-ConditionalOperator refusal is the generic Smithy enum message, not AWS's text ===
AWS: an unmodelled ConditionalOperator value is ValidationException whose message is the fixed sentence "Failed to satisfy constraint: Member must satisfy enum value set: [ALL, OR]" — no "Value '<v>' at '<member path>'" segment, and the printed value set reads [ALL, OR] even though the modeled enum is AND|OR (Smithy: com.amazonaws.dynamodb#ConditionalOperator, values AND, OR). On the write operations (PutItem/UpdateItem/DeleteItem) that sentence is prefixed with the fixed envelope "1 validation error detected: "; on Scan/Query it is served bare. Fixed text: everything after the optional envelope; there are no request-derived values in this message at all. AWS also orders this defect FIRST: a request carrying both a bad ConditionalOperator and a 40-level-deep Expected comparison value reports the enum sentence, not the nesting-depth sentence ("1 validation error detected: Nesting Levels have exceeded supported limits: Attributes in the item have nested levels beyond supported limit"), which the deep value alone does raise.
Emulator: both operations answer ValidationException "1 validation error detected: Value 'XOR' at 'conditionalOperator' failed to satisfy constraint: Member must satisfy enum value set: [AND, OR]" — it adds the Value/member-path segment AWS omits, prints the modeled set [AND, OR] instead of [ALL, OR], and applies the "1 validation error detected: " envelope on Scan where AWS serves none.
Why: a client that branches on this message (or asserts the documented legacy-parameter text) never matches, and a caller debugging a combined defect is pointed at the wrong parameter.
Fix: the legacy-parameter validator shared by the PutItem/UpdateItem/DeleteItem and Scan/Query handlers must author this sentence itself (and run before the depth check) instead of falling through to the generic Smithy enum-constraint serializer.
Traces: dynamodb.conditionaloperator-enum-error-precedes-nesting-depth-in-legacy-expected, dynamodb.scan-filter-conditional-operator-document-value-equality.

=== Cause 2 of 7: the ConditionalOperator companion-parameter refusals are emulator-authored and do not distinguish the one-element case ===
AWS: ConditionalOperator with no companion legacy map is ValidationException "ConditionalOperator cannot be used without Filter or Expected"; ConditionalOperator with a companion map holding exactly one element is the distinct sentence "ConditionalOperator can only be used when Filter or Expected has two or more elements". Both are fixed text with no request-derived values, named generically after "Filter or Expected" whichever parameter was supplied; on PutItem/UpdateItem/DeleteItem they carry the "1 validation error detected: " envelope, on Scan/Query they do not. This covers Scan with ScanFilter={}, Scan with only AttributesToGet, Query with no key condition, and the one-condition ScanFilter/QueryFilter/Expected forms.
Emulator: it serves its own sentences naming the one parameter it happened to look for — 'ConditionalOperator can only be used together with ScanFilter' on Scan/Query and 'ConditionalOperator can only be used together with Expected' on the writes — with no envelope, and it uses the same sentence for the no-element and one-element cases, so the two defects are indistinguishable.
Why: the message is text AWS never serves, and a client cannot tell "you forgot the filter" from "your filter needs a second condition".
Fix: the ConditionalOperator companion-parameter check in the Scan/Query and PutItem/UpdateItem/DeleteItem handlers — two distinct AWS sentences, selected on the companion map's element count.
Traces: dynamodb.scan-query-conditional-operator-needs-two-filter-conditions-rejected, dynamodb.write-conditional-operator-needs-two-expected-conditions-rejected.

=== Cause 3 of 7: a PartiQL INSERT with no VALUE clause is diagnosed as an UPDATE with no data manipulation ===
AWS: ExecuteStatement rejects each malformed statement class with its own fixed sentence — SELECT with no FROM is "Statement wasn't well formed, can't be processed: Expected FROM after SELECT list", UPDATE with neither SET nor REMOVE is "Statement wasn't well formed, can't be processed: Expected data manipulation", and INSERT INTO "<table>" with no VALUE clause is "Statement wasn't well formed, can't be processed: Unexpected term". The variable part is only the statement the client sent; the trailing diagnosis is fixed per class.
Emulator: `INSERT INTO "<table>"` is refused ValidationException "Statement wasn't well formed, can't be processed: Expected data manipulation" — the UPDATE-class diagnosis — so two different malformed statements are reported identically.
Why: a client (or a PartiQL console) that surfaces the parser diagnosis tells the user to add SET/REMOVE to an INSERT.
Fix: the PartiQL statement parser's error classification behind ExecuteStatement, which must distinguish a truncated INSERT from a dataless UPDATE.
Traces: dynamodb.execute-statement-missing-clause-rejected.

=== Cause 4 of 7: an index-qualified SELECT inside ExecuteTransaction is refused with the BatchExecuteStatement per-slot sentence ===
AWS: a SELECT addressed at an index (`SELECT * FROM "table"."index" WHERE ...`) inside ExecuteTransaction is a TOP-LEVEL ValidationException whose message is "Validation failed in TransactStatements[<i>]: Reads on indices are not supported within transactions." — fixed text with the zero-based statement position taken from the request — and the error carries no CancellationReasons. The same statement inside BatchExecuteStatement is instead a per-slot Error with Code ValidationError and the different sentence "Select statements within BatchExecuteStatement must specify the primary key in the where clause."
Emulator: ExecuteTransaction answers ValidationException with the message 'Select statements within ExecuteTransaction must specify the primary key in the where clause.' — the batch sentence with the operation name substituted. It names the wrong defect (the statement DOES carry the index's key in its WHERE clause), omits the "Validation failed in TransactStatements[0]: " prefix that locates the offending statement, and so loses the index-reads-in-transactions diagnosis entirely.
Why: a client is told to add a primary key to a statement that already has one, and cannot locate which TransactStatements slot was rejected.
Fix: the statement-admission check in the ExecuteTransaction handler (index-qualified reads must be rejected as unsupported-in-transaction, with the TransactStatements[i] prefix), kept separate from the BatchExecuteStatement per-slot check.
Traces: dynamodb.execute-transaction-index-select-top-level-validation-not-cancellation.

=== Cause 5 of 7: an attribute value of {"NULL": false} is accepted and stored ===
AWS: the NULL member of AttributeValue is a boolean whose only legal value is true (Smithy: com.amazonaws.dynamodb#AttributeValue$NULL, documented as `"NULL": true`). A PutItem carrying {"NULL": false} is ValidationException "1 validation error detected: One or more parameter values were invalid: Null attribute value types must have the value of true" — fixed text, no request-derived values — and the item is not created.
Emulator: the PutItem succeeds with HTTP 200 and an empty body (recorded: content-length 2) and the item lands, so a value AWS always refuses is persisted.
Why: the defect never surfaces in the emulator and the same code fails in production; worse, items exist here in a shape AWS cannot store, so a later export/migration breaks.
Fix: AttributeValue validation on the write path (PutItem/UpdateItem/BatchWriteItem/TransactWriteItems) must reject NULL=false before the item is accepted.
Traces: dynamodb.null-false-batchget-mixed-projection-and-query-keyconditions-with-expressions.

=== Cause 6 of 7: the legacy/expression mixing message lists the expression parameters in the wrong order ===
AWS: mixing legacy and expression parameters is ValidationException "Can not use both expression and non-expression parameters in the same request: Non-expression parameters: {<legacy parameters present>} Expression parameters: {<expression parameters present>}". The two brace lists are values taken from the request, but their ORDER is AWS's canonical parameter order, not the order the request or a hash map happens to yield: for a Scan carrying AttributesToGet, ScanFilter, ConditionalOperator, ProjectionExpression and FilterExpression, AWS serves "Non-expression parameters: {AttributesToGet, ScanFilter, ConditionalOperator} Expression parameters: {ProjectionExpression, FilterExpression}".
Emulator: the non-expression list is now correct, but the expression list is emitted as "{FilterExpression, ProjectionExpression}" — ProjectionExpression and FilterExpression are transposed, so the message does not equal AWS's for this request class.
Why: a client asserting on, or parsing, this message to tell the user which parameters collided gets a string AWS never serves.
Fix: the mixed-parameter message builder shared by the Scan/Query handlers — emit both lists in AWS's canonical parameter order.
Traces: dynamodb.scan-query-legacy-and-expression-mixing-names-every-parameter-rejected.

=== Cause 7 of 7: an exponent exactly at Long.MAX_VALUE is reported as unparseable instead of out of range ===
AWS: a number's exponent is parsed as a 64-bit integer and then range-checked against the documented N domain, so the two defects have distinct messages. An exponent that still fits (e.g. 1E9223372036854775807, exponent == 9223372036854775807) is a magnitude violation: "1 validation error detected: Number overflow. Attempting to store a number with magnitude larger than supported range" (fixed text, no values). Only an exponent past that integer's range (e.g. 1E9223372036854775808) cannot be parsed and gets "1 validation error detected: The parameter cannot be converted to a numeric value: <the literal as sent>", whose only variable part is the rejected literal.
Emulator: PutItem of {"N": "1E9223372036854775807"} is refused ValidationException "1 validation error detected: The parameter cannot be converted to a numeric value: 1E9223372036854775807" — the unparseable-literal message for a literal that parses, collapsing the overflow class into the conversion class at the boundary.
Why: a client distinguishing "this number is too big for DynamoDB" from "this is not a number" mis-classifies every value at the boundary, and the same write gets a different message in production.
Fix: the N-attribute parser/validator on the write path — parse the exponent as a 64-bit integer first and report overflow/underflow from the range check; reserve the conversion message for literals that do not parse.
Traces: dynamodb.zero-with-out-of-range-exponent-rejected.

## Evidence
Cause 1 — dynamodb.conditionaloperator-enum-error-precedes-nesting-depth-in-legacy-expected: PutItem with Expected (one AttributeValueList entry nested 40 levels) and ConditionalOperator='XOR'; expected ValidationException "1 validation error detected: Failed to satisfy constraint: Member must satisfy enum value set: [ALL, OR]", got "1 validation error detected: Value 'XOR' at 'conditionalOperator' failed to satisfy constraint: Member must satisfy enum value set: [AND, OR]". dynamodb.scan-filter-conditional-operator-document-value-equality: Scan with ConditionalOperator='XOR'; expected the bare "Failed to satisfy constraint: Member must satisfy enum value set: [ALL, OR]", got the same enveloped Value/path form.

Cause 2 — dynamodb.scan-query-conditional-operator-needs-two-filter-conditions-rejected: Scan(ConditionalOperator='AND', ScanFilter={}) gave 'ValidationException': 'ConditionalOperator can only be used together with ScanFilter', expected 'ConditionalOperator cannot be used without Filter or Expected'; the one-condition ScanFilter/QueryFilter cases expect 'ConditionalOperator can only be used when Filter or Expected has two or more elements'. dynamodb.write-conditional-operator-needs-two-expected-conditions-rejected: PutItem(ConditionalOperator='AND') with no Expected gave 'ConditionalOperator can only be used together with Expected', expected '1 validation error detected: ConditionalOperator cannot be used without Filter or Expected'.

Cause 3 — dynamodb.execute-statement-missing-clause-rejected: ExecuteStatement('INSERT INTO "<table>"') returned message "Statement wasn't well formed, can't be processed: Expected data manipulation", expected "Statement wasn't well formed, can't be processed: Unexpected term" (the SELECT-no-FROM and DELETE-no-WHERE classes in the same check pass).

Cause 4 — dynamodb.execute-transaction-index-select-top-level-validation-not-cancellation: ExecuteTransaction(TransactStatements=[{'Statement': 'SELECT * FROM "t"."gsi1" WHERE gpk=''g1'''}]) returned ValidationException 'Select statements within ExecuteTransaction must specify the primary key in the where clause.', expected 'Validation failed in TransactStatements[0]: Reads on indices are not supported within transactions.'

Cause 5 — dynamodb.null-false-batchget-mixed-projection-and-query-keyconditions-with-expressions: put_item(Item={'pk': {'S': 'n1'}, 'flag': {'NULL': False}}) was expected to be refused ValidationException '1 validation error detected: One or more parameter values were invalid: Null attribute value types must have the value of true'; it SUCCEEDED (HTTPStatusCode 200, content-length 2) and the item was created.

Cause 6 — dynamodb.scan-query-legacy-and-expression-mixing-names-every-parameter-rejected: Scan with AttributesToGet + ScanFilter + ConditionalOperator + FilterExpression + ProjectionExpression gave 'Can not use both expression and non-expression parameters in the same request: Non-expression parameters: {AttributesToGet, ScanFilter, ConditionalOperator} Expression parameters: {FilterExpression, ProjectionExpression}'; expected '... Expression parameters: {ProjectionExpression, FilterExpression}'.

Cause 7 — dynamodb.zero-with-out-of-range-exponent-rejected: put_item(Item={'v': {'N': '1E9223372036854775807'}}) returned '1 validation error detected: The parameter cannot be converted to a numeric value: 1E9223372036854775807', expected '1 validation error detected: Number overflow. Attempting to store a number with magnitude larger than supported range' (the 0E+300 overflow and 0E-300 underflow cases in the same check pass, as does 1E9223372036854775808 getting the conversion message).

## Steps to Reproduce
1. Create a PAY_PER_REQUEST table with pk (S) HASH (plus sk (S) RANGE and a GSI gsi1 on gpk (S) for steps 5-6) and seed one item.
2. Cause 1: ddb.put_item(TableName=t, Item={...}, Expected={'a': {'ComparisonOperator': 'EQ', 'AttributeValueList': [<40-level nested map>]}, 'b': {...}}, ConditionalOperator='XOR') and read Error.Message; repeat as ddb.scan(TableName=t, ConditionalOperator='XOR', ScanFilter={...}).
3. Cause 2: ddb.scan(TableName=t, ConditionalOperator='AND', ScanFilter={}) then ddb.scan(..., ScanFilter={'a': <one EQ condition>}) then ddb.put_item(TableName=t, Item={'pk': {'S': 'k2'}}, ConditionalOperator='AND') and read each Error.Message.
4. Cause 3: ddb.execute_statement(Statement='INSERT INTO "<t>"') and read Error.Message.
5. Cause 4: ddb.execute_transaction(TransactStatements=[{'Statement': 'SELECT * FROM "<t>"."gsi1" WHERE gpk=\'g1\''}]) and read Error.Message and Error for CancellationReasons.
6. Cause 5: ddb.put_item(TableName=t, Item={'pk': {'S': 'n1'}, 'flag': {'NULL': False}}) — expect ValidationException, observe HTTP 200, then get_item and observe the item exists.
7. Cause 6: ddb.scan(TableName=t, AttributesToGet=['a'], ScanFilter={'a': <EQ>}, ConditionalOperator='AND', FilterExpression='a = :v', ProjectionExpression='b', ExpressionAttributeValues={':v': {'S': 'x'}}) and read the two brace lists in Error.Message.
8. Cause 7: ddb.put_item(TableName=t, Item={'pk': {'S': 'exp-at-max'}, 'v': {'N': '1E9223372036854775807'}}) and compare Error.Message with the 1E9223372036854775808 case.
Then replay the dynamodb checks named in trace_ids.
