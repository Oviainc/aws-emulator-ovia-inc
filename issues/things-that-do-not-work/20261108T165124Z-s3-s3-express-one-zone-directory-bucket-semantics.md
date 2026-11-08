Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.put-object-encryption-type-mismatch-rejected s3.s3x-s3express-session-and-listing-readonly-session-put-denied s3.s3x-s3express-session-and-listing-inflight-mpu-prefix-listed
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.put-object-encryption-type-mismatch-rejected|_error_code#15|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: s3.s3x-s3express-session-and-listing-readonly-session-put-denied|_err#10|AssertionError|raise AssertionError(f"expected the call to be refused, it succeeded: {brief!r}")
Issue-Key: s3.s3x-s3express-session-and-listing-inflight-mpu-prefix-listed|run#9|AssertionError|assert dir_prefixes == ["inflight/", "settled/"], (
Issue-Commit: 0a1e14ce9c5d04d4281ea317c037d5ece999a2b8

# s3: S3 Express One Zone directory-bucket semantics diverge — mismatched-SSE append accepted, ReadOnly session token not enforced, in-flight MPU prefix missing from ListObjectsV2 (3 causes)

> _Note: this issue was identified against the code base at commit `0a1e14ce9c5d04d4281ea317c037d5ece999a2b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Three net-new root causes in the emulator's S3 Express One Zone (directory bucket) surface. All three are in the `s3` service and each needs closing before this issue is submitted.

Cause 1 of 3 — PutObject append with an SSE type that differs from the write session's committed type is accepted instead of refused.
AWS: a write to a directory-bucket object goes through a session whose server-side-encryption type is committed when the session is established. A PutObject that names a different SSE type than that session's is refused 400 InvalidRequest. Smithy: PutObject models errors InvalidRequest (httpError 400) and EncryptionTypeMismatch (httpError 400); PutObject.WriteOffsetBytes binds header `x-amz-write-offset-bytes`, PutObject.ServerSideEncryption binds `x-amz-server-side-encryption`, PutObject.SSEKMSKeyId binds `x-amz-server-side-encryption-aws-kms-key-id`. The refusal is InvalidRequest — NOT EncryptionTypeMismatch — and its message is fixed text with the two algorithms substituted: "Session was created with ServerSideEncryption as <session-sse> but request header specifies <requested-sse>." For a default-encrypted (SSE-S3) directory bucket receiving an SSE-KMS append that reads "Session was created with ServerSideEncryption as AES256 but request header specifies SSE-KMS." The variable parts are the session's committed algorithm and the algorithm named by the request header; everything else is fixed. The object is left unchanged, and the identical append re-issued with the matching encryption type succeeds.
Emulator: the mismatched append is accepted with HTTP 200 and the response reports ServerSideEncryption `aws:kms`, an SSEKMSKeyId naming the caller's CMK and x-amz-object-size — i.e. the object's committed encryption type is silently switched per request and no refusal is produced at all.
Why: a client that relies on S3 Express rejecting an encryption switch mid-append sees a 200 and believes the object is still SSE-S3 while the emulator reports it as SSE-KMS; code paths that handle InvalidRequest (re-establishing the session with the right SSE type) are never exercised, and the object's encryption state becomes a function of the last writer.
Fix: the PutObject handler's directory-bucket/session write path — the check of the request's SSE headers against the session's committed encryption type, and the InvalidRequest refusal it must raise before any bytes are applied.
Traces: s3.put-object-encryption-type-mismatch-rejected.

Cause 2 of 3 — a CreateSession token minted with SessionMode=ReadOnly is not enforced: PutObject signed with it succeeds.
AWS: CreateSession (Smithy: GET /{Bucket}?session, input SessionMode bound to header `x-amz-create-session-mode`, output Credentials required) mints scoped session credentials. A ReadOnly session may read the bucket but any write signed with it is refused 403 AccessDenied with message "Access Denied" — fixed text, no variable part. The addressed object keeps the bytes written through a ReadWrite session, and the byte-identical PutObject through a ReadWrite token succeeds.
Emulator: PutObject signed with the ReadOnly session's credentials succeeds with HTTP 200 (response carries ETag, ChecksumCRC32, ChecksumType FULL_OBJECT, ServerSideEncryption AES256) and overwrites the object; no 403 and no AccessDenied are produced.
Why: SessionMode is decorative — a caller handed a read-only directory-bucket session can mutate data, so no least-privilege boundary built on ReadOnly sessions holds, and a client cannot distinguish a read-only token from a read-write one.
Fix: CreateSession's handler must record the requested SessionMode on the minted session, and the directory-bucket data-plane authorizer must refuse write operations presented with a ReadOnly session.
Traces: s3.s3x-s3express-session-and-listing-readonly-session-put-denied.

Cause 3 of 3 — ListObjectsV2 on a directory bucket omits the common prefix of an in-flight multipart upload.
AWS: in a directory bucket the key of an in-progress multipart upload is already part of the bucket's listing namespace: ListObjectsV2 with Delimiter="/" returns that key's leading prefix in CommonPrefixes (the key itself is absent from Contents), and the prefix disappears once the upload is aborted or completed. A general-purpose bucket does not behave this way — that asymmetry is the behaviour under test. Smithy: ListObjectsV2 output CommonPrefixes (CommonPrefixList, xmlFlattened) alongside Contents.
Emulator: with an MPU open on key `inflight/big.bin` and one settled object under `settled/`, ListObjectsV2 Delimiter="/" returned CommonPrefixes holding only the settled prefix; the in-flight upload's prefix is never listed, so the directory bucket is indistinguishable from its general-purpose twin.
Why: tooling that discovers in-flight uploads by listing prefixes (upload reconciliation, orphan-part sweepers, directory-style browsers) sees nothing in flight and concludes there is no pending work.
Fix: the ListObjectsV2 common-prefix computation for directory buckets — it must union the in-flight multipart-upload key namespace into the prefix roll-up instead of listing settled objects only.
Traces: s3.s3x-s3express-session-and-listing-inflight-mpu-prefix-listed.

## Evidence
Cause 1 — expected: ClientError 400 InvalidRequest, message "Session was created with ServerSideEncryption as AES256 but request header specifies SSE-KMS."; actual: HTTP 200 with ServerSideEncryption='aws:kms', SSEKMSKeyId=<the request's CMK ARN>, Size=25. Replay detail: `AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {... 'HTTPStatusCode': 200, 'x-amz-server-side-encryption': 'aws:kms' ...}`.

Cause 2 — expected: ('AccessDenied', 403, 'Access Denied') from PutObject signed with the ReadOnly session; actual: success. Replay detail: `AssertionError: expected the call to be refused, it succeeded: {'ETag': '"6c2191d826ccdfc13153ac442e3006ee"', 'ChecksumCRC32': 'RG65Tw==', 'ChecksumType': 'FULL_OBJECT', 'ServerSideEncryption': 'AES256'}`.

Cause 3 — expected CommonPrefixes ['inflight/', 'settled/'] from ListObjectsV2(Delimiter='/') on the directory bucket while an MPU is open on 'inflight/big.bin'; actual ['settled/']. Replay detail: `AssertionError: directory bucket with an open MPU listed prefixes ['settled/'], expected ['inflight/', 'settled/']`.

## Steps to Reproduce
1. Create an S3 Express One Zone directory bucket: CreateBucket with CreateBucketConfiguration={Location:{Type:'AvailabilityZone',Name:<az-id>}, Bucket:{DataRedundancy:'SingleAvailabilityZone', Type:'Directory'}}.
2. Cause 1: PutObject a seed body (implicit SSE-S3), append with WriteOffsetBytes and no SSE headers (succeeds), then PutObject again with WriteOffsetBytes=<current size>, ServerSideEncryption='aws:kms' and SSEKMSKeyId=<a KMS CMK>. Expect 400 InvalidRequest with the session/SSE-mismatch message; observe 200.
3. Cause 2: CreateSession(Bucket, SessionMode='ReadWrite') and CreateSession(Bucket, SessionMode='ReadOnly'); write a key with the ReadWrite credentials, then PutObject the same key with the ReadOnly credentials (pinned, so botocore cannot re-mint). Expect 403 AccessDenied / 'Access Denied'; observe 200 and the object overwritten.
4. Cause 3: PutObject 'settled/a.txt', CreateMultipartUpload on 'inflight/big.bin' and leave it open, then ListObjectsV2(Bucket, Delimiter='/'). Expect CommonPrefixes ['inflight/','settled/'] on the directory bucket (and ['settled/'] on a general-purpose twin); observe ['settled/'] on both.
5. Replay: s3.put-object-encryption-type-mismatch-rejected, s3.s3x-s3express-session-and-listing-readonly-session-put-denied, s3.s3x-s3express-session-and-listing-inflight-mpu-prefix-listed.
