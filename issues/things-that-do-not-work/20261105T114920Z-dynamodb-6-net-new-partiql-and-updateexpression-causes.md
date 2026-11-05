Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.batch-execute-statement-returning-update-vs-delete-rejected dynamodb.execute-statement-quoting-rules dynamodb.execute-statement-undefined-function-and-projection-function-rejected dynamodb.partiql-from-clause-path-arity-rejected dynamodb.partiql-unknown-index-qualifier-rejected dynamodb.deep-dynamodb-2-set-list-index-out-of-bounds-appends
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.batch-execute-statement-returning-update-vs-delete-rejected|run#27|AssertionError|assert error.get('Message') == _EXPECTED_DELETE_MESSAGE, (
Issue-Key: dynamodb.execute-statement-quoting-rules|run#17|AssertionError|assert message == expected_b, f"unquoted reserved attribute: expected message {expected_b!r}, got {message!r}"
Issue-Key: dynamodb.execute-statement-undefined-function-and-projection-function-rejected|run#23|AssertionError|assert message == expected_projection_message, (
Issue-Key: dynamodb.partiql-from-clause-path-arity-rejected|run#16|AssertionError|assert message == expected_empty_msg, f"empty-index SELECT: unexpected message {message!r}"
Issue-Key: dynamodb.partiql-unknown-index-qualifier-rejected|run#17|AssertionError|assert err.get('Message') == expected, f"expected message {expected!r}, got {err.get('Message')!r}"
Issue-Key: dynamodb.deep-dynamodb-2-set-list-index-out-of-bounds-appends|run#15|AssertionError|assert stored == ['a', 'b', 'c', 'd', 'e', 'Y', 'X'], (
Issue-Commit: 9fc0802c1fad78cd4c44ae6731253718bd889678

# dynamodb: 6 net-new PartiQL and UpdateExpression causes — RETURNING, quoting, projection-function, FROM-clause path and unknown-index refusal messages diverge, and out-of-bounds list SET appends are ordered by clause instead of by index

> _Note: this issue was identified against the code base at commit `9fc0802c1fad78cd4c44ae6731253718bd889678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Six distinct causes: five PartiQL refusal messages that are not AWS's, and one UpdateExpression evaluation defect.

Cause 1 of 6 — a DELETE statement with an illegal RETURNING clause. Failing check: dynamodb.batch-execute-statement-returning-update-vs-delete-rejected.
AWS: 400 ValidationException, message `Invalid returning clause: RETURNING ALL NEW *. Only RETURNING ALL OLD * is allowed in DELETE statements.` — fixed text `Invalid returning clause: RETURNING ` + ` . Only RETURNING ALL OLD * is allowed in DELETE statements.` with the clause the caller wrote echoed back.
Emulator: `Invalid Statement: a DELETE can only return the item as it stood before it: ALL OLD or MODIFIED OLD` — emulator-authored prose that neither echoes the caller's clause nor matches AWS's wording.

Cause 2 of 6 — an unquoted reserved word used as an attribute name. Failing check: dynamodb.execute-statement-quoting-rules.
AWS: 400 ValidationException, message `Statement wasn't well formed, can't be processed: Unexpected keyword` (entirely fixed text).
Emulator: `Statement wasn't well formed, can't be processed: Unexpected term`.

Cause 3 of 6 — a function call used in the projection (SELECT) list. Failing check: dynamodb.execute-statement-undefined-function-and-projection-function-rejected.
AWS: 400 ValidationException, message `Unexpected path component at 1:8:16` — fixed text `Unexpected path component at ` plus the statement position (line:column:offset) of the offending component.
Emulator: `Statement wasn't well formed, can't be processed: Expected identifier for simple path` — a different message family that carries no position at all.

Cause 4 of 6 — a FROM clause whose path arity is wrong (e.g. an empty index qualifier). Failing check: dynamodb.partiql-from-clause-path-arity-rejected.
AWS: 400 ValidationException refusing the path itself — the empty-index SELECT is a malformed path, not a missing index.
Emulator: `The table does not have the specified index` — resolves the path and reports a lookup failure instead of refusing the arity.

Cause 5 of 6 — an empty-string path component in an index qualifier. Failing check: dynamodb.partiql-unknown-index-qualifier-rejected.
AWS: 400 ValidationException, message `Path component cannot be an empty string` (entirely fixed text).
Emulator: `The table does not have the specified index`.

Cause 6 of 6 — out-of-bounds list indices in a SET UpdateExpression append in clause order instead of index order. Failing check: dynamodb.deep-dynamodb-2-set-list-index-out-of-bounds-appends.
AWS: `SET l[99] = :x, l[98] = :y` on a 5-element list appends both values at the end ordered by their INDEX, not by the order the clauses appear: the stored list becomes [a, b, c, d, e, Y, X].
Emulator: stores [a, b, c, d, e, X, Y] — appended in clause order.

Why: causes 1-5 send a client that branches on the PartiQL refusal message down the wrong path, and causes 4-5 additionally misclassify a malformed statement as a missing-index condition, so a caller retries against a different index instead of fixing the statement. Cause 6 silently stores a different list than AWS would, so every subsequent read of that list by position is wrong.

Fix: the PartiQL statement parser/validator and its error serializer (the ExecuteStatement/BatchExecuteStatement/ExecuteTransaction validation path: RETURNING-clause check, reserved-word quoting, projection-path parsing with position reporting, FROM-clause path arity and empty path components), and the UpdateExpression evaluator's out-of-bounds list-index append ordering in the UpdateItem handler.

Traces: see trace_ids.

## Evidence
1. batch-execute-statement-returning-update-vs-delete-rejected: expected `Invalid returning clause: RETURNING ALL NEW *. Only RETURNING ALL OLD * is allowed in DELETE statements.`, got `Invalid Statement: a DELETE can only return the item as it stood before it: ALL OLD or MODIFIED OLD`.
2. execute-statement-quoting-rules: expected `Statement wasn't well formed, can't be processed: Unexpected keyword`, got `Statement wasn't well formed, can't be processed: Unexpected term`.
3. execute-statement-undefined-function-and-projection-function-rejected: expected `Unexpected path component at 1:8:16`, got `Statement wasn't well formed, can't be processed: Expected identifier for simple path`.
4. partiql-from-clause-path-arity-rejected: `AssertionError: empty-index SELECT: unexpected message 'The table does not have the specified index'`.
5. partiql-unknown-index-qualifier-rejected: expected `Path component cannot be an empty string`, got `The table does not have the specified index`.
6. deep-dynamodb-2-set-list-index-out-of-bounds-appends: expected stored list [a, b, c, d, e, Y, X] (appends ordered by index, not by clause order), got ['a', 'b', 'c', 'd', 'e', 'X', 'Y'].

## Steps to Reproduce
1. BatchExecuteStatement/ExecuteStatement with `DELETE FROM "t" WHERE pk='a' RETURNING ALL NEW *`.
2. ExecuteStatement selecting an unquoted reserved word as an attribute name.
3. ExecuteStatement with a function call in the SELECT list.
4. ExecuteStatement with `SELECT * FROM "t".""`.
5. ExecuteStatement with an empty-string index qualifier in the FROM path.
6. PutItem a 5-element list `l`, then UpdateItem `SET l[99] = :x, l[98] = :y` and read the list back.
Then re-replay the 6 checks in trace_ids.
