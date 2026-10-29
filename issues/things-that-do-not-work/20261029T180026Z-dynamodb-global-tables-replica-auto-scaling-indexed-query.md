Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.autoscaling-and-kinesis-destination-defaults dynamodb.replica-autoscaling-not-found dynamodb.concurrent-global-table-replica-update-consistency dynamodb.global-table-already-exists dynamodb.gsi-query-consistent-read-rejected dynamodb.deep-dynamodb-1-ddbvec-condition-on-non-schema-attribute-rejected dynamodb.deep-dynamodb-1-ddbvec-consumed-capacity-reports-vector-bytes dynamodb.deep-dynamodb-1-ddbvec-topk-above-100-rejected
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.autoscaling-and-kinesis-destination-defaults|run#5|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: dynamodb.replica-autoscaling-not-found|run#4|AssertionError|assert describe_code == "ResourceNotFoundException", (
Issue-Key: dynamodb.concurrent-global-table-replica-update-consistency|run#16|AssertionError|assert all(c in allowed for c in codes), outcomes
Issue-Key: dynamodb.global-table-already-exists|run#7|AssertionError|assert first_code == 'ValidationException', f'expected ValidationException, got {first_code}'
Issue-Key: dynamodb.gsi-query-consistent-read-rejected|run#7|AssertionError|assert err.get('Code') == 'ValidationException', f"expected ValidationException, got {err.get('Code')}"
Issue-Key: dynamodb.deep-dynamodb-1-ddbvec-condition-on-non-schema-attribute-rejected|run#17|AssertionError|assert err.get('Message') == expected_msg, (
Issue-Key: dynamodb.deep-dynamodb-1-ddbvec-consumed-capacity-reports-vector-bytes|run#11|AssertionError|assert set(per_index) == {'idx-cos', 'idx-dot'}, (
Issue-Key: dynamodb.deep-dynamodb-1-ddbvec-topk-above-100-rejected|_error#11|AssertionError|raise AssertionError(f'{what}: the call was ACCEPTED; real AWS rejects it')
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# DynamoDB global tables, replica auto scaling, indexed Query and vector indexes answer 501 NotImplemented where AWS answers a modeled error (8 causes)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTable

## Description
Eight net-new root causes in the `dynamodb` service. The common shape of the first five is that a call AWS answers with a modeled 400-class error instead gets HTTP 501 `NotImplemented` with an emulator-authored message of the form `operation <Op> with <Member> is scaffolded but not implemented in this emulator` — a status and code that exist nowhere in the DynamoDB model, so no SDK error class matches and no retry/branch logic fires. Each cause needs its own fix. (Sibling checks of several of these are carried by the open issues 20261025T024002Z-dynamodb-10-net-new-root-causes-query-scan.md and 20261027T001720Z-dynamodb-10-net-new-root-causes-searchvectors-ignores.md; those issues do not carry the replay keys listed here.)

Cause 1 of 8: replica auto-scaling on a non-replicated table.
AWS: `ResourceNotFoundException` (HTTP 400). Smithy: DescribeTableReplicaAutoScaling models exactly InternalServerError and ResourceNotFoundException — there is no NotImplemented.
Emulator: `NotImplemented`.
Why: a caller probing for replica auto-scaling cannot distinguish "no replicas" from a broken service.
Fix: the replica-auto-scaling handlers (DescribeTableReplicaAutoScaling / UpdateTableReplicaAutoScaling) and their state lookup.
Traces: dynamodb.autoscaling-and-kinesis-destination-defaults

Cause 2 of 8: replica auto-scaling for an absent table/replica.
AWS: `ResourceNotFoundException`.
Emulator: `NotImplemented`.
Why: same as cause 1, on the explicit not-found path.
Fix: same handlers' not-found path.
Traces: dynamodb.replica-autoscaling-not-found

Cause 3 of 8: concurrent UpdateTable ReplicaUpdates.
AWS: ReplicaUpdates is a modeled UpdateTable member (Smithy: UpdateTable.ReplicaUpdates -> ReplicationGroupUpdateList, @length min 1) and the operation either applies it or refuses with LimitExceededException/ResourceInUseException/ResourceNotFoundException/ValidationException.
Emulator: every one of four concurrent calls answered `NotImplemented` `operation UpdateTable with ReplicaUpdates is scaffolded but not implemented in this emulator`.
Why: global-table replica management is unusable and the concurrency contract is unobservable.
Fix: UpdateTable handler's ReplicaUpdates branch and replica state.
Traces: dynamodb.concurrent-global-table-replica-update-consistency

Cause 4 of 8: creating a global table that already exists.
AWS: `ValidationException`.
Emulator: `NotImplemented`.
Why: the already-exists branch a caller must handle is unreachable.
Fix: the global-table handler's already-exists check.
Traces: dynamodb.global-table-already-exists

Cause 5 of 8: Query on a GSI with ConsistentRead=true.
AWS: a strongly consistent read of a global secondary index is refused with `ValidationException`.
Emulator: `NotImplemented` (`operation Query with IndexName is scaffolded but not implemented in this emulator`), so the documented refusal never happens.
Why: a caller that wrongly asks for consistent GSI reads gets a 501 instead of the 400 that tells it the request is invalid.
Fix: Query handler's IndexName branch (and its ConsistentRead validation).
Traces: dynamodb.gsi-query-consistent-read-rejected

Cause 6 of 8: SearchVectors condition on a non-schema attribute — wrong message.
AWS: `SearchConditionExpression must not contain any attributes that is not in SearchSchema. Invalid attribute: <attribute name>` — the frame is fixed and the attribute name comes from the request.
Emulator: `Only top-level attributes from the vector index search schema can be referenced in a SearchConditionExpression` — invented prose that also drops the offending attribute.
Why: the caller is not told which attribute is invalid.
Fix: SearchVectors SearchConditionExpression validation message.
Traces: dynamodb.deep-dynamodb-1-ddbvec-condition-on-non-schema-attribute-rejected

Cause 7 of 8: a vector write is not metered per vector index.
AWS: ConsumedCapacity for a write that touches a vector index reports a per-index breakdown under VectorIndexes sized by the vector bytes.
Emulator: `VectorIndexes` is `{}` — the write is metered as if no index existed.
Why: vector-index cost is invisible, so capacity planning and cost attribution are wrong.
Fix: the write path's ConsumedCapacity construction (vector-index arm).
Traces: dynamodb.deep-dynamodb-1-ddbvec-consumed-capacity-reports-vector-bytes

Cause 8 of 8: SearchVectors accepts TopK above the maximum.
AWS: TopK above 100 is refused with `ValidationException`.
Emulator: the call is ACCEPTED.
Why: an out-of-range request silently returns results no real account would return, and the negative path is untested.
Fix: SearchVectors input validation (TopK range).
Traces: dynamodb.deep-dynamodb-1-ddbvec-topk-above-100-rejected

## Evidence
Replay details, one per cause:
- autoscaling-and-kinesis-destination-defaults: AssertionError: expected ResourceNotFoundException for replica auto-scaling on a non-replicated table, got NotImplemented.
- replica-autoscaling-not-found: AssertionError: expected ResourceNotFoundException, got NotImplemented.
- concurrent-global-table-replica-update-consistency: AssertionError: [('NotImplemented', "... when calling the UpdateTable operation: operation UpdateTable with ReplicaUpdates is scaffolded but not implemented in this emulator") x4].
- global-table-already-exists: AssertionError: expected ValidationException, got NotImplemented.
- gsi-query-consistent-read-rejected: AssertionError: expected ValidationException, got NotImplemented; the underlying refusal is 'operation Query with IndexName is scaffolded but not implemented in this emulator'.
- deep-dynamodb-1-ddbvec-condition-on-non-schema-attribute-rejected: expected message 'SearchConditionExpression must not contain any attributes that is not in SearchSchema. Invalid attribute: note', got 'Only top-level attributes from the vector index search schema can be referenced in a SearchConditionExpression'.
- deep-dynamodb-1-ddbvec-consumed-capacity-reports-vector-bytes: AssertionError: a vector write must meter every index over the attribute; VectorIndexes was {}.
- deep-dynamodb-1-ddbvec-topk-above-100-rejected: AssertionError: SearchVectors with TopK=101: the call was ACCEPTED; real AWS rejects it.

Smithy grounding: DescribeTableReplicaAutoScaling errors = {InternalServerError, ResourceNotFoundException}; UpdateTable errors = {InternalServerError, InvalidEndpointException, LimitExceededException, ResourceInUseException, ResourceNotFoundException} (+ service-common ValidationException); UpdateTable.ReplicaUpdates and UpdateTable.VectorIndexUpdates are modeled members; Query.IndexName -> IndexName @length 3..255 pattern ^[a-zA-Z0-9_.-]+$. None of these operations model a NotImplemented error or a 501 status.

## Steps to Reproduce
Boot the emulator and replay: dynamodb.autoscaling-and-kinesis-destination-defaults, dynamodb.replica-autoscaling-not-found, dynamodb.concurrent-global-table-replica-update-consistency, dynamodb.global-table-already-exists, dynamodb.gsi-query-consistent-read-rejected, dynamodb.deep-dynamodb-1-ddbvec-condition-on-non-schema-attribute-rejected, dynamodb.deep-dynamodb-1-ddbvec-consumed-capacity-reports-vector-bytes, dynamodb.deep-dynamodb-1-ddbvec-topk-above-100-rejected.
