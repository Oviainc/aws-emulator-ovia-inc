Issue-Source: trace-sweep-divergence
Issue-Trace: s3vectors-vector-bucket-policy-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3vectors-vector-bucket-policy-lifecycle s3vectors-vector-bucket-policy-overwrite s3vectors-vector-bucket-policy-roundtrip s3vectors-vector-bucket-policy-set-get-delete s3vectors-vector-bucket-policy-update-overwrite s3vectors-resource-selector-validation
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors|GetVectorBucketPolicy|2xx->2xx|$.policy
Issue-Key: s3vectors|GetVectorBucket|2xx->2xx|$.vectorBucket.creationTime
Issue-Key: s3vectors|GetVectorBucket|2xx->2xx|$.vectorBucket.defaultIndexMode
Issue-Key: s3vectors|GetIndex|2xx->2xx|$.index.creationTime
Issue-Key: s3vectors|GetIndex|2xx->2xx|$.index.indexMode
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# S3 Vectors read paths lose modeled values: index mode is omitted and a bucket policy is handed back reformatted

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVectorBucketPolicy

## Description
Three independent causes in which an s3vectors read returns HTTP 200 but the body does not carry the values the service returns: a stored document is handed back re-serialized rather than verbatim, and the index-mode enum is missing from both resource views. Each cause is numbered and carries its own fixture/step; all three must be closed before this issue is submitted.

=== Cause 1 of 3: GetVectorBucketPolicy returns a re-serialized policy instead of the document that was put ===
AWS: Smithy (services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/com-amazonaws-s3vectors-s3vectors.json) models GetVectorBucketPolicy's output as a single `policy` member (VectorBucketPolicy, a string) and PutVectorBucketPolicy's `policy` as "the JSON that defines the policy". The policy is an opaque document: the service stores the string it was given and returns it BYTE-FOR-BYTE, so a get after a put yields exactly the bytes that were put, and a get-modify-put round trip is stable.
Emulator: HTTP 200 with a `policy` string that is semantically the same JSON but textually different — the emulator parses the document and re-serializes it, inserting a space after every ':' and ',' (recorded: the put document '{"Version":"2012-10-17","Statement":[{...}]}' comes back as '{"Version": "2012-10-17", "Statement": [{...}]}'). The stored record is documented as holding the policy "exactly as PutVectorBucketPolicy was given it" (services/service-com-amazonaws-s3vectors-s3vectors/src/state.rs:153-157), so the reformatting is introduced on the put or the get path, not by intent.
Why: the policy document is a string to every client. Code that compares the policy it just wrote with the policy it reads back (drift detection, idempotent reconciliation, config checksums, terraform-style plan/apply) sees a difference on every single read and reports perpetual drift; a client that hashes or signs the document gets a different digest than the one it computed. Whitespace is not a per-run value — it is the caller's own bytes.
Fix: PutVectorBucketPolicy / GetVectorBucketPolicy (services/service-com-amazonaws-s3vectors-s3vectors/src/ops/put_vector_bucket_policy.rs and get_vector_bucket_policy.rs): validate the document by parsing a copy, but store and return the original string unmodified.
Traces: s3vectors-vector-bucket-policy-lifecycle#task#4 (cluster of 7, e.g. s3vectors-vector-bucket-policy-roundtrip#task#4, s3vectors-vector-bucket-policy-overwrite#task#3).

=== Cause 2 of 3: GetVectorBucket omits defaultIndexMode ===
AWS: Smithy's VectorBucket structure carries `defaultIndexMode` (enum IndexMode, values CLASSIC/ENHANCED), "the mode that is automatically assigned to new vector indexes in the vector bucket". A vector bucket created without ever calling PutVectorBucketDefaultIndexMode still HAS a default — the recorded GetVectorBucket response for a freshly created bucket reports defaultIndexMode 'ENHANCED' — so the member is always present, not conditional on an explicit set.
Emulator: HTTP 200 whose vectorBucket object has NO defaultIndexMode member at all. The stored record keeps `default_index_mode: Option<String>` as None "until PutVectorBucketDefaultIndexMode sets one" (src/state.rs:150-152) and the view serializes nothing when it is None.
Why: a client that reads a bucket's default before creating indexes under it (to decide whether to override the mode per index) finds the field absent and either crashes on a missing key or wrongly concludes no default exists; the emulator also cannot express the service's actual default, so mode-inheritance behavior is unverifiable.
Fix: the bucket's creation defaults and its output view — `crate::state::bucket_view` plus the default applied at CreateVectorBucket (services/service-com-amazonaws-s3vectors-s3vectors/src/state.rs): give a new bucket the service's default index mode and always serialize the member.
Note: the same step also reports creationTime, which the corpus asserts only as shaped and whose value differs per run; that part is NOT a defect and is not claimed here.
Traces: s3vectors-resource-selector-validation#task#5.

=== Cause 3 of 3: GetIndex omits indexMode ===
AWS: Smithy's Index structure carries `indexMode` (enum IndexMode: CLASSIC applies metadata filters during the vector search, ENHANCED applies them before it). An index created without an explicit mode inherits its bucket's default, and the recorded GetIndex response reports indexMode 'ENHANCED', so the member is always present.
Emulator: HTTP 200 whose index object has NO indexMode member (the record's `index_mode: Option<String>` is None, src/state.rs:175, and the view omits it).
Why: indexMode decides query semantics for metadata filtering. A client that reads the mode to choose a filter strategy, or that calls UpdateIndexMode only when the mode is not already ENHANCED, sees the field missing and takes the wrong branch; the whole CLASSIC-vs-ENHANCED distinction is invisible on this surface.
Fix: the index's creation defaults and its output view — `crate::state::index_view` plus the inheritance applied at CreateIndex (src/state.rs and src/ops/create_index.rs): resolve the index's mode from the bucket's default at create and always serialize the member.
Note: as in Cause 2, this step's creationTime is a per-run value and is not claimed.
Traces: s3vectors-resource-selector-validation#task#11.

Related open issue (does NOT cover these): "s3vectors: 6 behavioural divergences ..." (20261023T014718Z-s3vectors-6-behavioural-divergences-no-iam-authorization-on.md) reported GetVectorBucketPolicy as an unimplemented 501 stub; in this replay the operation answers 200, and the defect is the reformatted document.

## Evidence
Cause 1 — PutVectorBucketPolicy with '{"Version":"2012-10-17","Statement":[{"Sid":"AllowVectorAccess",...}]}' then GetVectorBucketPolicy: expected $.policy to be those exact bytes; actual HTTP 200 with the same JSON re-serialized with a space after every ':' and ',' ('{"Version": "2012-10-17", "Statement": [{"Sid": "AllowVectorAccess", ...}]}') — 7 steps across 5 fixtures.
Cause 2 — GetVectorBucket on a freshly created bucket: expected $.vectorBucket.defaultIndexMode == 'ENHANCED'; actual the member is missing (None). (The co-reported $.vectorBucket.creationTime is asserted as shaped only and is a per-run value; not claimed.)
Cause 3 — GetIndex on a freshly created index: expected $.index.indexMode == 'ENHANCED'; actual the member is missing (None). (Same note for $.index.creationTime.)
All three returned HTTP 200, so the divergence is in the response body alone.

## Steps to Reproduce
1. git_clone, then replay the simparity-wire-aws-v1 wire corpus against the built emulator (replay_wire_corpus).
2. Cause 1: CreateVectorBucket, PutVectorBucketPolicy with a compact (no-whitespace) policy document, then GetVectorBucketPolicy and compare $.policy byte-for-byte with what was put (fixture s3vectors-vector-bucket-policy-lifecycle, step task#4).
3. Cause 2: CreateVectorBucket, then GetVectorBucket(vectorBucketName=<b>) and look for $.vectorBucket.defaultIndexMode (fixture s3vectors-resource-selector-validation, step task#5).
4. Cause 3: CreateIndex in that bucket, then GetIndex(vectorBucketName=<b>, indexName=<i>) and look for $.index.indexMode (same fixture, step task#11).
