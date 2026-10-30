Issue-Source: trace-sweep-divergence
Issue-Trace: s3vectors-bucket-and-index-lifecycle
Issue-Step: teardown#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3vectors-bucket-and-index-lifecycle s3vectors-create-index-duplicate-and-not-found-errors s3vectors-create-index-errors s3vectors-create-index-nonexistent-bucket-error s3vectors-error-invalid-dimension-validation s3vectors-error-notfound-operations s3vectors-get-and-delete-index-by-arn s3vectors-index-lifecycle-basic s3vectors-index-lifecycle-by-arn s3vectors-index-referenced-by-arn s3vectors-index-with-metadata-and-encryption-config s3vectors-index-with-metadata-configuration s3vectors-metadata-and-encryption-config s3vectors-multiple-indexes-isolation s3vectors-not-found-errors-on-missing-bucket-and-index s3vectors-tag-resource-nonexistent-index s3vectors-error-duplicate-index-create s3vectors-duplicate-bucket-and-invalid-put-errors s3vectors-error-duplicate-bucket-creation s3vectors-vector-bucket-duplicate-and-missing s3vectors-vector-bucket-duplicate-create-conflict
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors|GetIndex|4xx->4xx|$.message
Issue-Key: s3vectors|DeleteIndex|4xx->4xx|$.message
Issue-Key: s3vectors|ListTagsForResource|4xx->4xx|$.message
Issue-Key: s3vectors|TagResource|4xx->4xx|$.message
Issue-Key: s3vectors|CreateIndex|4xx->4xx|$.message
Issue-Key: s3vectors|CreateVectorBucket|4xx->4xx|$.message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# S3 Vectors not-found and already-exists refusals use emulator wording instead of the service's sentences

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIndex

## Description
Six independent causes, all in the s3vectors emulator's shared refusal wording: the HTTP status and the error code are right on every one of them, but the `message` member the client reads is this emulator's own sentence rather than the sentence S3 Vectors answers. Each cause is numbered below and carries its own fixture/step; all six must be closed before this issue is submitted.

=== Cause 1 of 6: GetIndex's NotFoundException says 'vector index' where the service says 'index' ===
AWS: Smithy (services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/com-amazonaws-s3vectors-s3vectors.json) models NotFoundException on GetIndex with httpError 404 and a REQUIRED `message` member (doc: "The request was rejected because the specified resource can't be found."). For an index that does not exist the recorded response is HTTP 404, x-amzn-errortype NotFoundException, message exactly "The specified index could not be found". That message is entirely FIXED text — it interpolates no bucket name, index name or ARN from the request, so there is no variable part to fill.
Emulator: HTTP 404 NotFoundException with message "The specified vector index could not be found" — the extra word 'vector'. The sentence comes from the shared constant `INDEX_NOT_FOUND` (services/service-com-amazonaws-s3vectors-s3vectors/src/state.rs:302), whose own doc comment states the index wording was invented "by analogy" with the bucket wording because "no recording of this corpus carries the index wording". This corpus does carry it, and it is not the analogy.
Why: a client that branches on, logs or asserts the refusal text for a missing index — the common pattern for distinguishing a missing index from a missing bucket, since both arrive as the same 404 NotFoundException code — matches on a string the real service never sends. Such a branch silently takes its fallback path here and takes the intended path against AWS, and any test asserting the documented sentence cannot pass.
Fix: the shared not-found message constants in `crate::state` (services/service-com-amazonaws-s3vectors-s3vectors/src/state.rs, `INDEX_NOT_FOUND`), which every index lookup lifts into its operation's NotFoundException via `LookupError::not_found_exception`.
Traces: s3vectors-bucket-and-index-lifecycle#teardown#5 (and 14 further steps in the same cluster, e.g. s3vectors-index-lifecycle-basic#task#6, s3vectors-get-and-delete-index-by-arn#task#5).

=== Cause 2 of 6: DeleteIndex's NotFoundException carries the same wrong sentence ===
AWS: Smithy DeleteIndex models NotFoundException (HTTP 404, required `message`). Deleting an index that does not exist is refused with HTTP 404 NotFoundException and the same fixed sentence "The specified index could not be found".
Emulator: HTTP 404 NotFoundException, message "The specified vector index could not be found".
Why: the delete path is where idempotent teardown code lives — a client that tolerates "already gone" by inspecting the refusal text treats this as an unknown failure and aborts a teardown that AWS would let it finish.
Fix: same component as Cause 1 (`INDEX_NOT_FOUND` in crate::state); DeleteIndex's handler lifts the same LookupError.
Traces: s3vectors-bucket-and-index-lifecycle#teardown#6 (cluster of 5, e.g. s3vectors-error-notfound-operations#task#2).

=== Cause 3 of 6: ListTagsForResource on a missing index carries the same wrong sentence ===
AWS: Smithy ListTagsForResource (GET /tags/{resourceArn}) models NotFoundException (HTTP 404, required `message`). For a resourceArn naming an index that does not exist, the refusal is HTTP 404 NotFoundException with the fixed sentence "The specified index could not be found".
Emulator: HTTP 404 NotFoundException, message "The specified vector index could not be found".
Why: the tagging surface is reached by ARN, so the refusal text is the only thing telling the caller WHICH resource the ARN failed to resolve; the wrong sentence defeats that discrimination.
Fix: same component as Cause 1 — the tagging handlers resolve the ARN through crate::state and lift `INDEX_NOT_FOUND`.
Traces: s3vectors-tag-resource-nonexistent-index#task#4.

=== Cause 4 of 6: TagResource on a missing index carries the same wrong sentence ===
AWS: Smithy TagResource models NotFoundException (HTTP 404) and ConflictException; tagging an index that does not exist is refused with HTTP 404 NotFoundException and the fixed sentence "The specified index could not be found".
Emulator: HTTP 404 NotFoundException, message "The specified vector index could not be found".
Why: as Cause 3 — a tag-on-create helper that retries or reports based on the refusal text misclassifies the failure.
Fix: same component as Cause 1.
Traces: s3vectors-tag-resource-nonexistent-index#task#5.

=== Cause 5 of 6: CreateIndex's ConflictException replaces the service's sentence with a request-derived one ===
AWS: Smithy models ConflictException on CreateIndex with httpError 409 and a required `message` (doc: "The request failed because a vector bucket name or a vector index name already exists ... Choose a different name"). Re-creating an existing index is refused with HTTP 409 ConflictException and message exactly "An index with the specified name already exists" — again all FIXED text, with no index or bucket name interpolated.
Emulator: HTTP 409 ConflictException with a constructed sentence of the form "vector index already exists: <indexName> in vector bucket <vectorBucketName>" (recorded instance: "vector index already exists: sim2real-probe-s3vectors-idx-38c4fba7 in vector bucket sim2real-probe-s3vectors-vb-38c4fba7" — the two names are values from the request, not part of any template AWS uses).
Why: the message is both misworded and over-shares, so no client assertion on the conflict text can hold, and a caller that parses the text to recover the conflicting name is being taught a shape AWS does not emit.
Fix: CreateIndex's conflict path — the ConflictException message it constructs (its handler in services/service-com-amazonaws-s3vectors-s3vectors/src/ops/create_index.rs and the already-exists refusal it lifts from crate::state): emit the single fixed sentence with no request values.
Traces: s3vectors-bucket-and-index-lifecycle#task#3 (cluster of 4, e.g. s3vectors-create-index-errors#task#3).

=== Cause 6 of 6: CreateVectorBucket's ConflictException likewise ===
AWS: Smithy models ConflictException (HTTP 409, required `message`) on CreateVectorBucket. Re-creating an existing vector bucket is refused with HTTP 409 ConflictException and message exactly "A vector bucket with the specified name already exists" — fixed text, no bucket name interpolated.
Emulator: HTTP 409 ConflictException with "vector bucket already exists: <vectorBucketName>" (recorded instance: "vector bucket already exists: sim2real-probe-s3vectors-vb-20fdcc5a"; the name is a request value).
Why: duplicate-create is the single most commonly tolerated error in provisioning code (create-if-absent), and it is tolerated by inspecting code plus text; the wrong sentence makes that tolerance untestable here.
Fix: CreateVectorBucket's conflict path — the ConflictException message it constructs (services/service-com-amazonaws-s3vectors-s3vectors/src/ops/create_vector_bucket.rs and the shared already-exists refusal in crate::state).
Traces: s3vectors-duplicate-bucket-and-invalid-put-errors#task#3 (cluster of 4, e.g. s3vectors-error-duplicate-bucket-creation#task#2).

Related open issue (does NOT cover any cause here): "s3vectors: 6 behavioural divergences ..." (20261023T014718Z-s3vectors-6-behavioural-divergences-no-iam-authorization-on.md) concerns authorization, CloudFormation and pagination, not refusal wording.

## Evidence
Cause 1 — GetIndex on a missing index: expected HTTP 404 $.message == 'The specified index could not be found'; actual HTTP 404 $.message == 'The specified vector index could not be found' (15 steps).
Cause 2 — DeleteIndex on a missing index: expected 404 'The specified index could not be found'; actual 404 'The specified vector index could not be found' (5 steps).
Cause 3 — ListTagsForResource on an ARN for a missing index: expected 404 'The specified index could not be found'; actual 404 'The specified vector index could not be found'.
Cause 4 — TagResource on an ARN for a missing index: expected 404 'The specified index could not be found'; actual 404 'The specified vector index could not be found'.
Cause 5 — CreateIndex on an existing index: expected 409 'An index with the specified name already exists'; actual 409 'vector index already exists: <indexName> in vector bucket <bucketName>' (4 steps).
Cause 6 — CreateVectorBucket on an existing bucket: expected 409 'A vector bucket with the specified name already exists'; actual 409 'vector bucket already exists: <bucketName>' (4 steps).
In every cause the HTTP status and x-amzn-errortype matched; only the `message` member diverged.

## Steps to Reproduce
1. git_clone, then replay the simparity-wire-aws-v1 wire corpus against the built emulator (replay_wire_corpus).
2. Cause 1: CreateVectorBucket + CreateIndex, DeleteIndex, then GetIndex(vectorBucketName, indexName) for the deleted index — read $.message of the 404 (fixture s3vectors-bucket-and-index-lifecycle, step teardown#5).
3. Cause 2: call DeleteIndex again on the same deleted index — read $.message of the 404 (same fixture, step teardown#6).
4. Cause 3/4: ListTagsForResource and TagResource with a resourceArn naming an index that does not exist (fixture s3vectors-tag-resource-nonexistent-index, steps task#4 and task#5).
5. Cause 5: CreateIndex twice with the same indexName in one bucket (fixture s3vectors-bucket-and-index-lifecycle, step task#3).
6. Cause 6: CreateVectorBucket twice with the same vectorBucketName (fixture s3vectors-duplicate-bucket-and-invalid-put-errors, step task#3).
