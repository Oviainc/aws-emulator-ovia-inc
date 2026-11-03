Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-csv-get-with-sse-c-headers-on-a-kms-object-rejected-invalid-request s3.sse-c-wrong-customer-key-rejected s3.s3-default-encryption-change-applies-only-to-objects-written-after-it s3.sseinv-default-encryption-with-zero-or-two-rules-rejected s3.sseinv-put-object-aes256-with-a-kms-key-id-rejected s3.presigned-get s3.s3-csv-presigned-sigv2-url-for-an-sse-kms-object-rejected-invalid-argument s3.ssl-deny-bucket-policy-plain-http-head-refusal-carries-no-body s3.sigv4skew-past-dated-put-outside-window-rejected s3.object-annotation-validation-errors-rejected s3.put-object-annotation-limits s3.s3-eu-location-constraint-reads-back-as-eu-while-listing-names-eu-west-1 s3.s3-old-buckets-bucket-region-filter-on-a-foreign-endpoint-rejected s3.s3-old-buckets-head-bucket-with-a-foreign-expected-owner-rejected s3.s3-old-buckets-list-buckets-unknown-bucket-region-rejected s3.s3-old-buckets-object-writes-do-not-move-creation-date s3.s3-csv-get-with-part-number-and-range-together-rejected-invalid-request s3.s3-csv-get-part-number-past-a-single-part-object-rejected-invalid-part-number s3.s3unicode-list-objects-with-an-unsupported-encoding-type-is-rejected s3.e2e-list-s3-bucket-subdirectories-lifecycle s3.s3-pab-e2e-check-buckets-public-access-lifecycle s3.s3-oc-e2e-get-bucket-ownership-controls-lifecycle s3.s3-csv-e2e-retrieve-csv-object-contents-lifecycle s3.s3-lifecycle-tag-e2e-tag-filter-expiration-lifecycle
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.s3-csv-get-with-sse-c-headers-on-a-kms-object-rejected-invalid-request|run#7|AssertionError|assert (code, status, msg) == ("InvalidRequest", 400, _NOT_APPLICABLE), (
Issue-Key: s3.sse-c-wrong-customer-key-rejected|run#17|AssertionError|assert code == "AccessDenied", f"expected AccessDenied for a blocked SSE-C PutObject, got {code!r}"
Issue-Key: s3.s3-default-encryption-change-applies-only-to-objects-written-after-it|run#8|AssertionError|assert re.fullmatch(key_arn, key_id), f"the SSE-KMS object must name a key ARN; got {key_id!r}"
Issue-Key: s3.sseinv-default-encryption-with-zero-or-two-rules-rejected|_error#7|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error to read")
Issue-Key: s3.sseinv-put-object-aes256-with-a-kms-key-id-rejected|run#5|AssertionError|assert message == _KEY_WITH_AES_MSG, (
Issue-Key: s3.presigned-get|run#11|AssertionError|raise AssertionError(
Issue-Key: s3.s3-csv-presigned-sigv2-url-for-an-sse-kms-object-rejected-invalid-argument|run#10|AssertionError|assert (status, body) == (200, _CSV), (
Issue-Key: s3.ssl-deny-bucket-policy-plain-http-head-refusal-carries-no-body|run#13|AssertionError|assert tls_get.status_code == 200 and tls_get.content == INDEX_HTML, (
Issue-Key: s3.sigv4skew-past-dated-put-outside-window-rejected|run#11|AssertionError|assert err["Code"] == "RequestTimeTooSkewed", (
Issue-Key: s3.object-annotation-validation-errors-rejected|run#17|AssertionError|assert (not_found_code, not_found_message) == (code, message), (
Issue-Key: s3.put-object-annotation-limits|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: s3.s3-eu-location-constraint-reads-back-as-eu-while-listing-names-eu-west-1|run#6|AssertionError|assert locations == {legacy: "EU", modern: _EU_REGION}, (
Issue-Key: s3.s3-old-buckets-bucket-region-filter-on-a-foreign-endpoint-rejected|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: s3.s3-old-buckets-head-bucket-with-a-foreign-expected-owner-rejected|_error#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: s3.s3-old-buckets-list-buckets-unknown-bucket-region-rejected|_error#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: s3.s3-old-buckets-object-writes-do-not-move-creation-date|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: s3.s3-csv-get-with-part-number-and-range-together-rejected-invalid-request|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: s3.s3-csv-get-part-number-past-a-single-part-object-rejected-invalid-part-number|run#13|AssertionError|assert head == ("416", 416, "Range Not Satisfiable"), (
Issue-Key: s3.s3unicode-list-objects-with-an-unsupported-encoding-type-is-rejected|run#15|AssertionError|assert message == "Invalid Encoding Method specified in Request", message
Issue-Key: s3.e2e-list-s3-bucket-subdirectories-lifecycle|phase#5|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: s3.s3-pab-e2e-check-buckets-public-access-lifecycle|phase#4|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: s3.s3-oc-e2e-get-bucket-ownership-controls-lifecycle|phase#5|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: s3.s3-csv-e2e-retrieve-csv-object-contents-lifecycle|phase#4|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: s3.s3-lifecycle-tag-e2e-tag-filter-expiration-lifecycle|phase#4|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 56c3e2d5db4619f120a986f75a3622f8f9af8979

# s3: 10 more net-new root causes — SSE-C unimplemented, SSE-KMS key ARN absent, presigned/query-credential requests refused 403, clock-skew and parameter-validation refusals wrong, CloudFormation bucket properties not reproduced

> _Note: this issue was identified against the code base at commit `56c3e2d5db4619f120a986f75a3622f8f9af8979`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Ten further distinct net-new root causes under `### s3`, covering the encryption surface, the request-signing surface, parameter validation and the CloudFormation-provisioned bucket state. Close all ten before submitting.

Cause 1 of 10: SSE-C is an unimplemented scaffold on the object plane.
AWS: Smithy — GetObject/PutObject/CreateMultipartUpload/UploadPartCopy bind `SSECustomerAlgorithm` (`x-amz-server-side-encryption-customer-algorithm`), `SSECustomerKey` and `SSECustomerKeyMD5`. Presenting SSE-C headers for an object encrypted with SSE-KMS is refused HTTP 400 `<Code>InvalidRequest</Code>` with the fixed message "The encryption parameters are not applicable to this object."; presenting the WRONG customer key for an SSE-C object is refused HTTP 403 `<Code>AccessDenied</Code>`.
Emulator: every SSE-C-bearing request is refused HTTP 501 `<Code>NotImplemented</Code>` with "operation <Op> with x-amz-server-side-encryption-customer-algorithm is scaffolded but not implemented in this emulator" (and the CreateMultipartUpload variant with "with SSECustomerAlgorithm").
Why: no customer-managed-key workflow can be exercised at all, and a client that distinguishes "wrong key" (403) from "wrong encryption family" (400) sees neither.
Fix: SSE-C support in the GetObject / PutObject / CreateMultipartUpload / UploadPartCopy handlers and their encryption-parameter validation.
Traces: s3.s3-csv-get-with-sse-c-headers-on-a-kms-object-rejected-invalid-request, s3.sse-c-wrong-customer-key-rejected.

Cause 2 of 10: SSE-KMS objects report no key ARN, and PutBucketEncryption/PutObject encryption-parameter validation is missing or mis-worded.
AWS: recorded behavior. An object written under an SSE-KMS bucket default or an explicit `x-amz-server-side-encryption: aws:kms` reports `x-amz-server-side-encryption-aws-kms-key-id` as a full key ARN in this account and region on both HeadObject and GetObject. A ServerSideEncryptionConfiguration carrying zero Rules, or two Rules, is refused HTTP 400 (one rule exactly). `x-amz-server-side-encryption-aws-kms-key-id` sent alongside `x-amz-server-side-encryption: AES256` is refused HTTP 400 `<Code>InvalidArgument</Code>` with the fixed message "Server Side Encryption with AWS KMS managed key requires HTTP header x-amz-server-side-encryption : aws:kms".
Emulator: the SSE-KMS object's key id comes back as the empty string; the zero-rule/two-rule configuration is accepted ("the call succeeded; real AWS rejects it, so there is no error to read"); and the AES256+kms-key-id refusal message is 'x-amz-server-side-encryption-aws-kms-key-id is only valid with a server-side encryption algorithm of aws:kms or aws:kms:dsse'.
Why: a client cannot discover which CMK an object is under, so key rotation and cross-account grant tooling has nothing to act on; and an encryption configuration AWS refuses is stored and read back as valid.
Fix: the SSE-KMS key resolution written into object metadata and surfaced by Head/GetObject, plus PutBucketEncryption / PutObject encryption-parameter validation and its message.
Traces: s3.s3-default-encryption-change-applies-only-to-objects-written-after-it, s3.sseinv-default-encryption-with-zero-or-two-rules-rejected, s3.sseinv-put-object-aes256-with-a-kms-key-id-rejected.

Cause 3 of 10: requests whose credentials ride in the query string, and plain requests to the TLS endpoint, are refused 403 "Sign the request with the emulator credentials".
AWS: recorded behavior. A SigV4 presigned GET URL within its expiry serves the object with HTTP 200 and its bytes; a SigV2 query-credential (`AWSAccessKeyId`/`Signature`/`Expires`) URL serves an SSE-S3 object with HTTP 200 and is refused `InvalidArgument` only for an SSE-KMS object; a request over TLS to the REST endpoint under an aws:SecureTransport-deny policy is ALLOWED (it is the plaintext one that is denied).
Emulator: the presigned GET URLs answer HTTP 403 Forbidden, and the SigV2 and TLS positive controls answer HTTP 403 with the body `<Error><Code>AccessDenied</Code><Message>Sign the request with the emulator credentials</Message>...` — a message that exists nowhere in S3. The emulator is rejecting any request whose credential is not in the Authorization header.
Why: browser upload/download flows, pre-signed-link distribution and every SecureTransport policy test are unreachable; the 403 is indistinguishable from a real authorization failure.
Fix: the request-authentication path's query-credential (presigned SigV4 and legacy SigV2) branch. The open issue 20261027T190000Z-s3-legacy-query-credential-presigned-get.md is adjacent but does not carry these replay keys.
Traces: s3.presigned-get, s3.s3-csv-presigned-sigv2-url-for-an-sse-kms-object-rejected-invalid-argument, s3.ssl-deny-bucket-policy-plain-http-head-refusal-carries-no-body.

Cause 4 of 10: a SigV4 request dated outside the signing window is refused SignatureDoesNotMatch instead of RequestTimeTooSkewed.
AWS: recorded behavior. A SigV4 request whose `X-Amz-Date` is outside the permitted clock-skew window is refused HTTP 403 `<Code>RequestTimeTooSkewed</Code>` — the skew check runs BEFORE signature comparison, so the caller learns its clock is wrong rather than that its key is wrong.
Emulator: answers `SignatureDoesNotMatch` with "The request signature we calculated does not match the signature you provided. Check your key and signing method."
Why: a client with a drifting clock is told to rotate credentials instead of to resync time — the standard remediation path is inverted.
Fix: the clock-skew pre-check in the SigV4 verifier, ordered ahead of signature comparison.
Traces: s3.sigv4skew-past-dated-put-outside-window-rejected.

Cause 5 of 10: the object-annotation APIs accept invalid names and answer the wrong error for an over-long name.
AWS: recorded behavior. PutObjectAnnotation refuses an annotation name outside the allowed character set (e.g. one containing `!`) rather than storing it; GetObjectAnnotation for a name that is too long is refused with the SAME code and message as the put, `AnnotationNameTooLong` / "Your annotation name is too long.", not with a not-found error.
Emulator: PutObjectAnnotation with name `bad!name` returns 200 and echoes `AnnotationName: 'bad!name'`; GetObjectAnnotation for the too-long name answers `('NoSuchAnnotation', 'The specified annotation does not exist.')`.
Why: an invalid annotation is stored and then reads back as "absent", so a caller sees a write succeed and the read deny its existence.
Fix: the shared annotation-name validator used by PutObjectAnnotation / GetObjectAnnotation and the read handler's error mapping.
Traces: s3.object-annotation-validation-errors-rejected, s3.put-object-annotation-limits.

Cause 6 of 10: GetBucketLocation does not echo the legacy `EU` LocationConstraint.
AWS: Smithy — GetBucketLocation GET /{Bucket}?location, output `LocationConstraint` targets the `BucketLocationConstraint` enum whose value list includes the legacy value `EU` alongside `eu-west-1`. GetBucketLocation echoes the constraint the bucket was CREATED with, so a bucket created with `LocationConstraint=EU` reads back `EU` while ListBuckets names its region `eu-west-1`.
Emulator: both buckets read back `eu-west-1`; the create-time constraint is normalised away and the legacy value can never be observed.
Why: a client keyed on the legacy `EU` constraint (older SDKs and endpoint-resolution code) resolves the wrong endpoint form.
Fix: persist the create-time LocationConstraint in the bucket state and echo it from the GetBucketLocation handler.
Traces: s3.s3-eu-location-constraint-reads-back-as-eu-while-listing-names-eu-west-1.

Cause 7 of 10: ListBuckets / HeadBucket parameter validation is missing, and a configuration write moves the bucket creation date.
AWS: recorded behavior. ListBuckets with a `bucket-region` that is not a real region, and with a region filter that does not match the endpoint being addressed, is refused HTTP 400. HeadBucket with an `x-amz-expected-bucket-owner` naming an account that does not own the bucket is refused HTTP 403 `<Code>AccessDenied</Code>`. A bucket's `CreationDate` is set once at CreateBucket and is NOT advanced by object writes or by configuration writes.
Emulator: all three validation cases succeed ("the call succeeded; real AWS rejects it, so there is no error"), and the creation-date check never converges (`Error: Check timed out after 60.0s`), i.e. the observed CreationDate does not hold still.
Why: the confused-deputy guard is absent on HeadBucket, a bogus region filter silently returns everything, and inventory tooling that sorts or diffs on CreationDate sees buckets change age.
Fix: ListBuckets / HeadBucket input validation and the bucket-state CreationDate (set at create, never rewritten).
Traces: s3.s3-old-buckets-bucket-region-filter-on-a-foreign-endpoint-rejected, s3.s3-old-buckets-head-bucket-with-a-foreign-expected-owner-rejected, s3.s3-old-buckets-list-buckets-unknown-bucket-region-rejected, s3.s3-old-buckets-object-writes-do-not-move-creation-date.

Cause 8 of 10: GetObject accepts partNumber together with Range, and the out-of-range refusal uses the wrong reason phrase.
AWS: recorded behavior. `partNumber` and `Range` on the same GetObject/HeadObject are mutually exclusive and the request is refused HTTP 400 `<Code>InvalidRequest</Code>`. A `partNumber` past the number of parts of the object is refused HTTP 416 whose reason phrase is `Range Not Satisfiable` (and, for HEAD, there is no body so the reason phrase is all the client has).
Emulator: the partNumber+Range request SUCCEEDS ("the call succeeded; real AWS rejects it, so there is no error code"), and the 416 carries the reason phrase `Requested Range Not Satisfiable`.
Why: a caller combining both parameters gets an undefined slice of the object rather than an error, and a HEAD client keyed on the 416 reason phrase misreports the failure.
Fix: GetObject/HeadObject partNumber/Range validation and the 416 status line in the error serializer.
Traces: s3.s3-csv-get-with-part-number-and-range-together-rejected-invalid-request, s3.s3-csv-get-part-number-past-a-single-part-object-rejected-invalid-part-number.

Cause 9 of 10: an unsupported `encoding-type` is refused with a Smithy constraint message instead of S3's own refusal.
AWS: Smithy — the `EncodingType` enum admits exactly one value, `url`; ListObjects/ListObjectsV2 bind it to the `encoding-type` query parameter. A value outside that set is refused with S3's own REST-XML error, not with a Smithy enum-constraint sentence.
Emulator: answers the Smithy framework message `Value 'utf8' at 'encodingType' failed to satisfy constraint: Member must satisfy enum value set: [url]`, which names a member path no S3 client knows.
Why: a client parsing the `<Error>` body for an S3 code and message gets a shape it cannot interpret, and the failure is attributed to the SDK rather than to the request.
Fix: map the EncodingType constraint failure to S3's own error code and message in the listing handlers' input deserializer.
Traces: s3.s3unicode-list-objects-with-an-unsupported-encoding-type-is-rejected.

Cause 10 of 10: CloudFormation-provisioned AWS::S3::Bucket properties are not reproduced on the resulting bucket.
AWS: recorded behavior. A CloudFormation-created bucket reads back exactly what the template declared: a `PublicAccessBlockConfiguration` that names only three members leaves the omitted member at its explicit `false` (GetPublicAccessBlock returns `BlockPublicAcls: false` where the template omitted it); `OwnershipControls` declaring `ObjectWriter` reads back `ObjectWriter`; a `BucketEncryption` declaring an `aws:kms` default with no KeyId reads back `SSEAlgorithm: aws:kms`; and CloudFormation's flat one-entry `TagFilters` on a lifecycle rule surfaces through the S3 API as `{'And': {'Prefix': '', 'Tags': [...]}}` — an `And` with an EMPTY Prefix — not as `Filter.Tag`.
Emulator: the omitted PublicAccessBlock member comes back `true` (all four true), the `ObjectWriter` bucket reads back `BucketOwnerEnforced`, the declared `aws:kms` default reads back `AES256` (with an unexpected `BlockedEncryptionTypes` member), and the one-entry TagFilters surfaces as `Filter.Tag`.
Why: a stack that deploys successfully leaves a bucket whose public-access posture, ownership model, encryption and lifecycle scope all differ from the template — the deployed estate cannot be audited against its own source.
Fix: the AWS::S3::Bucket resource provider's property translation into S3 bucket state (public-access-block defaulting, ownership controls, bucket encryption, lifecycle TagFilters → And/Prefix+Tags).
Traces: s3.e2e-list-s3-bucket-subdirectories-lifecycle, s3.s3-pab-e2e-check-buckets-public-access-lifecycle, s3.s3-oc-e2e-get-bucket-ownership-controls-lifecycle, s3.s3-csv-e2e-retrieve-csv-object-contents-lifecycle, s3.s3-lifecycle-tag-e2e-tag-filter-expiration-lifecycle.

## Evidence
Cause 1 — "SSE-C headers on an SSE-KMS object must be 400 InvalidRequest/'The encryption parameters are not applicable to this object.'; got 'NotImplemented'/501/'operation GetObject with x-amz-server-side-encryption-customer-algorithm is scaffolded but not implemented in this emulator'"; "expected AccessDenied for a blocked SSE-C PutObject, got 'NotImplemented'".
Cause 2 — "the SSE-KMS object must name a key ARN; got ''"; "the call succeeded; real AWS rejects it, so there is no error to read" (zero/two rules); "the InvalidArgument refusal must carry the exact message 'Server Side Encryption with AWS KMS managed key requires HTTP header x-amz-server-side-encryption : aws:kms'; got 'x-amz-server-side-encryption-aws-kms-key-id is only valid with a server-side encryption algorithm of aws:kms or aws:kms:dsse'".
Cause 3 — "DATAPLANE_UNIMPLEMENTED: presigned GET URL did not serve the object: HTTPError: HTTP Error 403: Forbidden"; "positive control: a SigV2 URL must read an SSE-S3 object; got 403 b'<?xml version=\"1.0\"?><Error><Code>AccessDenied</Code><Message>Sign the request with the emulator credentials</Message>...'"; "the TLS GET was refused or wrong: 403 b'...<Code>AccessDenied</Code><Message>Sign the request with the emulator credentials</Message>...'".
Cause 4 — "expected RequestTimeTooSkewed, got 'SignatureDoesNotMatch' ('The request signature we calculated does not match the signature you provided. Check your key and signing method.')".
Cause 5 — "expected the too-long name to be rejected identically by GetObjectAnnotation ('AnnotationNameTooLong', 'Your annotation name is too long.'), got ('NoSuchAnnotation', 'The specified annotation does not exist.')"; "expected the call to be rejected, but it SUCCEEDED and returned {... 'AnnotationName': 'bad!name'}".
Cause 6 — "GetBucketLocation must echo the constraint each bucket was created with; got {'...-eu': 'eu-west-1', '...-west': 'eu-west-1'}".
Cause 7 — three × "the call succeeded; real AWS rejects it, so there is no error"; `Error: Check timed out after 60.0s` on s3.s3-old-buckets-object-writes-do-not-move-creation-date.
Cause 8 — "the call succeeded; real AWS rejects it, so there is no error code"; "expected ('416', 416, 'Range Not Satisfiable'), got ('416', 416, 'Requested Range Not Satisfiable')".
Cause 9 — "Value 'utf8' at 'encodingType' failed to satisfy constraint: Member must satisfy enum value set: [url]".
Cause 10 — "[phase 1/5 deploy] the deployed public-access block is {'BlockPublicAcls': True, 'IgnorePublicAcls': True, 'BlockPublicPolicy': True, 'RestrictPublicBuckets': True}, expected {'BlockPublicAcls': False, ...}"; "[phase 1/4 deploy] CloudFormation's three-member PublicAccessBlockConfiguration must surface with the omitted BlockPublicAcls as an explicit False; the API reports {... all True}"; "declared {...'-three': 'ObjectWriter'}, derived {...'-three': 'BucketOwnerEnforced'}"; "the stack must enable versioning and an aws:kms default naming no key; got 'Enabled', [{'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'AES256'}, 'BucketKeyEnabled': False, 'BlockedEncryptionTypes': {'EncryptionType': ['SSE-C']}}]"; "CloudFormation's flat one-entry TagFilters must surface as {'And': {'Prefix': '', 'Tags': [{'Key': 'shouldDelete', 'Value': 'true'}]}} ... the API reports {'Tag': {'Key': 'shouldDelete', 'Value': 'true'}}".

## Steps to Reproduce
Boot the emulator and re-replay exactly the check ids in trace_ids. Representative manual repros:
1. PutObject with x-amz-server-side-encryption-customer-algorithm=AES256 — expect it to work (and the wrong-key read to be 403 AccessDenied); observe 501 NotImplemented.
2. PutBucketEncryption with an aws:kms default naming no key, PutObject, then HeadObject — expect x-amz-server-side-encryption-aws-kms-key-id to be a key ARN; observe an empty value. PutBucketEncryption with zero or two Rules — expect 400.
3. generate_presigned_url('get_object') and fetch it — expect 200 and the object's bytes; observe 403 'Sign the request with the emulator credentials'.
4. Sign a PUT with X-Amz-Date well outside the skew window — expect 403 RequestTimeTooSkewed; observe SignatureDoesNotMatch.
5. PutObjectAnnotation with name 'bad!name' — expect a refusal; observe 200. GetObjectAnnotation for an over-long name — expect AnnotationNameTooLong; observe NoSuchAnnotation.
6. CreateBucket with CreateBucketConfiguration{LocationConstraint: 'EU'}, then GetBucketLocation — expect 'EU'.
7. ListBuckets with bucket-region='not-a-region'; HeadBucket with a foreign ExpectedBucketOwner — expect 400 / 403; observe success. Write an object, re-read CreationDate — expect it unchanged.
8. GetObject with both PartNumber and Range — expect 400 InvalidRequest; observe success. HeadObject with PartNumber=2 on a single-part object — expect reason phrase 'Range Not Satisfiable'.
9. ListObjectsV2 with EncodingType='utf8' — expect S3's own error code/message, observe the Smithy constraint sentence.
10. Deploy a stack whose AWS::S3::Bucket declares a three-member PublicAccessBlockConfiguration, OwnershipControls=ObjectWriter, BucketEncryption aws:kms with no KeyId, and a lifecycle rule with one TagFilter; then read GetPublicAccessBlock / GetBucketOwnershipControls / GetBucketEncryption / GetBucketLifecycleConfiguration and compare to the template.
