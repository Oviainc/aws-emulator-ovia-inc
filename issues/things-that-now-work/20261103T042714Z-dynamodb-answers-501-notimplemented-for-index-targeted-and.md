Issue-Severity: HIGH
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# DynamoDB answers 501 NotImplemented for index-targeted and projection-carrying reads that AWS refuses with ValidationException, so the modeled refusal is never served (1 cause)

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Query

## Description
One net-new root cause. Open issue 20261103T022510Z-dynamodb-index-targeted-and-projection-carrying-query-scan.md and 20261025T024002Z-dynamodb-10-net-new-root-causes-query-scan.md describe the same surface but the suite did not mark any of the checks below as owned by them, so the replay keys here are not covered.

=== Cause 1 of 1: the scaffold feature-gate is evaluated BEFORE request validation ===
AWS: Query, Scan, BatchGetItem and TransactGetItems validate the request first. A Query naming an index the table does not have is ValidationException "The table does not have the specified index: <IndexName>"; a key condition that omits the index partition key, or supplies a value whose type does not match the index key's declared AttributeType, is ValidationException; ConsistentRead=true against a global secondary index is ValidationException (an LSI accepts it); Select=COUNT or Select=ALL_ATTRIBUTES together with a ProjectionExpression is ValidationException; an unaliased reserved word in a ProjectionExpression is ValidationException; and overlapping document paths in a projection are ValidationException. All of these are client errors at HTTP 400. Smithy models IndexName on Query/Scan with @length 3..255 and pattern ^[a-zA-Z0-9_.-]+$ and models ResourceNotFoundException/ProvisionedThroughputExceededException but no server-side "unimplemented" error — NotImplemented is not in the operation's error set at all.
Emulator: the handlers short-circuit on an unimplemented-feature guard before any of that validation runs, so every one of these requests answers HTTP 501 with Code NotImplemented and a message of the form "operation <Op> with <Feature> is scaffolded but not implemented in this emulator" (observed for Query with IndexName, Query with ProjectionExpression, BatchGetItem with ProjectionExpression, Scan with a ProjectionExpression path into an attribute).
Why: 501/NotImplemented is not a client error. botocore and every AWS SDK treat a 5xx as retryable and as a service fault, so a caller that sent a genuinely invalid request retries it, then surfaces a server error — it never learns that its IndexName is wrong, that its ConsistentRead is illegal on a GSI, or that its ProjectionExpression conflicts with Select. Negative-path code written against AWS (catch ValidationException) does not fire at all. It also hides the gap: the request looks like an emulator limitation even when it is a request AWS would always refuse.
Fix: move request validation ahead of the feature-gate in the Query, Scan, BatchGetItem and TransactGetItems handlers, so a request AWS refuses is refused with its modeled ValidationException (HTTP 400) regardless of whether the index/projection execution path is implemented.
Traces: ddb-gsi-query-naming-an-absent-index-rejected, ddb-gsi-query-omitting-the-index-partition-key-rejected, ddb-gsi-query-on-numeric-index-key-with-a-string-value-rejected, query-capacity-gsi-rejects-consistent-read-lsi-accepts-it, select-non-specific-values-reject-projection-expression, ddb-wf-select-count-with-projection-expression-rejected, ddb-wf-unaliased-reserved-word-projection-rejected, query-and-put-item-placeholder-and-reserved-word-validation, batch-get-item-expression-and-legacy-parameter-rules, transact-get-items-top-level-vs-cancellation-error-class, ddb-ss-aliased-dotted-attribute-name-projects-the-top-level-attribute.

## Evidence
ddb-gsi-query-naming-an-absent-index-rejected: AssertionError: a query against an absent index raised ('NotImplemented', 'operation Query with IndexName is scaffolded but not implemented in this emulator'), expected ('ValidationException', 'The table does not have the specified index: TaskStatus').
ddb-gsi-query-omitting-the-index-partition-key-rejected: a sort-key-only condition on an index raised 'NotImplemented', expected 'ValidationException'.
ddb-gsi-query-on-numeric-index-key-with-a-string-value-rejected: a string value for the numeric index key raised 'NotImplemented', expected 'ValidationException'.
query-capacity-gsi-rejects-consistent-read-lsi-accepts-it: a strongly consistent Query against a GSI must be refused with ValidationException; got NotImplemented.
select-non-specific-values-reject-projection-expression: Query Select=ALL_ATTRIBUTES with a ProjectionExpression must be refused with ValidationException; got NotImplemented.
ddb-wf-select-count-with-projection-expression-rejected: Select=COUNT with a ProjectionExpression on Query raised 'NotImplemented', expected 'ValidationException'.
ddb-wf-unaliased-reserved-word-projection-rejected: an unaliased reserved word in BatchGetItem's ProjectionExpression raised 'NotImplemented', expected 'ValidationException'.
query-and-put-item-placeholder-and-reserved-word-validation: zero-match Query projecting a.#u undefined gave 'NotImplemented': 'operation Query with ProjectionExpression is scaffolded but not implemented in this emulator'.
batch-get-item-expression-and-legacy-parameter-rules: BatchGetItem with ProjectionExpression with AttributesToGet on one table gave 'NotImplemented'.
transact-get-items-top-level-vs-cancellation-error-class: expected ValidationException, got 'NotImplemented'.
ddb-ss-aliased-dotted-attribute-name-projects-the-top-level-attribute: ClientError (NotImplemented) 'operation Scan with a ProjectionExpression path into an attribute is scaffolded but not implemented in this emulator'.
All of these are HTTP 501 (see the sibling observation 'HTTPStatusCode': 501 recorded for the same guard).

## Steps to Reproduce
1. Create a table with HASH pk (S) / RANGE sk (S) and a GSI 'gsi1' with HASH gpk (S); wait for ACTIVE.
2. ddb.query(TableName=t, IndexName='no-such-index', KeyConditionExpression='gpk = :g', ExpressionAttributeValues={':g': {'S': 'x'}}) — expect ValidationException 'The table does not have the specified index: no-such-index' at HTTP 400; observe NotImplemented at HTTP 501.
3. ddb.query(TableName=t, IndexName='gsi1', ConsistentRead=True, KeyConditionExpression='gpk = :g', ...) — expect ValidationException; observe NotImplemented.
4. ddb.query(TableName=t, Select='COUNT', ProjectionExpression='pk', KeyConditionExpression='pk = :p', ...) — expect ValidationException; observe NotImplemented.
5. ddb.batch_get_item(RequestItems={t: {'Keys': [...], 'ProjectionExpression': 'status'}}) — expect ValidationException for the unaliased reserved word; observe NotImplemented.
Then replay the dynamodb checks named in trace_ids.
