Issue-Source: trace-sweep-divergence
Issue-Trace: s3vectors-resource-selector-validation
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3vectors-resource-selector-validation
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors|GetVectorBucket|4xx->2xx|$.message
Issue-Key: s3vectors|GetVectorBucket|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: s3vectors|GetVectorBucket|4xx->2xx|status
Issue-Key: s3vectors|GetIndex|4xx->2xx|$.message
Issue-Key: s3vectors|GetIndex|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: s3vectors|GetIndex|4xx->2xx|status
Issue-Key: s3vectors|GetVectorBucket|4xx->4xx|$.message
Issue-Key: s3vectors|ListIndexes|4xx->4xx|$.message
Issue-Related: 20261029T192108Z-s3vectors-s3-vectors-data-plane-operations-report-the-vector.md
Issue-Related: 20261029T192108Z-s3vectors-s3-vectors-not-found-and-already-exists-refusals.md
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# S3 Vectors selector validation: naming both the name and the ARN is accepted instead of refused, and the selector refusal is misworded

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVectorBucket

## Description
Six independent causes around the resource SELECTOR every s3vectors operation takes (vectorBucketName xor vectorBucketArn; or vectorBucketName+indexName xor indexArn). S3 Vectors treats the two forms as mutually EXCLUSIVE and answers ONE fixed sentence for any selector that is not exactly one of them — whether none was given or both were. The emulator instead accepts 'both' whenever the two agree (HTTP 200), and words the refusal it does emit in its own terms. Each cause is numbered and carries its own fixture/step; all six must be closed before this issue is submitted.

=== Cause 1 of 6: GetVectorBucket accepts vectorBucketName AND vectorBucketArn together (200 instead of 400) ===
AWS: Smithy (services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/com-amazonaws-s3vectors-s3vectors.json) documents GetVectorBucket as "To specify the bucket, you must use either the vector bucket name or the vector bucket ARN" and models ValidationException (HTTP 400, required `message`). A request carrying BOTH selectors — even two that name the same bucket — is refused with HTTP 400 ValidationException and the fixed sentence "Must specify either vectorBucketName or vectorBucketArn but not both". The "but not both" clause is the operative half: agreement does not make the pair valid.
Emulator: HTTP 200 with the bucket's attributes. `crate::state::resolve_bucket_name` (services/service-com-amazonaws-s3vectors-s3vectors/src/state.rs:515-535) explicitly accepts the both-specified case when the two agree, and refuses only when they disagree; its doc comment states that intent.
Why: the emulator is more permissive than the service on a shape clients hit constantly — code that fills in both the name it has and the ARN it cached passes every test here and is refused by real AWS on the first call. An SDK wrapper that unconditionally forwards both selectors looks correct until it reaches production.
Fix: `resolve_bucket_name` in crate::state (used by every bucket-addressed handler, e.g. src/ops/get_vector_bucket.rs:154): refuse whenever both selectors are present, regardless of whether they agree, with the service's fixed sentence.
Traces: s3vectors-resource-selector-validation#task#4.

=== Cause 2 of 6: GetIndex accepts indexArn together with vectorBucketName/indexName (200 instead of 400) ===
AWS: Smithy documents GetIndex as "To specify the vector index, you can either use both the vector bucket name and the vector index name, or use the vector index ARN", and refuses a request carrying both forms with HTTP 400 ValidationException, message "Must specify either indexArn or both vectorBucketName and indexName" — again a single fixed sentence with no request values.
Emulator: HTTP 200 with the index's attributes. `crate::state::resolve_index_name` (src/state.rs:544+) accepts the mixed form when the ARN agrees with the name pair.
Why: as Cause 1, for the index selector — the form most clients reach for once they hold an ARN from CreateIndex and a name from configuration.
Fix: `resolve_index_name` in crate::state (used by GetIndex, QueryVectors, PutVectors, ListVectors, DeleteVectors, ...): refuse any request that carries the ARN alongside either name member.
Traces: s3vectors-resource-selector-validation#task#10.

=== Cause 3 of 6: GetVectorBucket's no-selector refusal is misworded ===
AWS: a GetVectorBucket naming NEITHER vectorBucketName nor vectorBucketArn is refused with HTTP 400 ValidationException and the SAME fixed sentence as Cause 1: "Must specify either vectorBucketName or vectorBucketArn but not both" — the service answers one sentence for the whole selector rule rather than one per failure mode.
Emulator: HTTP 400 ValidationException with "either `vectorBucketName` or `vectorBucketArn` is required" (the (None, None) arm of resolve_bucket_name, src/state.rs:531-533).
Why: the status and code are right, but a client (or a test) matching the documented sentence sees an unrecognised error; and because the emulator words the two failure modes differently, code that keys off the single sentence cannot cover both.
Fix: same component as Cause 1 — collapse both selector failures in `resolve_bucket_name` onto the service's single sentence.
Traces: s3vectors-resource-selector-validation#task#3.

=== Cause 4 of 6: ListIndexes' no-selector refusal is misworded ===
AWS: ListIndexes takes the bucket selector ("To specify the bucket, you must use either ...") and refuses a request naming neither with HTTP 400 ValidationException, message "Must specify either vectorBucketName or vectorBucketArn but not both".
Emulator: HTTP 400 ValidationException with "either `vectorBucketName` or `vectorBucketArn` is required".
Why: as Cause 3.
Fix: same component as Cause 1 (`resolve_bucket_name`, reached from src/ops/list_indexes.rs).
Traces: s3vectors-resource-selector-validation#task#6.

=== Cause 5 of 6: GetIndex's incomplete-selector refusal is misworded ===
AWS: a GetIndex carrying neither form, or only half of the name pair (bucket without index, or index without bucket), is refused with HTTP 400 ValidationException and the fixed sentence "Must specify either indexArn or both vectorBucketName and indexName".
Emulator: HTTP 400 ValidationException with "either both `vectorBucketName` and `indexName`, or `indexArn`, is required" (the fallback arm of resolve_index_name).
Why: as Cause 3, for the index selector; three separate recorded steps (no selector, bucket only, index only) all take this sentence, so every partial-selector probe a client makes diverges.
Fix: same component as Cause 2 (`resolve_index_name`).
Traces: s3vectors-resource-selector-validation#task#7 (cluster of 3, also task#8 and task#9 of the same fixture).

=== Cause 6 of 6: QueryVectors' incomplete-selector refusal is misworded ===
AWS: QueryVectors takes the same index selector and refuses an incomplete one with HTTP 400 ValidationException, message "Must specify either indexArn or both vectorBucketName and indexName".
Emulator: HTTP 400 ValidationException with "either both `vectorBucketName` and `indexName`, or `indexArn`, is required".
Why: as Cause 5, on the data plane's hottest operation.
Fix: same component as Cause 2 (`resolve_index_name`, reached from src/ops/query_vectors.rs).
Traces: s3vectors-resource-selector-validation#task#12.

## Evidence
Cause 1 — GetVectorBucket(vectorBucketName=<b>, vectorBucketArn=<arn of b>): expected HTTP 400 with $.message 'Must specify either vectorBucketName or vectorBucketArn but not both' and an x-amzn-errortype error header; actual HTTP 200 with the bucket payload and no message/errortype.
Cause 2 — GetIndex(vectorBucketName=<b>, indexName=<i>, indexArn=<arn of b/i>): expected HTTP 400 'Must specify either indexArn or both vectorBucketName and indexName'; actual HTTP 200 with the index payload.
Cause 3 — GetVectorBucket with neither selector: expected 400 'Must specify either vectorBucketName or vectorBucketArn but not both'; actual 400 'either `vectorBucketName` or `vectorBucketArn` is required'.
Cause 4 — ListIndexes with neither selector: expected 400 'Must specify either vectorBucketName or vectorBucketArn but not both'; actual 400 'either `vectorBucketName` or `vectorBucketArn` is required'.
Cause 5 — GetIndex with an incomplete selector (3 steps: none, bucket only, index only): expected 400 'Must specify either indexArn or both vectorBucketName and indexName'; actual 400 'either both `vectorBucketName` and `indexName`, or `indexArn`, is required'.
Cause 6 — QueryVectors with an incomplete selector: expected 400 'Must specify either indexArn or both vectorBucketName and indexName'; actual 400 'either both `vectorBucketName` and `indexName`, or `indexArn`, is required'.
All six steps are in fixture s3vectors-resource-selector-validation.

## Steps to Reproduce
1. git_clone, then replay the simparity-wire-aws-v1 wire corpus against the built emulator (replay_wire_corpus).
2. CreateVectorBucket + CreateIndex and keep both the names and the minted ARNs.
3. Cause 1: GetVectorBucket passing BOTH vectorBucketName and the matching vectorBucketArn — observe HTTP 200 instead of 400 ValidationException (step task#4).
4. Cause 2: GetIndex passing vectorBucketName + indexName AND the matching indexArn — observe HTTP 200 instead of 400 (step task#10).
5. Cause 3: GetVectorBucket with no selector at all — read $.message (step task#3).
6. Cause 4: ListIndexes with no selector — read $.message (step task#6).
7. Cause 5: GetIndex with no selector, then with vectorBucketName only, then with indexName only — read $.message (steps task#7, task#8, task#9).
8. Cause 6: QueryVectors with an incomplete selector — read $.message (step task#12).
