Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.create-table-lsi-hash-key-differs-from-table-hash-key-rejected dynamodb.create-table-with-six-local-secondary-indexes-rejected dynamodb.ddb-gsi-select-count-scan-from-a-cursor-missing-its-sort-key-rejected dynamodb.delete-set-operand-type-mismatch-rejected dynamodb.exclusive-start-key-outside-range-and-missing-attribute-rejected dynamodb.execute-transaction-exceeds-100-statements-rejected dynamodb.update-table-sse-nonexistent-cmk-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.create-table-lsi-hash-key-differs-from-table-hash-key-rejected|run#14|AssertionError|assert message == expected_msg, f"expected message {expected_msg!r}, got {message!r}"
Issue-Key: dynamodb.create-table-with-six-local-secondary-indexes-rejected|run#20|AssertionError|assert message == expected_msg, f"expected message {expected_msg!r}, got {message!r}"
Issue-Key: dynamodb.ddb-gsi-select-count-scan-from-a-cursor-missing-its-sort-key-rejected|run#13|AssertionError|assert got == ("ValidationException", _TRUNCATED_EXPECTED), (
Issue-Key: dynamodb.delete-set-operand-type-mismatch-rejected|run#10|AssertionError|assert (code1, message1) == ("ValidationException", expected1), (
Issue-Key: dynamodb.exclusive-start-key-outside-range-and-missing-attribute-rejected|run#13|AssertionError|assert message == 'The provided starting key does not match the range key predicate', (
Issue-Key: dynamodb.execute-transaction-exceeds-100-statements-rejected|run#20|AssertionError|assert message.startswith(prefix) and message.endswith(suffix), (
Issue-Key: dynamodb.update-table-sse-nonexistent-cmk-rejected|run#13|AssertionError|assert re.sub(r'Request ID: [^;)]*', 'Request ID: <request-id>', msg) == expected, (
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# dynamodb: 7 net-new causes — ValidationException message text diverges from AWS on LSI/limit/operand/key-cursor/transaction-length/KMS refusals

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
Seven distinct causes, all of them a correct refusal carrying the wrong ValidationException message. An HTTP status, an error code and an error message are requirements; each cause below gives AWS's fixed text and names which parts are request/state-derived.

Cause 1 of 7: CreateTable with an LSI whose HASH key differs from the table's.
AWS: refuses with ValidationException and the fixed text `One or more parameter values were invalid: Index KeySchema does not have the same leading hash key as table KeySchema for index: <IndexName>. index hash key: <indexHashAttr>, table hash key: <tableHashAttr>` — index name and the two attribute names come from the request.
Emulator: `Invalid KeySchema for index lsi1: a table's local secondary index must have the table's partition key pk as its HASH key, got other` — invented fixed text in a different shape.
Fix: CreateTable's key-schema validation message.
Traces: create-table-lsi-hash-key-differs-from-table-hash-key-rejected.

Cause 2 of 7: CreateTable with six LSIs.
AWS: ValidationException `One or more parameter values were invalid: Number of LocalSecondaryIndexes exceeds per-table limit of 5` — entirely fixed text (the limit 5 is AWS's documented per-table LSI limit).
Emulator: `Invalid LocalSecondaryIndexes: a table may carry at most 5 local secondary indexes, got 6`.
Fix: CreateTable's LSI-count validation message.
Traces: create-table-with-six-local-secondary-indexes-rejected.

Cause 3 of 7: a Select=COUNT scan resumed from an ExclusiveStartKey that omits the index's sort key.
AWS: ValidationException with the fixed text `The provided starting key is invalid` and nothing appended.
Emulator: `The provided starting key is invalid: The provided key element does not match the schema` — it concatenates the schema-mismatch message onto the cursor message.
Fix: the ExclusiveStartKey validation in Scan/Query, which must not append the inner key-schema diagnosis.
Traces: ddb-gsi-select-count-scan-from-a-cursor-missing-its-sort-key-rejected.

Cause 4 of 7: UpdateExpression `DELETE #s :v` with a Number operand.
AWS: ValidationException `1 validation error detected: Invalid UpdateExpression: Incorrect operand type for operator or function; operator: DELETE, operand type: NUMBER, typeSet: ALLOWED_FOR_ADD_OPERAND` — the operator name and operand type are derived from the expression; the rest is fixed, including the `1 validation error detected: ` prefix.
Emulator: `An operand in the update expression has an incorrect data type` — a generic message with no prefix, no operator and no type set.
Fix: the update-expression operand type checker's message (and the `N validation error(s) detected:` prefixing).
Traces: delete-set-operand-type-mismatch-rejected.

Cause 5 of 7: an ExclusiveStartKey outside the key-condition range / missing an attribute.
AWS: ValidationException with AWS's exact text for an out-of-range cursor.
Emulator: `The provided starting key is outside query boundaries based on provided conditions` — not the recorded message.
Fix: the ExclusiveStartKey validation message in Query/Scan.
Traces: exclusive-start-key-outside-range-and-missing-attribute-rejected.

Cause 6 of 7: ExecuteTransaction with more than 100 statements.
AWS: ValidationException whose text starts `1 validation error detected: Value '[` , renders the offending member value, and ends `]' at 'transactStatements' failed to satisfy constraint: Member must have length less than or equal to 100`. Smithy: ExecuteTransaction.TransactStatements @length(min 1, max 100) — the constraint-violation message renders the value, not a summary of it.
Emulator: `1 validation error detected: Value '101 items' at 'transactStatements' failed to satisfy constraint: Member must have length less than or equal to 100` — it substitutes the literal text `101 items` for the value.
Fix: the constraint-violation renderer for list members in the AWS JSON validation layer.
Traces: execute-transaction-exceeds-100-statements-rejected.

Cause 7 of 7: UpdateTable SSESpecification naming a nonexistent CMK.
AWS: ValidationException relaying the KMS failure with the fixed shape `KMS validation error: com.amazonaws.services.kms.model.NotFoundException: Key '<key-arn>' does not exist (Service: AWSKMS; Status Code: 400; Error Code: NotFoundException; Request ID: <request-id>; Proxy: null)` — key ARN and request id are variable; `Service: AWSKMS`, the semicolon separators and the `Error Code: NotFoundException` field are fixed.
Emulator: `KMS validation error: com.amazonaws.services.kms.model.NotFoundException: Key '<key-arn>' does not exist (Service: Kms, Status Code: 400, Request ID: <id>; Proxy: null)` — service name `Kms`, comma separators, and the `Error Code` field dropped.
Fix: the KMS-relay message builder used by UpdateTable/CreateTable SSESpecification validation.
Traces: update-table-sse-nonexistent-cmk-rejected.

## Evidence
Cause 1: expected 'One or more parameter values were invalid: Index KeySchema does not have the same leading hash key as table KeySchema for index: lsi1. index hash key: other, table hash key: pk', got "Invalid KeySchema for index lsi1: a local secondary index must have the table's partition key pk as its HASH key, got other"
Cause 2: expected 'One or more parameter values were invalid: Number of LocalSecondaryIndexes exceeds per-table limit of 5', got 'Invalid LocalSecondaryIndexes: a table may carry at most 5 local secondary indexes, got 6'
Cause 3: a COUNT scan from a cursor without the sort key answered ('ValidationException', 'The provided starting key is invalid: The provided key element does not match the schema'), expected ('ValidationException', 'The provided starting key is invalid')
Cause 4: DELETE #s :v with a Number operand raised ('ValidationException', 'An operand in the update expression has an incorrect data type'), expected ('ValidationException', '1 validation error detected: Invalid UpdateExpression: Incorrect operand type for operator or function; operator: DELETE, operand type: NUMBER, typeSet: ALLOWED_FOR_ADD_OPERAND')
Cause 5: expected AWS's exact ValidationException message, got 'The provided starting key is outside query boundaries based on provided conditions'
Cause 6: expected the message to start with "1 validation error detected: Value '[" and end with "]' at 'transactStatements' failed to satisfy constraint: Member must have length less than or equal to 100", got "1 validation error detected: Value '101 items' at 'transactStatements' failed to satisfy constraint: Member must have length less than or equal to 100"
Cause 7: expected "... (Service: AWSKMS; Status Code: 400; Error Code: NotFoundException; Request ID: <request-id>; Proxy: null)" (request id masked), got "... (Service: Kms, Status Code: 400, Request ID: <id>; Proxy: null)"

## Steps to Reproduce
Cause 1: CreateTable with KeySchema HASH=pk and an LSI whose HASH is another attribute.
Cause 2: CreateTable with six LocalSecondaryIndexes.
Cause 3: Query/Scan an index with Select=COUNT and an ExclusiveStartKey missing the index sort key.
Cause 4: UpdateItem with UpdateExpression 'DELETE #s :v' where :v is an N.
Cause 5: Query with an ExclusiveStartKey outside the key condition's range.
Cause 6: ExecuteTransaction with 101 TransactStatements.
Cause 7: UpdateTable with SSESpecification KMSMasterKeyId pointing at a nonexistent key ARN.
