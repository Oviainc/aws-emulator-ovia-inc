Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-index-key-validation-wording
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-index-key-validation-wording ddbctl-list-backups-paginated-by-table ddbctl-list-exports-paginated-by-table-arn
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|PutItem|4xx->2xx|$
Issue-Key: dynamodb|PutItem|4xx->2xx|status
Issue-Key: dynamodb|ListBackups|4xx->4xx|$.message
Issue-Key: dynamodb|ListExports|4xx->4xx|$.message
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Request validation gaps: rejected value omitted from messages and index key types unchecked

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Input validation diverges in two ways: one class of invalid request is not rejected at all, and the requests that are rejected get a validation message that drops the offending value and uses the wrong member spelling. 3 causes, all dynamodb.

Cause 1 of 3: PutItem accepts an item whose secondary-index key attribute has the wrong type and answers 200.
AWS: Smithy: PutItem input Item is a required PutItemInputAttributeMap and the table's AttributeDefinitions fix each index key attribute's AttributeType; writing an item whose index key attribute has a different type is an input-validation failure. The recording shows HTTP 400 with __type 'com.amazon.coral.validate#ValidationException' and a message built from the request naming the index key, the expected type, the actual type and the index (recorded example: 'One or more parameter values were invalid: Type mismatch for Index Key gpk Expected: S Actual: N IndexName: gsi1'). PutItem does not model any success in this case.
Emulator: HTTP 200 with an empty body - the mistyped item is written.
Why: the write succeeds and the table accumulates items that AWS would have refused, so index-backed queries later return results no real table could hold, and clients never see the error that tells them their marshalling is wrong. The emulator is permissive exactly where DynamoDB is strict, which is the hardest class of difference to discover in testing.
Fix: PutItem's validation - before writing, compare each item attribute that participates in a table or secondary-index key against the table's AttributeDefinitions and raise ValidationException (400) on a type mismatch, naming the index key member, expected and actual types and the IndexName.
Traces: dynamodb-index-key-validation-wording#task#3 (cluster of 2, also #task#6).

Cause 2 of 3: ListBackups range-validation message omits the rejected value and uses the output-member spelling of the member name.
AWS: Smithy: ListBackups input member Limit targets BackupsInputLimit with @range min 1 max 100, so Limit=0 is a constraint violation. Status 400 and the error code match; the message is built from the request and by rule carries the rejected value and the wire member path: "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1".
Emulator: emits 'Value at \'Limit\' failed to satisfy constraint: ...' - the rejected value is dropped and the member is spelled 'Limit' instead of the lower-camel 'limit' used in the constraint report.
Why: clients and operators parse these messages to find which value of which member was refused (and some assert on them in contract tests); with the value missing, a rejected request cannot be diagnosed from its error alone, and the differing member spelling defeats message-matching.
Fix: the shared constraint-violation message renderer used by ListBackups' validation - include the rejected value and the lower-camel member path.
Traces: ddbctl-list-backups-paginated-by-table#task#10.

Cause 3 of 3: ListExports range-validation message has the same defect.
AWS: Smithy: ListExports input MaxResults targets ListExportsMaxLimit with @range min 1 max 25, so MaxResults=26 violates the upper bound. Recorded 400 message, built from the request: "1 validation error detected: Value '26' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25".
Emulator: 'Value at \'MaxResults\' failed to satisfy constraint: Member must have value less than or equal to 25' - again no rejected value, and 'MaxResults' instead of 'maxResults'.
Why: as cause 2, on the pagination-bound path.
Fix: same shared constraint-violation message renderer (one fix should cover causes 2 and 3; both must be re-replayed).
Traces: ddbctl-list-exports-paginated-by-table-arn#task#13.

## Evidence
Divergence 19 (PutItem, status 400->200): status expected 400 got 200; body expected {'__type': 'com.amazon.coral.validate#ValidationException', 'message': 'One or more parameter values were invalid: Type mismatch for Index Key gpk Expected: S Actual: N IndexName: gsi1'}, got {} (empty); 2 affected steps (dynamodb-index-key-validation-wording#task#3, #task#6).
Divergence 3 (ListBackups, 400->400): $.message expected "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1", got "1 validation error detected: Value at 'Limit' failed to satisfy constraint: Member must have value greater than or equal to 1".
Divergence 4 (ListExports, 400->400): $.message expected "1 validation error detected: Value '26' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 25", got "1 validation error detected: Value at 'MaxResults' failed to satisfy constraint: Member must have value less than or equal to 25".
Model: smithy_query(operation=ListBackups) -> Limit: BackupsInputLimit @range{min:1,max:100}; smithy_query(operation=ListExports) -> MaxResults: ListExportsMaxLimit @range{min:1,max:25}; smithy_query(operation=PutItem) -> Item: PutItemInputAttributeMap required, no success modeled for key-type mismatch.

## Steps to Reproduce
1. Replay dynamodb-index-key-validation-wording#task#3: PutItem an item whose GSI key attribute 'gpk' is an N where the table's AttributeDefinitions declare S -> 200 empty instead of 400 ValidationException.
2. Replay ddbctl-list-backups-paginated-by-table#task#10: ListBackups with Limit=0 -> 400 whose message omits "'0'" and spells the member 'Limit' rather than 'limit'.
3. Replay ddbctl-list-exports-paginated-by-table-arn#task#13: ListExports with MaxResults=26 -> 400 whose message omits "'26'" and spells the member 'MaxResults' rather than 'maxResults'.
