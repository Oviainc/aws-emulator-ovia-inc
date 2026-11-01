Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.deep-dynamodb-1-ddbvec-delete-unknown-vector-index-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.deep-dynamodb-1-ddbvec-delete-unknown-vector-index-rejected|run#11|AssertionError|assert err.get('Code') == 'ResourceNotFoundException', (
Issue-Commit: 578de6f0d5b839d3fe87385d733232b5a00d8be3

# dynamodb: UpdateTable VectorIndexUpdates Delete answers 501 NotImplemented, so deleting a vector index is impossible and the nonexistent-index refusal is never ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `578de6f0d5b839d3fe87385d733232b5a00d8be3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTable

## Description
One net-new root cause in the `dynamodb` emulator, grounded in a failing behavioral check that the replay report marked neither ALREADY FILED nor OWNED. The emulator is serving (219/452 checks pass, bootstrap_signal false, dynamodb 206/303), so this is a fidelity divergence, not a bootstrap gap. Model citations are from services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json.

=== Cause 1 of 1: UpdateTable's VectorIndexUpdates Delete branch is a 501 scaffold, so the Delete-of-a-nonexistent-vector-index refusal class (ResourceNotFoundException) is unreachable and a real vector index can never be removed ===

AWS: Smithy: UpdateTableInput.VectorIndexUpdates is an optional list (target VectorIndexUpdateList, member VectorIndexUpdate, whose members are Create -> CreateVectorIndexAction and Delete -> DeleteVectorIndexAction). DeleteVectorIndexAction.IndexName is @required (@length 3-255, @pattern ^[a-zA-Z0-9_.-]+$). UpdateTable's modeled error set is exactly InternalServerError (server, 500), InvalidEndpointException (client, 421), LimitExceededException (client, 400), ResourceInUseException (client, 400) and ResourceNotFoundException (client, 400) — there is no NotImplemented error and no 501 anywhere in the operation's closure. ResourceNotFoundException is documented as "The operation tried to access a nonexistent table or index". A Delete naming an index the table does not have is therefore refused with ResourceNotFoundException at HTTP 400, message template `Requested resource not found: Index <index-name> for table <table-name>` — the fixed text is `Requested resource not found: Index ` / ` for table `, and the two variable parts are the DeleteVectorIndexAction.IndexName taken from the request and the TableName taken from the request/service state. The refusal has no effect on state: every pre-existing vector index stays present and ACTIVE in DescribeTable's VectorIndexes. Note the deliberate asymmetry recorded by this check: a SearchVectors against a missing index is instead ValidationException with `The table does not have the specified index: <index-name>`, so an engine that routes both lookups through one shared "unknown index" path will get one of the two wrong. In the positive direction a Delete naming a real index succeeds, the table settles ACTIVE with exactly that index gone and its siblings still searchable.

Emulator: every UpdateTable carrying VectorIndexUpdates is refused with an unmodeled error — HTTP code `NotImplemented` and message `operation UpdateTable with VectorIndexUpdates is scaffolded but not implemented in this emulator` — regardless of whether the request's Delete names an existing index or a missing one. So neither the refusal class (ResourceNotFoundException/400) nor the success path (index actually removed) is reachable, and no distinction between an existing and a nonexistent index name is ever made.

Why: a client cannot remove a vector index from a table at all, so the vector-index lifecycle is one-way. On the negative path the client receives a server-class, unmodeled status where AWS returns a modeled 400 client error: SDK and client retry policies treat a 5xx as retryable and will loop on a request that production rejects permanently, and error-code branching on ResourceNotFoundException never fires. Because the handler cannot tell a missing index from a present one, a client also cannot use the refusal to probe which indexes exist, and the ValidationException-vs-ResourceNotFoundException distinction between SearchVectors and UpdateTable that AWS maintains is invisible here.

Fix: UpdateTable's VectorIndexUpdates branch and the vector-index state it mutates (the UpdateTable operation handler in the dynamodb service crate, plus the table/vector-index state it writes). At minimum, order the index-existence validation ahead of the not-implemented check so the Delete of an unknown index is refused with ResourceNotFoundException and AWS's message, and the refusal leaves every existing vector index ACTIVE; then implement the Delete action so a named real index is removed and its siblings keep answering SearchVectors.

Related already-open issue (named here because the replay report did NOT mark this check as OWNED by it, so its replay key is not carried anywhere): 20261027T001720Z-dynamodb-10-net-new-root-causes-searchvectors-ignores.md describes the same 501 scaffold as its Cause 5, but keyed to the Create side (two Creates in one call owing LimitExceededException) and grounded only in dynamodb.deep-dynamodb-1-ddbvec-two-index-updates-in-one-call-rejected. This issue carries the Delete side — the ResourceNotFoundException refusal class, its message, the effect-absence assertion, and actual index removal — which that issue's trace list does not cover.

Traces: dynamodb.deep-dynamodb-1-ddbvec-delete-unknown-vector-index-rejected.

## Evidence
Replayed suite simparity-behavioral-ga-prod-v1 against the booted emulator: 219/452 passed (48.5%), mismatched 122, errored 111; dynamodb 206/303 (68%). bootstrap_signal false, so the emulator is serving.

Cause 1 — dynamodb.deep-dynamodb-1-ddbvec-delete-unknown-vector-index-rejected [mismatch]:

  File "check_dynamodb_deep_dynamodb_1_ddbvec_delete_unknown_vector_index_rejected.py", line 75, in run
    assert err.get('Code') == 'ResourceNotFoundException', (
  AssertionError: expected ResourceNotFoundException, got {'Message': 'operation UpdateTable with VectorIndexUpdates is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

The failing call is:
  ddb.update_table(TableName=<table>, VectorIndexUpdates=[{'Delete': {'IndexName': 'idx-dropped'}}])
against a table created with VectorIndexes=[{'IndexName': 'idx-keep', 'VectorAttribute': {'AttributeName': 'emb'}, 'Dimensions': 4, 'DistanceFunction': 'EUCLIDEAN', 'Projection': {'ProjectionType': 'ALL'}}, {'IndexName': 'idx-drop', ... 'DistanceFunction': 'DOT_PRODUCT', ...}], i.e. 'idx-dropped' is a near-miss of the real 'idx-drop'.

Expected (AWS):   HTTP 400, Code 'ResourceNotFoundException', Message 'Requested resource not found: Index idx-dropped for table <table-name>' (fixed text 'Requested resource not found: Index ' + ' for table '; the index name comes from the request's DeleteVectorIndexAction.IndexName, the table name from the request's TableName).
Actual (emulator): Code 'NotImplemented' with Message 'operation UpdateTable with VectorIndexUpdates is scaffolded but not implemented in this emulator' — a status/code pair absent from UpdateTable's modeled error set (InternalServerError 500, InvalidEndpointException 421, LimitExceededException 400, ResourceInUseException 400, ResourceNotFoundException 400).

The check never reaches its later assertions, which are also unreachable while the branch is a stub: that after the refusal DescribeTable still reports [('idx-drop','ACTIVE'), ('idx-keep','ACTIVE')]; that a Delete of the real 'idx-drop' removes exactly that index and leaves 'idx-keep' answering SearchVectors; and that a SearchVectors on the deleted 'idx-drop' is then ValidationException with 'The table does not have the specified index: idx-drop'.

## Steps to Reproduce
1. Build and boot the emulator, then replay the check id in trace_ids (dynamodb.deep-dynamodb-1-ddbvec-delete-unknown-vector-index-rejected).
2. CreateTable with KeySchema [{pk, HASH}], BillingMode PAY_PER_REQUEST, and two vector indexes over attribute 'emb' with Dimensions 4 — 'idx-keep' (EUCLIDEAN) and 'idx-drop' (DOT_PRODUCT), both Projection ALL. Wait for TableStatus ACTIVE and both VectorIndexes IndexStatus ACTIVE, then PutItem an item carrying the embedding.
3. Call UpdateTable(TableName=<table>, VectorIndexUpdates=[{'Delete': {'IndexName': 'idx-dropped'}}]) — a name the table does not have.
   Observe: code 'NotImplemented' with 'operation UpdateTable with VectorIndexUpdates is scaffolded but not implemented in this emulator'.
   Expected: HTTP 400 ResourceNotFoundException, 'Requested resource not found: Index idx-dropped for table <table>'.
4. Then DescribeTable and confirm the refusal had no effect: VectorIndexes must still be [('idx-drop','ACTIVE'), ('idx-keep','ACTIVE')].
5. Positive control: UpdateTable(TableName=<table>, VectorIndexUpdates=[{'Delete': {'IndexName': 'idx-drop'}}]) must succeed; poll DescribeTable until VectorIndexes is ['idx-keep'] and TableStatus is ACTIVE; SearchVectors on 'idx-keep' must still return the item; SearchVectors on the deleted 'idx-drop' must then be ValidationException with 'The table does not have the specified index: idx-drop'.
