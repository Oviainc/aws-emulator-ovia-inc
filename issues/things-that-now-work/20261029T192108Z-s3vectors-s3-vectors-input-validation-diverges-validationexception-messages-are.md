Issue-Source: trace-sweep-divergence
Issue-Trace: s3vectors-error-invalid-encryption-configuration
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3vectors-error-invalid-encryption-configuration s3vectors-error-kms-encryption-missing-key s3vectors-error-invalid-bucket-name s3vectors-error-invalid-bucket-name-validation s3vectors-vector-bucket-invalid-name-validation s3vectors-error-invalid-dimension-validation s3vectors-vector-bucket-list-empty-and-validation s3vectors-duplicate-bucket-and-invalid-put-errors s3vectors-put-vectors-dimension-mismatch-error s3vectors-vector-bucket-policy-error-paths s3vectors-vector-bucket-policy-validation-error
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors|CreateVectorBucket|4xx->2xx|$.fieldList
Issue-Key: s3vectors|CreateVectorBucket|4xx->2xx|$.message
Issue-Key: s3vectors|CreateVectorBucket|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: s3vectors|CreateVectorBucket|4xx->2xx|status
Issue-Key: s3vectors|CreateVectorBucket|4xx->4xx|$.fieldList[*].message
Issue-Key: s3vectors|CreateIndex|4xx->4xx|$.fieldList
Issue-Key: s3vectors|ListVectorBuckets|4xx->4xx|$.fieldList
Issue-Key: s3vectors|ListVectorBuckets|4xx->4xx|$.message
Issue-Key: s3vectors|PutVectors|4xx->4xx|$.fieldList[*].message
Issue-Key: s3vectors|PutVectors|4xx->4xx|$.fieldList[*].path
Issue-Key: s3vectors|PutVectorBucketPolicy|4xx->4xx|$.message
Issue-Related: 20261029T192108Z-s3vectors-s3-vectors-data-plane-operations-report-the-vector.md
Issue-Related: 20261029T192108Z-s3vectors-s3-vectors-not-found-and-already-exists-refusals.md
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# S3 Vectors input validation diverges: ValidationException messages are emulator-worded, fieldList is dropped, and an aws:kms encryption configuration with no kmsKeyArn is accepted

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVectorBucket

## Description
Six independent causes in the s3vectors emulator's input validation: one request that S3 Vectors REFUSES is accepted with HTTP 200, and five refusals that do happen carry the emulator's own ValidationException wording (and, for the constraint-range cases, no `fieldList` at all) instead of the service's. Each cause is numbered and carries its own fixture/step; all six must be closed before this issue is submitted.

=== Cause 1 of 6: CreateVectorBucket accepts encryptionConfiguration.sseType=aws:kms with no kmsKeyArn (200 instead of 400 ValidationException) ===
AWS: Smithy (services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/com-amazonaws-s3vectors-s3vectors.json) models EncryptionConfiguration with `sseType` (enum, default AES256) and `kmsKeyArn`, whose doc states the parameter "is allowed if and only if sseType is set to aws:kms" — the biconditional runs both ways, so aws:kms without a key ARN is not a valid configuration. CreateVectorBucket therefore refuses it with HTTP 400 ValidationException (modeled, httpError 400, required `message`, optional `fieldList` of path+message), x-amzn-errortype ValidationException, message "kmsKeyArn must be specified when sseType is set to aws:kms" and fieldList [{path: "EncryptionConfiguration", message: "kmsKeyArn must be specified when sseType is set to aws:kms"}]. The sentence is fixed text naming the two member names; nothing is taken from the request.
Emulator: HTTP 200 — the bucket is CREATED. No message, no fieldList, no x-amzn-errortype. The emulator enforces only the other direction of the biconditional (kmsKeyArn present while sseType is not aws:kms); see the DEFAULT_SSE_TYPE/KMS_SSE_TYPE discussion in services/service-com-amazonaws-s3vectors-s3vectors/src/state.rs:31-41.
Why: a client gets a vector bucket that it believes is CMK-encrypted but that carries no key — the request AWS would have rejected outright succeeds, so a misconfigured encryption setting in the application under test is never surfaced here and fails only in production. Every negative test for this validation is unpassable.
Fix: CreateVectorBucket's input validation (services/service-com-amazonaws-s3vectors-s3vectors/src/ops/create_vector_bucket.rs plus the shared encryption checks in crate::state): refuse sseType == aws:kms with an absent kmsKeyArn as ValidationException (400) carrying the message above and the matching single-entry fieldList.
Traces: s3vectors-error-invalid-encryption-configuration#task#1 (cluster of 2, also s3vectors-error-kms-encryption-missing-key#task#1).

=== Cause 2 of 6: CreateVectorBucket's invalid-bucket-name refusal is misworded ===
AWS: Smithy VectorBucketName is length 3..63 and VectorBucketArn's pattern encodes the name form ([a-z0-9][a-z0-9-.]{1,61}[a-z0-9]). An ill-formed name (e.g. one with upper case and underscores) is refused with HTTP 400 ValidationException, message "Invalid vector bucket name" and fieldList[0].message "Invalid vector bucket name" — one fixed sentence that does NOT echo the rejected name or restate the rule.
Emulator: HTTP 400 ValidationException with message and fieldList[0].message both set to "`vectorBucketName` must be 3 to 63 characters of lowercase letters, numbers, hyphens and dots, and must begin and end with a letter or a number: <value>" (the trailing value is the rejected name from the request). Built by `crate::state::validate_resource_name` (src/state.rs:411-430).
Why: the status and code are right but the text is not the service's, so an assertion or a user-facing error surface built on AWS's wording diverges; the emulator also echoes caller input back into the message, a shape AWS does not use here.
Fix: `validate_resource_name` in crate::state (and its callers in create_vector_bucket.rs) — emit the service's fixed sentence per resource kind.
Traces: s3vectors-error-invalid-bucket-name#task#1 (cluster of 3, e.g. s3vectors-vector-bucket-invalid-name-validation#task#1).

=== Cause 3 of 6: CreateIndex's dimension range refusal uses the framework's constraint wording and omits fieldList ===
AWS: Smithy CreateIndex input member `dimension` is range 1..4096 (required). A dimension above the ceiling is refused with HTTP 400 ValidationException whose message is "1 validation error detected. Value at '/dimension' failed to satisfy constraint: Member must be between 1 and 4096, inclusive" and whose fieldList is [{path: "/dimension", message: "Value at '/dimension' failed to satisfy constraint: Member must be between 1 and 4096, inclusive"}]. Fixed parts: "<n> validation error(s) detected. ", "Value at '<json-pointer>' failed to satisfy constraint: Member must be between <min> and <max>, inclusive"; variable parts: the error count, the JSON-POINTER member path ('/dimension', leading slash) and the modeled min/max. The rejected VALUE is not quoted.
Emulator: HTTP 400 ValidationException with message "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must have value less than or equal to <max>" and NO fieldList at all. Three divergences in one: a colon instead of a period after "detected", the rejected value quoted and the member named without the leading slash, the one-sided "must have value less than or equal to" phrasing instead of the inclusive-range phrasing, and the dropped fieldList.
Why: fieldList is the machine-readable half of ValidationException — a client that reports which member failed reads path from fieldList, and here it finds the member absent, so it can only fall back to string-scraping a message that is also misworded. Any form-validation or retry-with-correction logic is untestable.
Fix: the service's constraint-violation rendering — the validate stage that turns a modeled range violation into ValidationException for s3vectors (the shared ValidationException construction in crate::state / the runtime validation adapter used by src/ops/create_index.rs): emit the service's sentence and populate fieldList with one JSON-pointer path per violated member.
Traces: s3vectors-error-invalid-dimension-validation#task#2.

=== Cause 4 of 6: ListVectorBuckets' maxResults range refusal, same rendering fault ===
AWS: Smithy ListVectorBucketsMaxResults is range 1..500. maxResults above 500 is refused with HTTP 400 ValidationException, message "1 validation error detected. Value at '/maxResults' failed to satisfy constraint: Member must be between 1 and 500, inclusive" and fieldList [{path: "/maxResults", message: "Value at '/maxResults' failed to satisfy constraint: Member must be between 1 and 500, inclusive"}].
Emulator: HTTP 400 ValidationException, message "1 validation error detected: Value '<value>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 500", fieldList absent.
Why: as Cause 3 — and page-size validation is exactly what a paginating client probes when tuning its loop.
Fix: same component as Cause 3 (the constraint-violation rendering shared by every s3vectors operation).
Traces: s3vectors-vector-bucket-list-empty-and-validation#task#2.

=== Cause 5 of 6: PutVectors' dimension-mismatch refusal names the wrong path and the wrong sentence ===
AWS: Smithy PutVectors models ValidationException (HTTP 400); the index's modeled `dimension` fixes the required vector length. A vector whose data length differs from the index dimension is refused with HTTP 400 ValidationException, fieldList [{path: "vectors[<i>]", message: "vector must have length <indexDimension>, but has length <actualLength>"}] and top-level message "Invalid record for key '<vectorKey>': vector must have length <indexDimension>, but has length <actualLength>". Fixed text: "Invalid record for key '", "': vector must have length ", ", but has length "; variable parts: the offending vector's position in the request list (path), its `key` from the request, the index's configured dimension, and the submitted data length. The path points at the LIST ELEMENT (vectors[0]), not at its `data` member, and the index name does not appear.
Emulator: HTTP 400 ValidationException with fieldList[0].path "vectors[0].data" and both fieldList[0].message and the top-level message set to "`vectors[0].data` has <n> values, but vector index <indexName> is configured for <d> dimensions" — a different sentence, a deeper path, the index name substituted in and the offending vector's `key` dropped from the top-level message entirely.
Why: the key in AWS's top-level message is how a client reports which record of a batch was rejected back to its caller; the emulator omits it, so batch-level error reporting cannot be exercised, and a client keying off fieldList[0].path looks up 'vectors[0].data' where AWS gives 'vectors[0]'.
Fix: PutVectors' per-vector validation (services/service-com-amazonaws-s3vectors-s3vectors/src/ops/put_vectors.rs and the dimension check it calls in crate::state): refuse with the element-level path, the service's sentence, and a top-level message prefixed with the offending record's key.
Traces: s3vectors-duplicate-bucket-and-invalid-put-errors#task#4 (cluster of 2, also s3vectors-put-vectors-dimension-mismatch-error#task#3).

=== Cause 6 of 6: PutVectorBucketPolicy's malformed-policy refusal leaks the parser's own diagnostic ===
AWS: Smithy PutVectorBucketPolicy's `policy` member is "the JSON that defines the policy"; a document that is not a well-formed policy is refused with HTTP 400 ValidationException and message "The provided policy is malformed or invalid" — one fixed sentence that reveals nothing about where parsing failed.
Emulator: HTTP 400 ValidationException with "`policy` is not a valid policy document: <parser detail>" (recorded instance: "... it is not well-formed JSON (key must be a string at line 1 column 2)"), built by `crate::state::validate_bucket_policy` (src/state.rs:447-508), which composes a member-named prefix with the serde_json error text.
Why: the status and code are right, but the text is an internal parser diagnostic, not the service's sentence; a client asserting or displaying AWS's wording diverges, and the leaked line/column detail is a shape AWS never emits for this error class.
Fix: `validate_bucket_policy` in crate::state (and PutVectorBucketPolicy's handler) — refuse with the single fixed sentence regardless of which grammar rule failed.
Traces: s3vectors-vector-bucket-policy-error-paths#task#2 (cluster of 2, also s3vectors-vector-bucket-policy-validation-error#task#2).

## Evidence
Cause 1 — CreateVectorBucket with encryptionConfiguration {sseType: 'aws:kms'} and no kmsKeyArn: expected HTTP 400, x-amzn-errortype 'ValidationException', $.message 'kmsKeyArn must be specified when sseType is set to aws:kms', $.fieldList [{path: 'EncryptionConfiguration', message: 'kmsKeyArn must be specified when sseType is set to aws:kms'}]; actual HTTP 200 with the bucket created, no message, no fieldList, no errortype header.
Cause 2 — CreateVectorBucket(vectorBucketName='Invalid_Bucket_Name'): expected 400 $.message and $.fieldList[0].message 'Invalid vector bucket name'; actual 400 with '`vectorBucketName` must be 3 to 63 characters of lowercase letters, numbers, hyphens and dots, and must begin and end with a letter or a number: <name>'.
Cause 3 — CreateIndex(dimension above 4096): expected 400 $.message "1 validation error detected. Value at '/dimension' failed to satisfy constraint: Member must be between 1 and 4096, inclusive" and $.fieldList [{path: '/dimension', message: same}]; actual 400 "1 validation error detected: Value '<value>' at 'dimension' failed to satisfy constraint: Member must have value less than or equal to 4096" with $.fieldList absent (None).
Cause 4 — ListVectorBuckets(maxResults above 500): expected 400 $.message "1 validation error detected. Value at '/maxResults' failed to satisfy constraint: Member must be between 1 and 500, inclusive" with the matching $.fieldList entry; actual 400 "1 validation error detected: Value '<value>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 500", $.fieldList absent.
Cause 5 — PutVectors with a vector whose data length != the index dimension: expected 400 $.fieldList[0].path 'vectors[0]', $.fieldList[0].message 'vector must have length <d>, but has length <n>', $.message "Invalid record for key '<key>': vector must have length <d>, but has length <n>"; actual 400 $.fieldList[0].path 'vectors[0].data' and both messages '`vectors[0].data` has <n> values, but vector index <indexName> is configured for <d> dimensions'.
Cause 6 — PutVectorBucketPolicy with a malformed policy document: expected 400 $.message 'The provided policy is malformed or invalid'; actual 400 '`policy` is not a valid policy document: it is not well-formed JSON (<parser detail>)'.

## Steps to Reproduce
1. git_clone, then replay the simparity-wire-aws-v1 wire corpus against the built emulator (replay_wire_corpus).
2. Cause 1: CreateVectorBucket(vectorBucketName=<fresh>, encryptionConfiguration={sseType:'aws:kms'}) with no kmsKeyArn — observe HTTP 200 instead of 400 ValidationException (fixture s3vectors-error-invalid-encryption-configuration, step task#1).
3. Cause 2: CreateVectorBucket(vectorBucketName='Invalid_Bucket_Name') — read $.message / $.fieldList[0].message (fixture s3vectors-error-invalid-bucket-name, step task#1).
4. Cause 3: CreateIndex(dimension=5000, dataType/distanceMetric valid) — read $.message and $.fieldList (fixture s3vectors-error-invalid-dimension-validation, step task#2).
5. Cause 4: ListVectorBuckets(maxResults=501) — read $.message and $.fieldList (fixture s3vectors-vector-bucket-list-empty-and-validation, step task#2).
6. Cause 5: create an index with dimension 4, then PutVectors with a vector whose key is 'bad-dim' and whose data has 2 values — read $.message and $.fieldList[0] (fixture s3vectors-duplicate-bucket-and-invalid-put-errors, step task#4).
7. Cause 6: PutVectorBucketPolicy with a body that is not well-formed JSON — read $.message (fixture s3vectors-vector-bucket-policy-error-paths, step task#2).
