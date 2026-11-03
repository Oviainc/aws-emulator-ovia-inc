Issue-Source: trace-sweep-divergence
Issue-Trace: s3vectors-vbi-list-vectors-paging
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3vectors-vbi-list-vectors-paging s3vectors-resource-selector-validation
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors|ListVectors|4xx->2xx|$.fieldList
Issue-Key: s3vectors|ListVectors|4xx->2xx|$.message
Issue-Key: s3vectors|ListVectors|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: s3vectors|ListVectors|4xx->2xx|status
Issue-Key: s3vectors|ListVectors|2xx->2xx|$.nextToken
Issue-Key: s3vectors|GetVectorBucket|2xx->2xx|$.vectorBucket.creationTime
Issue-Key: s3vectors|GetIndex|2xx->2xx|$.index.creationTime
Issue-Commit: 56c3e2d5db4619f120a986f75a3622f8f9af8979

# s3vectors read paths serve internal values on the wire: a raw-vector-key nextToken that is never validated, and creationTime timestamps in the wrong serialized form

> _Note: this issue was identified against the code base at commit `56c3e2d5db4619f120a986f75a3622f8f9af8979`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListVectors

## Description
FOUR independent net-new root causes found by the WIRE replay, all of them the same shared fault: an s3vectors read path hands the emulator's INTERNAL state value straight to the client instead of the wire form the service serves. Causes 1 and 2 are the ListVectors pagination token (it is the raw vector key, and because it is not a minted token it is never validated, so a forged token is answered 200 where AWS answers 400). Causes 3 and 4 are the `creationTime` timestamp on the two control-plane reads. None of the four clusters carried a stub_path or an owned_by, and no open issue names any of these wire steps. All four must be closed before this issue is submitted.

Model consulted: services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/com-amazonaws-s3vectors-s3vectors.json (S3Vectors, version 2025-07-15, protocol restJson1).

=== Cause 1 of 4: ListVectors' nextToken is the raw key of the last vector on the page ===
AWS: Smithy models ListVectors as @paginated (inputToken nextToken, outputToken nextToken, items vectors, pageSize maxResults) with ListVectorsNextToken a string of @length 1..2048. Model-wise that is all it says, so: recorded behavior only — the recorded responses carry an opaque, service-minted continuation token whose shape the fixture asserts, and which does NOT disclose the index's key space.
Emulator: the token is the vector KEY of the page's last item, handed back verbatim (the replayed page returned the key of a seeded vector as its whole nextToken), so the shape assertion on the member fails. The token is the internal cursor, not an encoded one.
Why: a caller that treats the token as opaque is unaffected, but AWS's token is also the unit that carries the page's SCOPE (the index, the selection, the segment) and is what makes a forged or cross-listing token detectable — which is precisely what Cause 2 shows is lost here. A raw-key token additionally leaks the key space of the index to any client that can see a page boundary, and it teaches clients a token format they can parse and construct, which AWS does not offer.
Fix: ListVectors' pagination in the s3vectors crate (services/service-com-amazonaws-s3vectors-s3vectors/src/ops/list_vectors.rs and the cursor mint/decode helpers it calls in crate::state) — mint an encoded token that binds the cursor to the listing that produced it, instead of returning the last key.
Traces: s3vectors-vbi-list-vectors-paging#task#4.

=== Cause 2 of 4: a nextToken this service never issued is answered HTTP 200 with a page instead of 400 ValidationException "Invalid page token" ===
AWS: Smithy models ValidationException on ListVectors (errorKind client, httpError 400, required `message`, optional `fieldList` of ValidationExceptionField{path, message}). A nextToken that the service did not mint is refused with HTTP 400 ValidationException whose `message` is exactly "Invalid page token" and whose `fieldList` is exactly [{path: "nextToken", message: "Invalid page token"}] — all fixed text, with the rejected token NOT echoed, and the x-amzn-errortype header naming ValidationException.
Emulator: HTTP 200 with a normal page body and no error members at all ($.message, $.fieldList and the error-type header are all absent). The token is accepted because, per Cause 1, it is interpreted as a bare key position rather than decoded as a token the service minted.
Why: this is the most consequential of the four — a request AWS REFUSES is SERVED. A paginating client that corrupts, truncates, caches across listings or hand-builds a token gets a silently mispositioned page: records before the bogus position are skipped, or the walk restarts, and the client believes it enumerated the index. Nothing surfaces the defect, so the token-refusal path a real client must handle (and any test asserting it) is never exercised against the emulator.
Fix: ListVectors' pagination input validation (services/service-com-amazonaws-s3vectors-s3vectors/src/ops/list_vectors.rs and the cursor decode helper in crate::state) — decode-or-refuse: any token not minted by this listing must raise ValidationException with AWS's fixed sentence at the top level and the same sentence under fieldList path "nextToken". Closing Cause 1 is the precondition for this check being possible.
Traces: s3vectors-vbi-list-vectors-paging#task#8 (this issue's representative step).

=== Cause 3 of 4: GetVectorBucket's vectorBucket.creationTime is serialized in a form the service does not use ===
AWS: Smithy models VectorBucket.creationTime as a required Timestamp (doc: "Date and time when the vector bucket was created.") with no @timestampFormat override, so under restJson1 it is serialized as an epoch-seconds number. Recorded behavior only: the recorded GetVectorBucket responses carry creationTime in the epoch-seconds form the fixture asserts as a SHAPE (the instant itself is not asserted and is not part of this finding), and the emulator's value does not conform to it.
Emulator: GetVectorBucket answers HTTP 200 (correct) with vectorBucket.creationTime emitted as the raw system clock reading carrying sub-second fractional precision (six fractional digits) rather than the whole epoch-seconds value the recordings carry. Hypothesis: the handler serializes its stored SystemTime/f64 directly instead of passing it through the protocol's timestamp serializer, which is why the fractional component appears.
Why: creationTime is a required member, so every client deserializes it. A consumer with a stricter timestamp reader (an integer-typed field, a schema-validating proxy, a log pipeline, a diff against a recorded response) rejects or mis-reads a value whose form the service never emits, and anything that round-trips or compares the member sees a difference that is not about the instant. Note explicitly: the per-run clock INSTANT is not the claim here and must not be asserted by a fix; the serialized FORM is.
Fix: the GetVectorBucket read path's timestamp serialization (services/service-com-amazonaws-s3vectors-s3vectors/src/ops/get_vector_bucket.rs and the creation-instant field on the bucket record in crate::state) — emit the member through the restJson1 epoch-seconds timestamp serializer in the form the recordings carry.
Traces: s3vectors-resource-selector-validation#task#5.

=== Cause 4 of 4: GetIndex's index.creationTime has the same wrong serialized form ===
AWS: Smithy models Index.creationTime as a required Timestamp (doc: "Date and time when the vector index was created."), again with no @timestampFormat override, so restJson1 epoch-seconds applies. Recorded behavior only, exactly as in Cause 3: the recorded GetIndex responses carry the member in the asserted shape and the emulator's value does not conform.
Emulator: GetIndex answers HTTP 200 (correct) with index.creationTime carrying the same sub-second fractional system-clock form as Cause 3.
Why: same as Cause 3, on the index read — the member is required, so no client can avoid it. It is listed as its own cause because it is a second read path with its own stored field and its own replay step: fixing only the bucket path leaves GetIndex diverging, and the two steps must both be re-replayed.
Fix: the GetIndex read path's timestamp serialization (services/service-com-amazonaws-s3vectors-s3vectors/src/ops/get_index.rs and the creation-instant field on the index record in crate::state); if a shared helper is introduced for Cause 3, this path must use it.
Traces: s3vectors-resource-selector-validation#task#11.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 499 steps, 488 passed, 11 mismatched, 0 blocked/errored (score 97.8%, emulator_up true, bootstrap_signal false). The four clusters below were reported with empty stub_path and empty owned_by.

Cause 2 (representative) — ListVectors replayed with a nextToken the emulator never minted (fixture s3vectors-vbi-list-vectors-paging, step task#8): status expected 400, got 200; $.message missing (expected 'Invalid page token', got None); $.fieldList missing (expected [{'path': 'nextToken', 'message': 'Invalid page token'}], got None); header x-amzn-errortype missing (the recorded response carries the ValidationException type, the emulator sends none). divergence_key service=s3vectors|ListVectors|status=400->200|keys=$.fieldList,$.message,$headers.x-amzn-errortype,status.

Cause 1 — ListVectors first page of a multi-page listing (fixture s3vectors-vbi-list-vectors-paging, step task#4; status 200 on both sides): $.nextToken failed the asserted shape — expected '<shaped-not-asserted>' (an opaque service-minted token), got 'doc-002', which is the KEY of the last vector on the page in this run's seeded index. The key value itself is a per-run name and not the requirement; the requirement is that the member is an encoded token and not the cursor key. divergence_key service=s3vectors|ListVectors|status=200->200|keys=$.nextToken.

Cause 3 — GetVectorBucket (fixture s3vectors-resource-selector-validation, step task#5; status 200 on both sides): $.vectorBucket.creationTime failed the asserted shape — expected '<shaped-not-asserted>' (the recorded epoch-seconds form; the instant is deliberately not asserted), got 1793676776.444361, i.e. an epoch-seconds value carrying six fractional digits of the replay host's system clock. divergence_key service=s3vectors|GetVectorBucket|status=200->200|keys=$.vectorBucket.creationTime.

Cause 4 — GetIndex (fixture s3vectors-resource-selector-validation, step task#11; status 200 on both sides): $.index.creationTime, same failure — expected '<shaped-not-asserted>', got 1793676776.44943. divergence_key service=s3vectors|GetIndex|status=200->200|keys=$.index.creationTime.

The numeric instants above (1793676776.*) are this replay's clock and the 'doc-002' key is this fixture's seeded vector key; neither is a requirement. What diverges is the serialized form of the timestamp member, the fact that the token is the raw cursor key, and the missing 400 refusal.

## Steps to Reproduce
0. git_clone, then replay_wire_corpus and read the s3vectors clusters. The grounding wire steps are s3vectors-vbi-list-vectors-paging#task#8 (Cause 2), s3vectors-vbi-list-vectors-paging#task#4 (Cause 1), s3vectors-resource-selector-validation#task#5 (Cause 3) and s3vectors-resource-selector-validation#task#11 (Cause 4). All four must be re-replayed to confirm the fix.
1. Shared setup: CreateVectorBucket(<fresh lower-case name>), then CreateIndex(dataType=float32, dimension=123, distanceMetric=cosine) and PutVectors of several vectors with distinct keys (e.g. doc-001..doc-005).
2. Cause 1: POST /ListVectors with vectorBucketName/indexName and maxResults smaller than the number of seeded vectors -> inspect $.nextToken and compare it with the key of the last entry in $.vectors; it must not be that key, and must be an encoded token.
3. Cause 2: POST /ListVectors again with nextToken set to a token this listing never minted (a hand-written string, a truncated copy of the token from step 2, or a token minted for a different index) -> observe HTTP 200 with a page instead of 400 ValidationException with $.message == 'Invalid page token' and $.fieldList == [{path: 'nextToken', message: 'Invalid page token'}]; check the x-amzn-errortype header as well.
4. Cause 3: POST /GetVectorBucket for the bucket -> read $.vectorBucket.creationTime and compare its serialized form with the recorded fixture's form (whole epoch seconds, no sub-second fraction).
5. Cause 4: POST /GetIndex for the index -> read $.index.creationTime and apply the same comparison.
