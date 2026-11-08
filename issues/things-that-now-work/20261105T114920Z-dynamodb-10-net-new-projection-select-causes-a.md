Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.ddb-wf-keys-only-gsi-query-withholds-status dynamodb.gsi-query-omits-attributes-the-projection-excludes dynamodb.ddbgsi-query-filter-on-a-non-projected-attribute-rejected-while-scan-matches-nothing dynamodb.ddb-wf-select-count-with-projection-expression-rejected dynamodb.select-non-specific-values-reject-projection-expression dynamodb.select-all-projected-attributes-requires-an-index-rejected dynamodb.gsi-query-attributes-to-get-validation dynamodb.projection-overlap-rejected-on-query-batch-and-transact-get dynamodb.projection-shared-list-index-merges-into-one-element dynamodb.query-key-condition-expression-third-predicate-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.ddb-wf-keys-only-gsi-query-withholds-status|run#13|AssertionError|assert code == ERR_VALIDATION, (
Issue-Key: dynamodb.gsi-query-omits-attributes-the-projection-excludes|run#23|AssertionError|assert named is None, (
Issue-Key: dynamodb.ddbgsi-query-filter-on-a-non-projected-attribute-rejected-while-scan-matches-nothing|_error_code#17|AssertionError|raise AssertionError(
Issue-Key: dynamodb.ddb-wf-select-count-with-projection-expression-rejected|run#9|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: dynamodb.select-non-specific-values-reject-projection-expression|run#12|AssertionError|assert msg == message, f'Query Select={select}: expected AWS message {message!r}, got {msg!r}'
Issue-Key: dynamodb.select-all-projected-attributes-requires-an-index-rejected|run#11|AssertionError|assert message == "1 validation error detected: ALL_PROJECTED_ATTRIBUTES can be used only when Querying using an IndexName", (
Issue-Key: dynamodb.gsi-query-attributes-to-get-validation|run#22|AssertionError|assert message == 'One or more parameter values were invalid: Duplicate value in attribute name: kept', (
Issue-Key: dynamodb.projection-overlap-rejected-on-query-batch-and-transact-get|run#23|AssertionError|assert (code, msg) == ("ValidationException", _OVERLAP_A_AB), (
Issue-Key: dynamodb.projection-shared-list-index-merges-into-one-element|run#8|AssertionError|assert merged == {"rows": {"L": [{"M": {"a": {"S": "a0"}, "b": {"S": "b0"}}}]}}, (
Issue-Key: dynamodb.query-key-condition-expression-third-predicate-rejected|run#15|AssertionError|assert message2 == expected2, f"expected message {expected2!r}, got {message2!r}"
Issue-Commit: 9fc0802c1fad78cd4c44ae6731253718bd889678

# dynamodb: 10 net-new projection/Select causes — a ProjectionExpression naming a non-projected or non-key attribute is accepted instead of refused, and every Select/ProjectionExpression refusal message and the shared-list-index response shape diverge

> _Note: this issue was identified against the code base at commit `9fc0802c1fad78cd4c44ae6731253718bd889678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Query

## Description
Ten distinct causes in DynamoDB's projection and Select handling. Three are missing refusals (the emulator accepts a request AWS rejects), six are wrong refusal messages, and one is a wrong response shape.

AWS grounding used throughout — Smithy (dynamodb, QueryInput.Select) documents: `ALL_PROJECTED_ATTRIBUTES` is "Allowed only when querying an index"; `SPECIFIC_ATTRIBUTES` "Returns only the attributes listed in ProjectionExpression"; "If you query or scan a global secondary index, you can only request attributes that are projected into the index. Global secondary index queries cannot fetch attributes from the parent table."; and "If you use the ProjectionExpression parameter, then the value for Select can only be SPECIFIC_ATTRIBUTES. Any other value for Select will return an error."

Cause 1 of 10 — a ProjectionExpression naming a non-projected attribute of a KEYS_ONLY GSI is accepted. Failing check: dynamodb.ddb-wf-keys-only-gsi-query-withholds-status.
AWS: 400 ValidationException (a GSI query cannot fetch an attribute not projected into the index).
Emulator: the Query succeeds and no error is produced at all — `a ProjectionExpression naming 'status' against the KEYS_ONLY index WorkflowKeysOnly must be refused with ValidationException, got None: None`.

Cause 2 of 10 — same rule for an INCLUDE-projection GSI. Failing check: dynamodb.gsi-query-omits-attributes-the-projection-excludes.
AWS: 400 ValidationException.
Emulator: the Query is accepted and returns the projected subset — `a ProjectionExpression naming a non-projected attribute must be refused, but the query was accepted and returned [{'pk': {'S': 'CUSTOMER#1'}}]`.

Cause 3 of 10 — a FilterExpression on a non-projected GSI attribute is accepted. Failing check: dynamodb.ddbgsi-query-filter-on-a-non-projected-attribute-rejected-while-scan-matches-nothing.
AWS: 400 ValidationException for the Query (the attribute is not in the index), while the equivalent Scan of the index is legal and simply matches nothing.
Emulator: the Query succeeds and returns `[]` — `real AWS refuses this Query, but it SUCCEEDED and returned []`.

Cause 4 of 10 — Select=COUNT together with a ProjectionExpression. Failing check: dynamodb.ddb-wf-select-count-with-projection-expression-rejected.
AWS: 400 ValidationException, message `1 validation error detected: Cannot specify the ProjectionExpression when choosing to get only the Count` (entirely fixed text).
Emulator: `One or more parameter values were invalid: Select type COUNT is not supported when the attributes to return are listed; the only Select type that is, is SPECIFIC_ATTRIBUTES`.

Cause 5 of 10 — Select=ALL_ATTRIBUTES (and the other non-SPECIFIC_ATTRIBUTES values) together with a ProjectionExpression. Failing check: dynamodb.select-non-specific-values-reject-projection-expression.
AWS: 400 ValidationException, message `1 validation error detected: Cannot specify the ProjectionExpression when choosing to get ALL_ATTRIBUTES` — fixed text with the offending Select value from the request.
Emulator: `One or more parameter values were invalid: Select type ALL_ATTRIBUTES is not supported when the attributes to return are listed; the only Select type that is, is SPECIFIC_ATTRIBUTES`.

Cause 6 of 10 — Select=ALL_PROJECTED_ATTRIBUTES without an IndexName. Failing check: dynamodb.select-all-projected-attributes-requires-an-index-rejected.
AWS: 400 ValidationException, message `1 validation error detected: ALL_PROJECTED_ATTRIBUTES can be used only when Querying using an IndexName` (entirely fixed text).
Emulator: `One or more parameter values were invalid: Select type ALL_PROJECTED_ATTRIBUTES is allowed only when an index is scanned`.

Cause 7 of 10 — a duplicated name in the legacy AttributesToGet. Failing check: dynamodb.gsi-query-attributes-to-get-validation.
AWS: 400 ValidationException, message `One or more parameter values were invalid: Duplicate value in attribute name: kept` — fixed text `One or more parameter values were invalid: Duplicate value in attribute name: ` plus the duplicated name from the request.
Emulator: answers the ProjectionExpression overlap message instead — `Invalid AttributesToGet: Two document paths overlap with each other; must remove or rewrite one of these paths; path one: [kept], path two: [kept]`.

Cause 8 of 10 — overlapping ProjectionExpression paths are refused with the wrong precedence. Failing check: dynamodb.projection-overlap-rejected-on-query-batch-and-transact-get.
AWS: the overlap is reported first — 400 ValidationException with the overlap message naming the two paths (`Invalid ProjectionExpression: Two document paths overlap with each other; must remove or rewrite one of these paths; path one: [a], path two: [a, b]`), even when an unused ExpressionAttributeNames entry is also present.
Emulator: reports the unused-name defect instead — `'ValidationException': 'Value provided in ExpressionAttributeNames unused in expressions: keys: {#z}'`.

Cause 9 of 10 — two ProjectionExpression paths through the SAME list index are returned as two separate list elements. Failing check: dynamodb.projection-shared-list-index-merges-into-one-element.
AWS: paths that share a list index merge into ONE response element carrying both leaves, and the unprojected sibling is withheld: `{"rows": {"L": [{"M": {"a": {"S": "a0"}, "b": {"S": "b0"}}}]}}`.
Emulator: returns `{'rows': {'L': [{'M': {'a': {'S': 'a0'}}}, {'M': {'b': {'S': 'b0'}}}]}}` — one element per path, changing the list's length and the index of every element.

Cause 10 of 10 — a KeyConditionExpression with more predicates than the schema has key attributes. Failing check: dynamodb.query-key-condition-expression-third-predicate-rejected.
AWS: 400 ValidationException, message `The number of query conditions (3) exceeds the number of key attributes defined in the schema (2)` — fixed text around the two counts, taken from the request and the table's key schema.
Emulator: `Query key condition not supported`.

Why: causes 1-3 are the most serious — a client's GSI query that AWS refuses is accepted here and silently returns a wrong (empty or truncated) result set, so a bug that fails in production passes in the emulator. Cause 9 changes the shape and indices of a returned list, which a client reading `rows[0]` will mis-parse. The six message causes send a client that branches on the refusal text down the wrong path, and cause 8 reports an entirely different defect than the one AWS reports first.

Fix: DynamoDB's projection/Select validation and projection evaluation — the Query/Scan/GetItem/BatchGetItem/TransactGetItems validators that check Select against ProjectionExpression and against IndexName, the GSI projection check that must refuse attributes not projected into the index, the ProjectionExpression overlap/unused-name precedence, the legacy AttributesToGet validator, the key-condition arity check, and the projection serializer that merges paths sharing a list index.

Traces: see trace_ids.

## Evidence
1. ddb-wf-keys-only-gsi-query-withholds-status: expected 400 ValidationException; got no error — `got None: None`.
2. gsi-query-omits-attributes-the-projection-excludes: expected 400 ValidationException; got 200 with `[{'pk': {'S': 'CUSTOMER#1'}}]`.
3. ddbgsi-query-filter-on-a-non-projected-attribute-rejected-while-scan-matches-nothing: expected 400 ValidationException for the Query; got 200 with `[]`.
4. ddb-wf-select-count-with-projection-expression-rejected: expected `1 validation error detected: Cannot specify the ProjectionExpression when choosing to get only the Count`, got `One or more parameter values were invalid: Select type COUNT is not supported when the attributes to return are listed; the only Select type that is, is SPECIFIC_ATTRIBUTES`.
5. select-non-specific-values-reject-projection-expression: expected `1 validation error detected: Cannot specify the ProjectionExpression when choosing to get ALL_ATTRIBUTES`, got `One or more parameter values were invalid: Select type ALL_ATTRIBUTES is not supported when the attributes to return are listed; the only Select type that is, is SPECIFIC_ATTRIBUTES`.
6. select-all-projected-attributes-requires-an-index-rejected: expected `1 validation error detected: ALL_PROJECTED_ATTRIBUTES can be used only when Querying using an IndexName`, got `One or more parameter values were invalid: Select type ALL_PROJECTED_ATTRIBUTES is allowed only when an index is scanned`.
7. gsi-query-attributes-to-get-validation: expected `One or more parameter values were invalid: Duplicate value in attribute name: kept`, got `Invalid AttributesToGet: Two document paths overlap with each other; must remove or rewrite one of these paths; path one: [kept], path two: [kept]`.
8. projection-overlap-rejected-on-query-batch-and-transact-get: expected the overlap ValidationException; got `'ValidationException': 'Value provided in ExpressionAttributeNames unused in expressions: keys: {#z}'`.
9. projection-shared-list-index-merges-into-one-element: expected `{"rows": {"L": [{"M": {"a": {"S": "a0"}, "b": {"S": "b0"}}}]}}`, got `{'rows': {'L': [{'M': {'a': {'S': 'a0'}}}, {'M': {'b': {'S': 'b0'}}}]}}`.
10. query-key-condition-expression-third-predicate-rejected: expected `The number of query conditions (3) exceeds the number of key attributes defined in the schema (2)`, got `Query key condition not supported`.

## Steps to Reproduce
1-3. Create a table with a KEYS_ONLY and an INCLUDE GSI, then Query the index with a ProjectionExpression / FilterExpression naming an attribute the index does not project — observe success instead of 400 ValidationException.
4-6. Query with Select=COUNT + ProjectionExpression, Select=ALL_ATTRIBUTES + ProjectionExpression, and Select=ALL_PROJECTED_ATTRIBUTES without IndexName — compare messages.
7. Query with AttributesToGet=['kept','kept'].
8. GetItem with ProjectionExpression='a, a.b' and an unused ExpressionAttributeNames entry '#z'.
9. GetItem with ProjectionExpression='rows[0].a, rows[0].b' on an item whose rows[0] also has 'c'.
10. Query a 2-key table with a KeyConditionExpression carrying 3 predicates.
Then re-replay the 10 checks in trace_ids.
