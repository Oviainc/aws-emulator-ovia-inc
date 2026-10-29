Issue-Source: trace-sweep-divergence
Issue-Trace: s3vectors-not-found-errors-on-missing-bucket-and-index
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3vectors-not-found-errors-on-missing-bucket-and-index s3vectors-vector-data-plane-not-found-errors s3vectors-vectors-operations-on-nonexistent-bucket-error
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors|PutVectors|4xx->4xx|$.message
Issue-Key: s3vectors|GetVectors|4xx->4xx|$.message
Issue-Key: s3vectors|ListVectors|4xx->4xx|$.message
Issue-Key: s3vectors|QueryVectors|4xx->4xx|$.message
Issue-Key: s3vectors|DeleteVectors|4xx->4xx|$.message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# S3 Vectors data-plane operations report the vector bucket as missing where the service reports the index

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutVectors

## Description
Five independent causes, one per vector data-plane operation, with a single shared fault: when a data-plane call is addressed by the (vectorBucketName, indexName) pair and NEITHER resource exists, S3 Vectors refuses it as a missing INDEX — the index is the resource the operation addresses — while the emulator resolves the bucket first and reports the BUCKET as missing. Status (404) and error code (NotFoundException) are correct in all five; the `message` names the wrong resource kind. Each cause is numbered and carries its own fixture/step; all five must be closed before this issue is submitted.

=== Cause 1 of 5: PutVectors ===
AWS: Smithy (services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/com-amazonaws-s3vectors-s3vectors.json) models NotFoundException (HTTP 404, required `message`) on PutVectors, whose input selector is `vectorBucketName`+`indexName` or `indexArn` ("To specify the vector index, you can either use both the vector bucket name and ..."). The recorded refusal for a PutVectors naming a bucket AND an index that do not exist is HTTP 404 NotFoundException with the fixed sentence "The specified index could not be found" — the addressed resource is the index, and the sentence carries no request values.
Emulator: HTTP 404 NotFoundException with "The specified vector bucket could not be found" (the shared constant `BUCKET_NOT_FOUND`, services/service-com-amazonaws-s3vectors-s3vectors/src/state.rs:298), because the handler resolves the bucket via `crate::state::resolve_bucket_name`/`store.bucket()` and refuses there before the index is ever looked up. (The 'vector index' wording of the emulator's index sentence is a separate defect, filed as the not-found/already-exists wording issue.)
Why: the resource kind in the sentence is the only signal distinguishing the two failures, since both are 404 NotFoundException. A client that creates the index on demand when a write reports a missing index (the standard lazy-provision path) never fires that branch here, and a client that reports "bucket missing" to an operator sends them to fix the wrong resource.
Fix: the index-resolution order in the vector data-plane handlers — resolve the (bucket, index) selector as ONE index lookup and refuse with the index not-found sentence, rather than refusing on the bucket lookup first (`crate::state::resolve_index_name` / the per-op `store.bucket()`-then-`index()` sequence in services/service-com-amazonaws-s3vectors-s3vectors/src/ops/put_vectors.rs).
Traces: s3vectors-not-found-errors-on-missing-bucket-and-index#task#1 (cluster of 3, e.g. s3vectors-vectors-operations-on-nonexistent-bucket-error#task#1).

=== Cause 2 of 5: GetVectors ===
AWS: Smithy models NotFoundException (HTTP 404) on GetVectors with the same index selector; a GetVectors against a nonexistent bucket+index is refused with "The specified index could not be found".
Emulator: HTTP 404 NotFoundException, "The specified vector bucket could not be found".
Why: as Cause 1 — a read path that distinguishes "index not created yet" from "bucket gone" cannot do so.
Fix: same component as Cause 1, in GetVectors' handler (src/ops/get_vectors.rs).
Traces: s3vectors-not-found-errors-on-missing-bucket-and-index#task#2 (cluster of 3).

=== Cause 3 of 5: ListVectors ===
AWS: Smithy models NotFoundException (HTTP 404) on ListVectors with the same index selector; the refusal is "The specified index could not be found".
Emulator: HTTP 404 NotFoundException, "The specified vector bucket could not be found".
Why: as Cause 1.
Fix: same component as Cause 1, in ListVectors' handler (src/ops/list_vectors.rs).
Traces: s3vectors-not-found-errors-on-missing-bucket-and-index#task#3 (cluster of 3).

=== Cause 4 of 5: QueryVectors ===
AWS: Smithy models NotFoundException (HTTP 404) on QueryVectors with the same index selector; the refusal is "The specified index could not be found".
Emulator: HTTP 404 NotFoundException, "The specified vector bucket could not be found".
Why: as Cause 1; query is the hottest data-plane path, so this is the refusal clients handle most often.
Fix: same component as Cause 1, in QueryVectors' handler (src/ops/query_vectors.rs).
Traces: s3vectors-not-found-errors-on-missing-bucket-and-index#task#4 (cluster of 3).

=== Cause 5 of 5: DeleteVectors ===
AWS: Smithy models NotFoundException (HTTP 404) on DeleteVectors with the same index selector; the refusal is "The specified index could not be found".
Emulator: HTTP 404 NotFoundException, "The specified vector bucket could not be found".
Why: as Cause 1 — idempotent cleanup code keys off the missing-index sentence.
Fix: same component as Cause 1, in DeleteVectors' handler (src/ops/delete_vectors.rs).
Traces: s3vectors-vector-data-plane-not-found-errors#task#5 (cluster of 2, also s3vectors-vectors-operations-on-nonexistent-bucket-error#task#4).

## Evidence
All five causes: request addresses a vectorBucketName + indexName pair where neither resource exists.
Cause 1 PutVectors — expected HTTP 404 $.message == 'The specified index could not be found'; actual HTTP 404 $.message == 'The specified vector bucket could not be found'.
Cause 2 GetVectors — expected 404 'The specified index could not be found'; actual 404 'The specified vector bucket could not be found'.
Cause 3 ListVectors — expected 404 'The specified index could not be found'; actual 404 'The specified vector bucket could not be found'.
Cause 4 QueryVectors — expected 404 'The specified index could not be found'; actual 404 'The specified vector bucket could not be found'.
Cause 5 DeleteVectors — expected 404 'The specified index could not be found'; actual 404 'The specified vector bucket could not be found'.
Status 404 and x-amzn-errortype NotFoundException matched throughout; only the resource kind named in `message` diverged.

## Steps to Reproduce
1. git_clone, then replay the simparity-wire-aws-v1 wire corpus against the built emulator (replay_wire_corpus).
2. Without creating anything, call PutVectors(vectorBucketName=<absent-bucket>, indexName=<absent-index>, vectors=[...]) and read $.message of the 404 (fixture s3vectors-not-found-errors-on-missing-bucket-and-index, step task#1).
3. Repeat with GetVectors (step task#2), ListVectors (step task#3) and QueryVectors (step task#4) on the same absent pair.
4. Repeat with DeleteVectors (fixture s3vectors-vector-data-plane-not-found-errors, step task#5).
5. In each case the message names the vector bucket; the service names the index.
