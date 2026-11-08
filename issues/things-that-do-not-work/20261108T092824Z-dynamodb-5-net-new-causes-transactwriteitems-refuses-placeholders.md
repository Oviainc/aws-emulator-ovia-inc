Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.transact-items-accept-placeholders-without-an-expression dynamodb.expression-attribute-placeholders-without-an-expression-rejected dynamodb.batch-request-items-key-not-a-table-name-rejected dynamodb.deep-dynamodb-1-ddbvec-two-index-updates-in-one-call-rejected dynamodb.deep-dynamodb-1-ddbvec-update-table-backfills-preexisting-items
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|TransactWriteItems|ValidationException
Issue-Key: dynamodb.expression-attribute-placeholders-without-an-expression-rejected|_assert_rejected#15|AssertionError|assert message == expected_message, f"{what}: expected message {expected_message!r}, got {message!r}"
Issue-Key: dynamodb.batch-request-items-key-not-a-table-name-rejected|run#14|AssertionError|assert message == (
Issue-Key: dynamodb.deep-dynamodb-1-ddbvec-two-index-updates-in-one-call-rejected|run#15|AssertionError|assert st == [('idx-orig', 'DELETING')], f'single Delete described {st!r}'
Issue-Key: dynamodb.deep-dynamodb-1-ddbvec-update-table-backfills-preexisting-items|run#17|AssertionError|assert err.get('Code') == 'ValidationException', (
Issue-Commit: d8469fdbee4aefb676d6a1b6581464fb2b4efb05

# dynamodb: 5 net-new causes — TransactWriteItems refuses placeholders without an expression, vector-index UpdateTable/SearchVectors responses diverge, and two ValidationException messages are mis-rendered

> _Note: this issue was identified against the code base at commit `d8469fdbee4aefb676d6a1b6581464fb2b4efb05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TransactWriteItems

## Description
Five independent root causes in the DynamoDB emulator, all grounded in failing behavioral checks that were not already filed or owned. Each cause is separate and must be closed on its own; the issue is NOT closed until all five are fixed.

Cause 1 of 5: TransactWriteItems / TransactGetItems refuse ExpressionAttributeNames or ExpressionAttributeValues carried without any expression.
AWS: Smithy — `Put`, `Delete` (members of TransactWriteItem) and `Get` (member of TransactGetItem) each declare optional `ExpressionAttributeNames` (ExpressionAttributeNameMap) and, for Put/Delete, `ExpressionAttributeValues` (ExpressionAttributeValueMap), independently of the optional `ConditionExpression` / `ProjectionExpression`. There is no modelled constraint tying one to the other, and TransactWriteItems' modelled errors do not include a validation class for this combination. Recorded behavior: a transactional Put or Delete carrying only `ExpressionAttributeNames={"#n":"color"}` (or only `ExpressionAttributeValues={":v":{"S":"red"}}`) and NO expression is ACCEPTED and applied — the item is written / removed — and a TransactGetItems `Get` carrying `ExpressionAttributeNames` with no `ProjectionExpression` returns the FULL item. This is deliberately asymmetric with the single-item ops: PutItem with the same placeholders IS refused, with ValidationException and message `1 validation error detected: ExpressionAttributeNames can only be specified when using expressions` (and the ExpressionAttributeValues twin), and the emulator already matches AWS on those single-item refusals.
Emulator: TransactWriteItems is refused — HTTP 400, code `ValidationException`, message `ExpressionAttributeNames can only be specified when using expressions` (note: also missing the `1 validation error detected: ` envelope AWS uses for the PutItem form, so even the message text is wrong for the class it is imitating). The transactional write never happens.
Why: a client that uses one shared helper to build item requests — attaching a names/values map unconditionally and only sometimes adding a condition — works on AWS for transactions and breaks here. The write is lost, not merely mis-messaged, so a transaction a caller believes committed did not.
Fix: the TransactWriteItems / TransactGetItems handler's per-item validation (the expression-placeholder cross-check must apply to PutItem/DeleteItem/GetItem/Scan but NOT to the TransactItems members).
Traces: dynamodb.transact-items-accept-placeholders-without-an-expression

Cause 2 of 5: GetItem and Scan render the empty-placeholder-map refusal with the `1 validation error detected: ` envelope AWS does not use there.
AWS: recorded behavior only (no modelled constraint — `ExpressionAttributeNameMap` carries no @length min). An empty `{}` ExpressionAttributeNames (or ExpressionAttributeValues) map supplied alongside a valid expression that does use placeholders is a ValidationException whose message differs by operation family: the mutating single-item ops (PutItem, DeleteItem) use the counted envelope `1 validation error detected: ExpressionAttributeNames must not be empty`, while GetItem and Scan use the BARE sentence `ExpressionAttributeNames must not be empty` (and the ExpressionAttributeValues twin `ExpressionAttributeValues must not be empty`). Fixed text: the whole sentence; there are no request- or state-derived variables in it — only the member name (`ExpressionAttributeNames` vs `ExpressionAttributeValues`) varies, taken from which member the caller sent.
Emulator: GetItem with `ProjectionExpression="pk"` and `ExpressionAttributeNames={}` is refused with ValidationException but message `1 validation error detected: ExpressionAttributeNames must not be empty` — the envelope belongs only to the PutItem/DeleteItem form. The emulator already gets the PutItem/DeleteItem variants and the `… can only be specified when using expressions` variants right, so this is the envelope being applied to one message class too broadly.
Why: a client that matches on the refusal text (or a test suite asserting it) sees a message AWS never produces for GetItem/Scan.
Fix: the GetItem and Scan handlers' expression validation, where this message is rendered — the bare form must not go through the counted-constraint envelope. Related and distinct: the resolved issue 20261030T023722Z-dynamodb-scan-s-unused-expressionattributenames-validationexception-message-is-wrapped.md fixed the SAME envelope defect for Scan's `… can only be specified when using expressions` message (that case now passes); the empty-map message on GetItem/Scan was not covered by it.
Traces: dynamodb.expression-attribute-placeholders-without-an-expression-rejected

Cause 3 of 5: the BatchGetItem / BatchWriteItem `RequestItems` map-key refusal drops the echoed request value.
AWS: Smithy — `BatchGetItemInput.RequestItems` (BatchGetRequestMap) and `BatchWriteItemInput.RequestItems` (BatchWriteItemRequestMap) are required maps whose KEY is a table name constrained @length 3..255 and @pattern `[a-zA-Z0-9_.-]+`. A key that violates the floor (e.g. `ab`) is ValidationException, and AWS's counted-constraint message ECHOES the whole offending request value before the member path: `1 validation error detected: Value '<the serialized RequestItems map>' at 'requestItems' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 255, Member must have length greater than or equal to 3, Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+]`. Fixed text: everything outside the quoted value — `1 validation error detected: Value '` … `' at 'requestItems' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 255, Member must have length greater than or equal to 3, Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+]`. Variable part: the quoted value, which is the caller's own RequestItems rendered by the service (JSON-ish for BatchGetItem, the server-side Java toString form for BatchWriteItem) — it comes from the request, never from a fixture.
Emulator: the echoed value is omitted entirely — the message is `1 validation error detected: Value at 'requestItems' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 255, Member must have length greater than or equal to 3, Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+]` (`Value at` with no `'<value>'`). Code and status are right.
Why: the echoed value is how a caller identifies WHICH batch key was rejected when a batch names several tables; without it the refusal is unactionable, and the message is one AWS never emits.
Fix: the shared constraint-report serializer used by the BatchGetItem / BatchWriteItem handlers' RequestItems validation — the map-key branch must render the offending value into the `Value '…'` slot like the scalar branches do.
Traces: dynamodb.batch-request-items-key-not-a-table-name-rejected

Cause 4 of 5: UpdateTable's response to a single vector-index `Delete` omits the index instead of reporting it DELETING.
AWS: Smithy — `UpdateTable` takes `VectorIndexUpdates` (VectorIndexUpdateList) and returns `TableDescription`; `VectorIndexDescription.IndexStatus` is an enum documented as `CREATING` / `ACTIVE` / `DELETING`, where `DELETING` means "the index is being deleted". So an UpdateTable that applies one `Delete` must answer with `TableDescription.VectorIndexes` still LISTING that index, with `IndexStatus` = `DELETING`; the entry disappears only once the delete completes (DescribeTable then reports no VectorIndexes and the table ACTIVE).
Emulator: the UpdateTable response's `TableDescription.VectorIndexes` is empty/absent immediately — the deleted index is dropped from the response rather than reported as `DELETING`. (The refusal half of the same check — two index updates in one call answering LimitExceededException `Subscriber limit exceeded: Only 1 online index can be created or deleted simultaneously per table` and leaving the index set unchanged — passes; only this response shape diverges.)
Why: a client that reads the UpdateTable response to learn the new index state (and to poll from `DELETING` to gone) sees the index as already gone, and cannot distinguish "delete requested" from "delete finished" or from "the index never existed".
Fix: the UpdateTable handler's vector-index branch and its TableDescription serializer — a requested delete must set IndexStatus=DELETING and keep the entry in the returned VectorIndexes list.
Traces: dynamodb.deep-dynamodb-1-ddbvec-two-index-updates-in-one-call-rejected

Cause 5 of 5: SearchVectors against a vector index that is still CREATING answers the wrong error class.
AWS: Smithy — `SearchVectors` requires `TableName` and `IndexName`, and `VectorIndexDescription.IndexStatus` documents `CREATING` as not-yet-usable (`Backfilling` notes that results may be incomplete only once the index is searchable). Recorded behavior: searching an index that UpdateTable has just created, while its status is `CREATING`, is refused with HTTP 400, code `ValidationException`, message `The table does not have the specified index: <IndexName>` — fixed text `The table does not have the specified index: `, variable part the `IndexName` taken from the request. The same code and message are what a search on a DELETED index returns, so the index is simply not addressable until it is ACTIVE.
Emulator: the call is refused with code `ResourceNotFoundException` and message `Requested resource not found` — the wrong error class, and a message with no mention of the index. (SearchVectors does model ResourceNotFoundException, so this is not an unmodelled code; it is the wrong one for this condition.)
Why: ResourceNotFoundException reads as "the table is gone" and is retry-class for most clients, whereas ValidationException is a terminal request defect naming the index; a client polling an index into service either retries forever or tears the table down.
Fix: the SearchVectors handler's index lookup — an index present but not ACTIVE must be rejected as ValidationException with the not-in-table message, not fall through to the table-level not-found path.
Traces: dynamodb.deep-dynamodb-1-ddbvec-update-table-backfills-preexisting-items

## Evidence
All five from the behavioral suite replay against this checkout (service heading `### dynamodb`).

Cause 1 — dynamodb.transact-items-accept-placeholders-without-an-expression (verdict: error):
  expected: transact_write_items(TransactItems=[{'Put': {'TableName': t, 'Item': {...}, 'ExpressionAttributeNames': {'#n': 'color'}}}]) succeeds and the item is readable.
  actual:   botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the TransactWriteItems operation: ExpressionAttributeNames can only be specified when using expressions
  (the same check's PutItem control — refusal with `1 validation error detected: ExpressionAttributeNames can only be specified when using expressions` — passed.)

Cause 2 — dynamodb.expression-attribute-placeholders-without-an-expression-rejected (verdict: mismatch):
  AssertionError: GetItem with an empty EAN map and a valid ProjectionExpression: expected message 'ExpressionAttributeNames must not be empty', got '1 validation error detected: ExpressionAttributeNames must not be empty'
  (the PutItem/DeleteItem envelope forms and the `… can only be specified when using expressions` forms for PutItem/DeleteItem/GetItem/Scan all passed earlier in the same check.)

Cause 3 — dynamodb.batch-request-items-key-not-a-table-name-rejected (verdict: mismatch):
  expected: "1 validation error detected: Value '{\"ab\":{\"Keys\":[{\"pk\":{\"S\":\"a\"}}]}}' at 'requestItems' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 255, Member must have length greater than or equal to 3, Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+]"
  actual:   "1 validation error detected: Value at 'requestItems' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 255, Member must have length greater than or equal to 3, Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+]"
  (code ValidationException matched; only the echoed value is missing.)

Cause 4 — dynamodb.deep-dynamodb-1-ddbvec-two-index-updates-in-one-call-rejected (verdict: mismatch):
  expected: UpdateTable(VectorIndexUpdates=[{'Delete': {'IndexName': 'idx-orig'}}])['TableDescription']['VectorIndexes'] -> [('idx-orig', 'DELETING')]
  actual:   AssertionError: single Delete described []

Cause 5 — dynamodb.deep-dynamodb-1-ddbvec-update-table-backfills-preexisting-items (verdict: mismatch):
  expected: SearchVectors on the just-created, still-CREATING index 'late-euc' -> ValidationException, 'The table does not have the specified index: late-euc'
  actual:   AssertionError: searching a CREATING vector index: expected ValidationException, got {'Message': 'Requested resource not found', 'Code': 'ResourceNotFoundException'}

## Steps to Reproduce
Replay the five named checks against the emulator (run_behavioral_suite, or each check standalone):

1. Cause 1: create an on-demand table keyed pk(S). Call TransactWriteItems with one Put whose item is well formed and which carries ExpressionAttributeNames={'#n':'color'} and no ConditionExpression; repeat with ExpressionAttributeValues={':v':{'S':'red'}}; repeat the pair as Delete; then TransactGetItems with a Get carrying ExpressionAttributeNames and no ProjectionExpression. All four writes must be applied and the Get must return the full item. (Control: the same placeholders on PutItem must still be refused with the `1 validation error detected: …` message.)
2. Cause 2: on the same table call GetItem(Key=..., ProjectionExpression='pk', ExpressionAttributeNames={}) and Scan(FilterExpression='attribute_exists(pk)', ExpressionAttributeNames={}) / ExpressionAttributeValues={}. Each must be ValidationException with the BARE message `ExpressionAttributeNames must not be empty` / `ExpressionAttributeValues must not be empty`.
3. Cause 3: call BatchGetItem(RequestItems={'ab': {'Keys': [{'pk': {'S': 'a'}}]}}) and BatchWriteItem(RequestItems={'ab': [{'PutRequest': {'Item': {}}}]}). Each must be ValidationException whose message quotes the caller's RequestItems in the `Value '…'` slot before `at 'requestItems'`.
4. Cause 4: CreateTable with VectorIndexes=[{'IndexName':'idx-orig', ...}], wait ACTIVE, then UpdateTable(VectorIndexUpdates=[{'Delete': {'IndexName':'idx-orig'}}]). The response's TableDescription.VectorIndexes must still list idx-orig with IndexStatus='DELETING'.
5. Cause 5: CreateTable without VectorIndexes, put items, UpdateTable(VectorIndexUpdates=[{'Create': {'IndexName':'late-euc', ...}}]), then immediately SearchVectors(TableName=t, IndexName='late-euc', TopK=5, SearchVector=[...]) while the index is CREATING. It must be ValidationException with message `The table does not have the specified index: late-euc`.
