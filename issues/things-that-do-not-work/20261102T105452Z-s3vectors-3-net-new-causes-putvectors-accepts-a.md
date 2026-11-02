Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3vectors.s3v-vbi-put-vectors-with-duplicate-keys-in-one-request-rejected s3vectors.s3v-vbi-query-vectors-with-the-wrong-dimension-rejected s3vectors.s3v-vbi-list-vectors-segment-index-past-segment-count-rejected
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors.s3v-vbi-put-vectors-with-duplicate-keys-in-one-request-rejected|_error#4|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: s3vectors.s3v-vbi-query-vectors-with-the-wrong-dimension-rejected|run#6|AssertionError|assert got == (
Issue-Key: s3vectors.s3v-vbi-list-vectors-segment-index-past-segment-count-rejected|run#10|AssertionError|assert message == _MSG_SEGMENT_OUT_OF_RANGE, (
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# s3vectors: 3 net-new causes — PutVectors accepts a request carrying the same vector key twice, QueryVectors' wrong-dimension refusal uses the PutVectors sentence, and ListVectors' segment-range refusal is emulator-worded

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutVectors

## Description
THREE net-new root causes in the s3vectors emulator's data-plane refusal contract, all grounded in failing checks the replay report left unmarked (neither ALREADY FILED nor OWNED). Cause 1 is a request AWS REFUSES that the emulator ACCEPTS with 200; Causes 2 and 3 are refusals made at the right moment with the right status and code but carrying the emulator's own sentence. All three must be closed before this issue is submitted.

Neighbouring OPEN issue, named so a fixer does not mistake it for coverage: 20261029T195812Z-s3vectors-s3-vectors-refusal-messages-diverge-on-8-further.md carries ten other s3vectors check ids, including (in its Cause 5) PutVectors' dimension-mismatch wording and (in its Cause 8) the SAME ListVectors segment-range sentence as Cause 3 below, reached through the check s3vectors.list-vectors-parallel-segments-partition-the-index. Cause 3 here is grounded in a DIFFERENT check (s3v-vbi-list-vectors-segment-index-past-segment-count-rejected) whose replay key that issue does not carry, so it is re-filed to keep that check grounded; it should close together with that issue's Cause 8. Causes 1 and 2 appear in no open issue.

=== Cause 1 of 3: PutVectors accepts one request that carries the same vector key twice ===
AWS: Smithy (services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/com-amazonaws-s3vectors-s3vectors.json) models PutVectors (POST /PutVectors, http 200, @idempotent) with required `vectors` (PutVectorsInputList, @length min 1 max 500) and ValidationException (errorKind client, httpError 400, required `message`); each entry carries a `key`. The model does not express the cross-entry uniqueness rule, so recorded behavior supplies it: a single PutVectors request whose `vectors` list holds two entries with the SAME key is refused with HTTP 400 ValidationException and message exactly "Request must not contain duplicate keys" — entirely FIXED text, with no key, index name or position interpolated — and nothing is written (a subsequent ListVectors on the index returns an empty list). The same vector submitted ONCE is accepted and stored.
Emulator: the duplicate-key request SUCCEEDS — HTTP 200 with an empty body, so the batch is silently de-duplicated (or the last entry silently wins) and no validation of the batch's key set happens at all.
Why: batch writers build the `vectors` list by concatenating or re-chunking upstream pages, and a duplicated key means the batch was assembled wrongly — AWS refuses the whole request so the producer can fix and re-send it. Against the emulator the defect is swallowed, so a pipeline silently stores a different number of vectors than it believes it sent, the atomic "nothing is written" guarantee cannot be verified, and no client-side handling of this ValidationException is ever exercised.
Fix: PutVectors' input validation in the s3vectors crate (services/service-com-amazonaws-s3vectors-s3vectors/src/ops/put_vectors.rs and the batch pre-checks it calls in crate::state) — reject the request with AWS's fixed sentence when the submitted keys are not unique, before any vector is written.
Traces: s3vectors.s3v-vbi-put-vectors-with-duplicate-keys-in-one-request-rejected.

=== Cause 2 of 3: QueryVectors' wrong-width query vector is refused with PutVectors' record-level sentence instead of the query-vector sentence ===
AWS: Smithy models QueryVectors (POST /QueryVectors) with required `queryVector` (VectorData) and required `topK`, and ValidationException (errorKind client, httpError 400, required `message`); the index's own `dimension` (1..4096) fixes the admissible query width. Recorded behavior: a query whose `queryVector.float32` length disagrees with the bound index's dimension is refused with HTTP 400 ValidationException and message exactly "Query vector contains invalid values or is invalid for this index" — all FIXED text: neither the submitted length, nor the index dimension, nor the index name is interpolated. (This is the same sentence Bedrock forwards when it probes an index at CreateKnowledgeBase time, which is how a caller recognises it.) A query at the index's own dimension answers normally.
Emulator: HTTP 400 ValidationException (correct status and code) with "vector must have length 123, but has length 12" — the WRITE path's record-level dimension sentence, echoing the index dimension and the submitted length, re-used on the read path.
Why: ValidationException covers every input defect on QueryVectors, so the sentence is the only discriminator. AWS deliberately answers one opaque sentence for a bad query vector (it covers NaN/infinite values as well as the wrong width), and consumers — including other AWS services that forward this refusal verbatim, as Bedrock's knowledge-base storage validation does — match on it. A client asserting or forwarding AWS's wording diverges, and the echoed numbers are a shape AWS does not emit for this error class.
Fix: QueryVectors' input validation in the s3vectors crate (services/service-com-amazonaws-s3vectors-s3vectors/src/ops/query_vectors.rs and the query-vector check it calls in crate::state) — emit the query-vector sentence with no submitted values, keeping the PutVectors record-level sentence to the write path.
Traces: s3vectors.s3v-vbi-query-vectors-with-the-wrong-dimension-rejected.

=== Cause 3 of 3: ListVectors' segmentIndex-out-of-range refusal is emulator-worded and echoes the submitted numbers ===
AWS: Smithy models ListVectorsInput `segmentCount` (integer, default 1, range 1..16) and `segmentIndex` (integer, default 0, range 0..15), and ValidationException (400, required `message`) on ListVectors. segmentIndex == segmentCount is inside BOTH shape ranges, so the request leaves the SDK and the SERVICE refuses it: HTTP 400 ValidationException with message exactly "Segment index is zero-indexed and must be smaller than segment count" — all fixed text restating the rule, interpolating neither submitted number. The control (segmentIndex = segmentCount - 1) lists a subset of the index.
Emulator: HTTP 400 ValidationException (correct code; the control segment also lists correctly) with "`segmentIndex` (2) must be less than `segmentCount` (2)" — the emulator's own sentence, naming the wire members in backticks and echoing both submitted values (the 2s here are this run's request values, not part of any template).
Why: a parallel-listing worker pool derives its segment assignment from its own worker count, and this refusal is how a mis-derived assignment is reported; a client asserting or surfacing AWS's wording diverges.
Fix: ListVectors' input validation in the s3vectors crate (services/service-com-amazonaws-s3vectors-s3vectors/src/ops/list_vectors.rs and the segment cross-field check it calls in crate::state) — emit the service's fixed sentence with no submitted values. Shared root cause with Cause 8 of open issue 20261029T195812Z-s3vectors-s3-vectors-refusal-messages-diverge-on-8-further.md; this check must be re-replayed on top of that fix.
Traces: s3vectors.s3v-vbi-list-vectors-segment-index-past-segment-count-rejected.

## Evidence
Cause 1 — PutVectors(indexArn=<empty 123-dimension float32/cosine index>, vectors=[{key:'doc-001', data:{float32:[0.1]*123}, metadata:{...}}, <the identical entry again>]): expected 400 ValidationException with message 'Request must not contain duplicate keys' and an index still empty; actual HTTP 200 with an empty body ({'ResponseMetadata': {'HTTPStatusCode': 200, ...}}). AssertionError: expected a ClientError, but the call SUCCEEDED.

Cause 2 — QueryVectors(indexArn=<123-dimension index>, topK=3, queryVector={float32:[0.5]*12}): expected ('ValidationException', 'Query vector contains invalid values or is invalid for this index'); actual ('ValidationException', 'vector must have length 123, but has length 12'). The 123 and 12 in the emulator's sentence are the index dimension and the submitted length — values AWS does not interpolate for this error. The full-dimension control query returns all three seeded vectors against both.

Cause 3 — ListVectors(indexArn=<seeded index>, segmentCount=2, segmentIndex=2), both values inside the modeled shape ranges: expected 400 ValidationException 'Segment index is zero-indexed and must be smaller than segment count'; actual 400 ValidationException '`segmentIndex` (2) must be less than `segmentCount` (2)'. The preceding ListVectors(segmentCount=2, segmentIndex=1) control succeeded and listed a subset.

In Causes 2 and 3 the HTTP status (400) and the error code (ValidationException) matched and only the `message` member diverged; in Cause 1 the emulator answered 200 where AWS answers 400.

## Steps to Reproduce
0. git_clone, then run_behavioral_suite and read the '### s3vectors' section of the failing-check list. The grounding checks are s3vectors.s3v-vbi-put-vectors-with-duplicate-keys-in-one-request-rejected, s3vectors.s3v-vbi-query-vectors-with-the-wrong-dimension-rejected, s3vectors.s3v-vbi-list-vectors-segment-index-past-segment-count-rejected.
1. Shared setup: CreateVectorBucket(<fresh name>), then CreateIndex twice with dataType=float32, dimension=123, distanceMetric=cosine — one index seeded with three vectors (keys doc-001..doc-003, each a constant fill, two metadata keys), one left empty.
2. Cause 1: PutVectors(indexArn=<the EMPTY index>, vectors=[v, v]) where both entries carry key 'doc-001' -> observe HTTP 200 instead of 400 ValidationException 'Request must not contain duplicate keys'; ListVectors on that index to confirm what (if anything) was written. Then PutVectors with the single entry and confirm it stores exactly that key.
3. Cause 2: QueryVectors(indexArn=<the SEEDED index>, topK=3, queryVector={'float32':[0.5]*123}) as the control, then QueryVectors with queryVector={'float32':[0.5]*12} -> read $.message of the 400.
4. Cause 3: ListVectors(indexArn=<the SEEDED index>, segmentCount=2, segmentIndex=1) as the control, then ListVectors(segmentCount=2, segmentIndex=2) -> read $.message of the 400.
