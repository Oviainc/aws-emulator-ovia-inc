Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.duplicate-set-members-in-condition-key-and-transact-rejected dynamodb.empty-and-duplicate-set-members-rejected-for-ss-ns-and-bs dynamodb.execute-statement-parameter-count-and-key-type-mismatch-rejected dynamodb.query-scan-exclusive-start-key-empty-value-rejected dynamodb.ddb-frc-update-item-setting-a-gsi-key-to-the-empty-string-rejected dynamodb.ddb-gsi-query-on-numeric-index-key-with-a-string-value-rejected dynamodb.number-precision-limited-to-38-digits dynamodb.empty-string-expression-rejected-across-operations dynamodb.transact-get-items-top-level-vs-cancellation-error-class dynamodb.transact-write-items-condition-expression-defects-match-putitem-message
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.duplicate-set-members-in-condition-key-and-transact-rejected|run#19|AssertionError|assert message == invalid + 'Input collection ["a", "a"] contains duplicates.', (
Issue-Key: dynamodb.empty-and-duplicate-set-members-rejected-for-ss-ns-and-bs|run#22|AssertionError|assert message == expected_key_message, (
Issue-Key: dynamodb.execute-statement-parameter-count-and-key-type-mismatch-rejected|run#17|AssertionError|assert message == "Key attribute's data type should match its data type in table's schema: Key pk", (
Issue-Key: dynamodb.query-scan-exclusive-start-key-empty-value-rejected|run#24|AssertionError|assert message == expected, f'Query/{label}: expected {expected!r}, got {message!r}'
Issue-Key: dynamodb.ddb-frc-update-item-setting-a-gsi-key-to-the-empty-string-rejected|run#11|AssertionError|assert got == ("ValidationException", _INDEX_EXPECTED), (
Issue-Key: dynamodb.ddb-gsi-query-on-numeric-index-key-with-a-string-value-rejected|run#7|AssertionError|assert message == _TYPE_MISMATCH_MESSAGE, (
Issue-Key: dynamodb.number-precision-limited-to-38-digits|run#37|AssertionError|assert message == "Attempting to store more than 38 significant digits in a Number", (
Issue-Key: dynamodb.empty-string-expression-rejected-across-operations|run#26|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: dynamodb.transact-get-items-top-level-vs-cancellation-error-class|run#26|AssertionError|assert resp['Error']['Code'] == 'TransactionCanceledException', (
Issue-Key: dynamodb.transact-write-items-condition-expression-defects-match-putitem-message|run#12|AssertionError|assert _PREFIX + tx_message == put_message, (
Issue-Commit: 9fc0802c1fad78cd4c44ae6731253718bd889678

# dynamodb: 10 net-new key/attribute-value validation causes — duplicate and empty set members, key type mismatch, 38-digit overflow, empty-expression and TransactGetItems refusals all answer the wrong message or the wrong error class

> _Note: this issue was identified against the code base at commit `9fc0802c1fad78cd4c44ae6731253718bd889678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
Ten distinct validation causes in DynamoDB's request-validation layer. Every one of them is a refusal whose HTTP status and error code are right but whose message (or, in one case, error class) is not AWS's. Each check asserts AWS's exact message, so the expected text quoted below is AWS's recorded wording for that error class; the variable parts are named per cause.

Cause 1 of 10 — a duplicate member in a set used as a KEY is reported as a schema mismatch. Failing check: dynamodb.duplicate-set-members-in-condition-key-and-transact-rejected.
AWS: 400 ValidationException, message `One or more parameter values were invalid: Input collection ["a", "a"] contains duplicates.` — fixed text `One or more parameter values were invalid: Input collection ` + ` contains duplicates.` around the offending collection rendered from the request.
Emulator: 400 ValidationException with `The provided key element does not match the schema`.

Cause 2 of 10 — the same duplicate/empty-member rule for SS/NS/BS on a key. Failing check: dynamodb.empty-and-duplicate-set-members-rejected-for-ss-ns-and-bs.
AWS: `One or more parameter values were invalid: Input collection ["a", "a"] contains duplicates.`
Emulator: `The provided key element does not match the schema`.

Cause 3 of 10 — PartiQL key type mismatch. Failing check: dynamodb.execute-statement-parameter-count-and-key-type-mismatch-rejected.
AWS: 400 ValidationException, message `Key attribute's data type should match its data type in table's schema: Key pk` — fixed text `Key attribute's data type should match its data type in table's schema: Key ` plus the key attribute name from the request.
Emulator: `The provided key element does not match the schema`.

Cause 4 of 10 — an empty key value in ExclusiveStartKey. Failing check: dynamodb.query-scan-exclusive-start-key-empty-value-rejected.
AWS: 400 ValidationException, message `One or more parameter values are not valid. The AttributeValue for a key attribute cannot contain an empty binary value. Key: gpk` — fixed text `One or more parameter values are not valid. The AttributeValue for a key attribute cannot contain an empty ` + `<string|binary>` + ` value. Key: ` plus the key attribute name from the request.
Emulator: `The provided starting key is invalid`.

Cause 5 of 10 — an UpdateExpression that sets a secondary-index key to the empty string. Failing check: dynamodb.ddb-frc-update-item-setting-a-gsi-key-to-the-empty-string-rejected.
AWS: 400 ValidationException, message `One or more parameter values are not valid. The update expression attempted to update a secondary index key to a value that is not supported. The AttributeValue for a key attribute cannot contain an empty string value.` — entirely fixed text; AWS does NOT name the index or key here.
Emulator: `One or more parameter values are not valid. A value specified for a secondary index key is not supported. The AttributeValue for a key attribute cannot contain an empty string value. IndexName: by-tag, IndexKey: tag` — wrong sentence plus index/key names AWS does not emit.

Cause 6 of 10 — a Query key condition whose operand type does not match the index key's schema type. Failing check: dynamodb.ddb-gsi-query-on-numeric-index-key-with-a-string-value-rejected.
AWS: 400 ValidationException, message `One or more parameter values were invalid: Condition parameter type does not match schema type` — entirely fixed text.
Emulator: `One or more parameter values were invalid: Type mismatch for key CreationTime expected: N actual: S` (the key-level message AWS uses for a key in an item write, not for a key condition).

Cause 7 of 10 — a Number with more than 38 significant digits. Failing check: dynamodb.number-precision-limited-to-38-digits.
AWS: 400 ValidationException, message `Attempting to store more than 38 significant digits in a Number` with NO `N validation error detected: ` prefix.
Emulator: `1 validation error detected: Attempting to store more than 38 significant digits in a Number` — adds a prefix AWS reserves for the Smithy constraint-validation family.

Cause 8 of 10 — an empty expression string. Failing check: dynamodb.empty-string-expression-rejected-across-operations.
AWS: 400 ValidationException, message `Invalid KeyConditionExpression: The expression can not be empty;` — fixed text with the expression member's name (`KeyConditionExpression`, `ConditionExpression`, ...) taken from the request.
Emulator: `Invalid KeyConditionExpression: Syntax error; token: "<EOF>", near: ""` — the parser's generic syntax error instead of the empty-expression rule.

Cause 9 of 10 — a defect in a TransactGetItems request item is answered with the wrong ERROR CLASS. Failing check: dynamodb.transact-get-items-top-level-vs-cancellation-error-class.
AWS: this defect is a per-item cancellation, so TransactGetItems answers 400 TransactionCanceledException with CancellationReasons naming the failing item; only top-level request defects answer ValidationException.
Emulator: answers `ValidationException` — `AssertionError: expected TransactionCanceledException, got 'ValidationException'`.

Cause 10 of 10 — TransactWriteItems ConditionExpression defects do not carry the same message PutItem produces. Failing check: dynamodb.transact-write-items-condition-expression-defects-match-putitem-message.
AWS: for the same ConditionExpression defect, PutItem's message and TransactWriteItems' message agree once TransactWriteItems' `N validation error detected: ` prefix is accounted for — i.e. PutItem answers `1 validation error detected: Invalid ConditionExpression: The expression can not be empty;` and TransactWriteItems answers `Invalid ConditionExpression: The expression can not be empty;`.
Emulator: TransactWriteItems already carries the `1 validation error detected: ` prefix itself, so prefixing it again does not equal the PutItem message — the two operations disagree on where the prefix belongs.

Why (all ten): a client that branches on the refusal message — the only machine-readable detail DynamoDB gives beyond ValidationException — takes the wrong branch, and the TransactGetItems case changes the error code itself, so a caller that handles TransactionCanceledException/CancellationReasons sees an unexpected ValidationException and cannot tell which request item was at fault. Messages that name resources AWS does not name (cause 5) or add a prefix AWS does not add (cause 7) also leak emulator-specific text into client logs and assertions.

Fix: DynamoDB's request-validation layer and its error serializer — the shared key/attribute-value validators used by PutItem/UpdateItem/GetItem/Query/Scan/BatchGetItem, the PartiQL key binder, the expression parser's empty-expression rule, and the TransactGetItems/TransactWriteItems error classification that decides between top-level ValidationException and per-item TransactionCanceledException.

Traces: see trace_ids.

## Evidence
Each line is expected (AWS) vs actual (emulator), taken from the replay:
1. duplicate-set-members-in-condition-key-and-transact-rejected: expected `One or more parameter values were invalid: Input collection ["a", "a"] contains duplicates.`, got `The provided key element does not match the schema`.
2. empty-and-duplicate-set-members-rejected-for-ss-ns-and-bs: expected `One or more parameter values were invalid: Input collection ["a", "a"] contains duplicates.`, got `The provided key element does not match the schema`.
3. execute-statement-parameter-count-and-key-type-mismatch-rejected: expected `Key attribute's data type should match its data type in table's schema: Key pk`, got `The provided key element does not match the schema`.
4. query-scan-exclusive-start-key-empty-value-rejected: expected `One or more parameter values are not valid. The AttributeValue for a key attribute cannot contain an empty binary value. Key: gpk`, got `The provided starting key is invalid`.
5. ddb-frc-update-item-setting-a-gsi-key-to-the-empty-string-rejected: expected ('ValidationException', `One or more parameter values are not valid. The update expression attempted to update a secondary index key to a value that is not supported. The AttributeValue for a key attribute cannot contain an empty string value.`), got ('ValidationException', `One or more parameter values are not valid. A value specified for a secondary index key is not supported. The AttributeValue for a key attribute cannot contain an empty string value. IndexName: by-tag, IndexKey: tag`).
6. ddb-gsi-query-on-numeric-index-key-with-a-string-value-rejected: expected `One or more parameter values were invalid: Condition parameter type does not match schema type`, got `One or more parameter values were invalid: Type mismatch for key CreationTime expected: N actual: S`.
7. number-precision-limited-to-38-digits: expected `Attempting to store more than 38 significant digits in a Number`, got `1 validation error detected: Attempting to store more than 38 significant digits in a Number`.
8. empty-string-expression-rejected-across-operations: expected `Invalid KeyConditionExpression: The expression can not be empty;`, got `Invalid KeyConditionExpression: Syntax error; token: "<EOF>", near: ""`.
9. transact-get-items-top-level-vs-cancellation-error-class: expected error code `TransactionCanceledException`, got `ValidationException`.
10. transact-write-items-condition-expression-defects-match-putitem-message: TransactWriteItems message `1 validation error detected: Invalid ConditionExpression: The expression can not be empty;` prefixed with `1 validation error detected: ` does not equal the PutItem message `1 validation error detected: Invalid ConditionExpression: The expression can not be empty;`.

## Steps to Reproduce
1. GetItem with a duplicated SS member in the key; 2. PutItem/key write with a duplicated SS/NS/BS member; 3. ExecuteStatement with a parameter whose type does not match the key's schema type; 4. Query/Scan with an ExclusiveStartKey whose key attribute is an empty B; 5. UpdateItem `SET tag = ''` on an item indexed by `tag`; 6. Query a GSI whose index hash key is N with an S operand; 7. PutItem with a Number of 39 significant digits; 8. Query with KeyConditionExpression=''; 9. TransactGetItems with a per-item defect; 10. the same empty ConditionExpression through PutItem and TransactWriteItems and compare messages.
Then re-replay the 10 checks in trace_ids.
