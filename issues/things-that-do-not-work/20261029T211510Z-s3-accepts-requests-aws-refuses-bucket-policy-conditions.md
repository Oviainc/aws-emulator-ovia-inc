Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.encryption-enforcement-reject-unencrypted s3.bucket-policy-deny-enforcement s3.copy-object-missing-source-permission-denied s3.expected-bucket-owner-mismatch-rejected s3.create-bucket-invalid-name-rejected s3.error-invalid-bucket-name s3.deep-s3-storage-class-express-onezone-class-put-rejected s3.deep-s3-storage-class-outposts-class-put-rejected s3.deep-s3-storage-class-deep-archive-expedited-restore-rejected s3.s3unicode-a-nul-byte-in-an-object-key-is-rejected s3.object-key-1024-bytes-boundary-rejected s3.sigv4skew-past-dated-put-outside-window-rejected s3.bucket-policy-status-reflects-policy
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.encryption-enforcement-reject-unencrypted|_err#9|AssertionError|raise AssertionError(f"expected this call to fail, but it SUCCEEDED and returned {result!r}")
Issue-Key: s3.bucket-policy-deny-enforcement|_poll_until_refused#11|Timeout|time.sleep(interval_s)
Issue-Key: s3.copy-object-missing-source-permission-denied|_poll_for_error#16|AssertionError|raise AssertionError(f"expected {expected_code} within {timeout_s}s, last outcome: {last!r}")
Issue-Key: s3.expected-bucket-owner-mismatch-rejected|run#13|AssertionError|assert exc is not None, (
Issue-Key: s3.create-bucket-invalid-name-rejected|_error#15|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: s3.error-invalid-bucket-name|_err#12|AssertionError|raise AssertionError(f"{what} succeeded; expected a refusal")
Issue-Key: s3.deep-s3-storage-class-express-onezone-class-put-rejected|_error#7|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: s3.deep-s3-storage-class-outposts-class-put-rejected|_error#7|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: s3.deep-s3-storage-class-deep-archive-expedited-restore-rejected|_error#10|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: s3.s3unicode-a-nul-byte-in-an-object-key-is-rejected|run#15|AssertionError|assert exc is not None, (
Issue-Key: s3.object-key-1024-bytes-boundary-rejected|run#22|AssertionError|assert code == "KeyTooLongError", f"expected KeyTooLongError for a 1025-byte key, got {code!r}"
Issue-Key: s3.sigv4skew-past-dated-put-outside-window-rejected|run#9|AssertionError|assert status == 403, (
Issue-Key: s3.bucket-policy-status-reflects-policy|_error#5|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Commit: 482573dd66a8fdd59e0387c95d89889e67d3b0e1

# S3 accepts requests AWS refuses: bucket-policy conditions, CopyObject source authorization, ExpectedBucketOwner, bucket-name and storage-class validation, key bytes and SigV4 skew all unenforced

> _Note: this issue was identified against the code base at commit `482573dd66a8fdd59e0387c95d89889e67d3b0e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Ten net-new root causes, all of the same family: a request real S3 refuses is accepted with 2xx. A fixer must close all ten.

Cause 1 of 10: a bucket policy Deny is not evaluated on the object write path.
AWS: a bucket policy that Denies s3:PutObject unless the request carries the required encryption header refuses the unencrypted PutObject with AccessDenied / HTTP 403 / 'Access Denied'; an explicit Deny also beats any identity-based Allow.
Emulator: the PutObject succeeds with HTTP 200 (ETag, x-amz-checksum-crc32, x-amz-server-side-encryption present), and the companion deny-enforcement check never observes a refusal inside its window and is killed at the 60 s budget.
Why: a client relying on a bucket policy to make encryption (or any condition) mandatory gets data written in violation of that policy, and the policy appears to be installed while being inert.
Fix: the object-write authorization path -- bucket-policy evaluation (statement conditions and explicit Deny) must run before PutObject commits.
Traces: s3.encryption-enforcement-reject-unencrypted, s3.bucket-policy-deny-enforcement

Cause 2 of 10: CopyObject does not authorize the read of its copy source.
AWS: CopyObject requires s3:GetObject on x-amz-copy-source; without it the call is refused AccessDenied / 403 / 'Access Denied' and no destination object is created.
Emulator: CopyObject returns 200 with CopyObjectResult{ETag, LastModified} even though the caller has no read grant on the source.
Why: a principal that may write a bucket can exfiltrate any object it can name into it; a least-privilege boundary that exists on AWS does not exist here.
Fix: the CopyObject handler's authorization, which must evaluate the source read separately from the destination write.
Traces: s3.copy-object-missing-source-permission-denied

Cause 3 of 10: x-amz-expected-bucket-owner is ignored.
AWS: when ExpectedBucketOwner does not match the bucket's owner account, the call is refused AccessDenied / 403; the member is modeled on effectively every S3 operation (Smithy: ExpectedBucketOwner: AccountId bound to httpHeader x-amz-expected-bucket-owner on ListObjectsV2, AbortMultipartUpload, GetObjectTagging, DeleteObjects, UploadPart, RestoreObject, ...).
Emulator: ListObjectsV2 with ExpectedBucketOwner=000000000001 against a bucket owned by 000000000000 succeeds and returns the listing.
Why: the confused-deputy guard that header exists for is absent, so a caller cannot pin the account it believes it is talking to.
Fix: the shared request pre-check that compares ExpectedBucketOwner against the bucket's owner, applied on bucket and object operations alike.
Traces: s3.expected-bucket-owner-mismatch-rejected

Cause 4 of 10: CreateBucket performs no bucket-name validation.
AWS: a bucket name must be 3-63 characters, lowercase letters, digits, hyphens and dots only. An uppercase name is refused InvalidBucketName / 400 ('The specified bucket is not valid'); an over-length name is refused likewise. Nothing is created.
Emulator: CreateBucket with an uppercase name ('SIM2REAL-BEHAV-S3FID-...') returns 200, and so does a name far longer than 63 characters -- the 200 even echoes a Location header built from the illegal name.
Why: a client can create buckets it can never address virtual-hosted-style, and loses the early, local validation every SDK and AWS itself performs; names that are invalid everywhere else become real here.
Fix: the CreateBucket handler's name validation, which must apply the length and character rules before creating state.
Traces: s3.create-bucket-invalid-name-rejected, s3.error-invalid-bucket-name

Cause 5 of 10: PutObject accepts storage classes that are illegal for a general-purpose bucket.
AWS: x-amz-storage-class EXPRESS_ONEZONE is valid only on a directory bucket and OUTPOSTS only on an S3 on Outposts bucket; on a general-purpose bucket each is refused InvalidStorageClass / 400 and no object is created.
Emulator: both PutObjects succeed with 200 (ETag, checksum and SSE headers returned).
Why: the object is stored under a class the bucket cannot host, so later reads, lifecycle transitions and cost/behaviour assumptions all diverge from AWS, and an application bug that AWS rejects immediately is silently accepted.
Fix: the PutObject storage-class validation, which must gate each class on the bucket's type.
Traces: s3.deep-s3-storage-class-express-onezone-class-put-rejected, s3.deep-s3-storage-class-outposts-class-put-rejected

Cause 6 of 10: RestoreObject accepts a Tier that the object's storage class does not offer.
AWS: a DEEP_ARCHIVE object cannot be restored with GlacierJobParameters/Tier=Expedited -- the request is refused InvalidObjectState / 403; only Standard and Bulk are available for DEEP_ARCHIVE.
Emulator: the RestoreObject succeeds (HTTP 200, empty body).
Why: a caller is told an Expedited restore is under way for an object class that cannot do one, so its retrieval-time expectations and its error handling for the real refusal are both wrong.
Fix: the RestoreObject handler's tier/storage-class compatibility validation.
Traces: s3.deep-s3-storage-class-deep-archive-expedited-restore-rejected

Cause 7 of 10: a NUL byte in an object key is accepted.
AWS: refuses a key containing a NUL (0x00) byte; such a key can never be stored.
Emulator: PutObject with key 'nul/a\x00b.txt' succeeds.
Why: a key that no AWS client, console or listing can round-trip now exists in the bucket, and every consumer that assumes AWS's key-byte rules can be surprised by it.
Fix: the object-key validation shared by the object-write handlers.
Traces: s3.s3unicode-a-nul-byte-in-an-object-key-is-rejected

Cause 8 of 10: an over-length object key is refused with the wrong error code.
AWS: a key longer than 1024 bytes is refused KeyTooLongError (HTTP 400), with the 1024-byte key itself still accepted -- the boundary is exactly 1024.
Emulator: refuses with InvalidRequest instead of KeyTooLongError.
Why: a client branching on KeyTooLongError (to truncate or rename a key) never sees it and treats a well-understood limit violation as an unclassified bad request.
Fix: the object-key length validation's error classification.
Traces: s3.object-key-1024-bytes-boundary-rejected

Cause 9 of 10: SigV4 request-time skew is not enforced.
AWS: a request whose X-Amz-Date is outside the +/-15-minute window around S3's clock is refused 403 RequestTimeTooSkewed ('The difference between the request time and the current time is too large'), carrying the request time and the server time.
Emulator: a PUT signed 1500 s behind the server clock is accepted with HTTP 200 and a PutObjectOutput body.
Why: replayed or stale-clock requests that AWS rejects are honoured, so a client cannot test or rely on the skew guard, and signature freshness is unverifiable.
Fix: the SigV4 verification path, which must compare the signed date against the server clock and refuse outside the window.
Traces: s3.sigv4skew-past-dated-put-outside-window-rejected

Cause 10 of 10: GetBucketPolicyStatus answers 200 for a bucket with no policy.
AWS: when the bucket has no policy, GetBucketPolicyStatus is refused NoSuchBucketPolicy / 404 ('The bucket policy does not exist'); PolicyStatus.IsPublic is only reported once a policy exists.
Emulator: returns 200 with PolicyStatus{IsPublic: false}.
Why: a caller cannot distinguish 'no policy' from 'a policy that is not public', which is exactly the distinction this operation exists to make.
Fix: the GetBucketPolicyStatus handler, which must refuse when no bucket policy is stored.
Traces: s3.bucket-policy-status-reflects-policy

## Evidence
s3.encryption-enforcement-reject-unencrypted -> AssertionError: expected this call to fail, but it SUCCEEDED and returned {... 'HTTPStatusCode': 200, 'x-amz-server-side-encryption': 'aws:kms', 'ETag': '"a98474a85ee60329ad3831c4ea6b2503"' ...}
s3.bucket-policy-deny-enforcement -> Error: Check timed out after 60.0s (the expected refusal never appears)
s3.copy-object-missing-source-permission-denied -> AssertionError: expected AccessDenied within 20.0s, last outcome: {... 'HTTPStatusCode': 200 ..., 'CopyObjectResult': {'ETag': '"57ddf96cc1f9880bacccf1c1e4b2ea0e"', ...}}
s3.expected-bucket-owner-mismatch-rejected -> AssertionError: ListObjectsV2 with ExpectedBucketOwner=000000000001 (the bucket is owned by 000000000000) succeeded; the confused-deputy guard is not enforced on bucket ops
s3.error-invalid-bucket-name -> AssertionError: CreateBucket with an uppercase bucket name (bucket='SIM2REAL-BEHAV-S3FID-EBEE05671BD7') succeeded; expected a refusal
s3.create-bucket-invalid-name-rejected -> AssertionError: expected a ClientError, but the call SUCCEEDED and returned {... 'location': 'http://sim2real-behav-s3fid-1e0e6e2bb6e7zzzz...zzzz.s3.amazonaws.com/' ...}
s3.deep-s3-storage-class-express-onezone-class-put-rejected / -outposts-class-put-rejected -> AssertionError: expected a ClientError, but the call SUCCEEDED and returned {... 'HTTPStatusCode': 200, 'ETag': ..., 'ServerSideEncryption': 'AES256'}
s3.deep-s3-storage-class-deep-archive-expedited-restore-rejected -> AssertionError: expected a ClientError, but the call SUCCEEDED and returned {... 'HTTPStatusCode': 200 ...}
s3.s3unicode-a-nul-byte-in-an-object-key-is-rejected -> AssertionError: PutObject accepted a key containing a NUL byte ('nul/a\x00b.txt'); real S3 refuses it
s3.object-key-1024-bytes-boundary-rejected -> AssertionError: expected KeyTooLongError for a 1025-byte key, got 'InvalidRequest'
s3.sigv4skew-past-dated-put-outside-window-rejected -> AssertionError: a PUT signed 1500 s behind S3's clock must be refused 403, got 200: b'<PutObjectOutput xmlns="http://s3.amazonaws.com/doc/2006-03-01/"></PutObjectOutput>'
s3.bucket-policy-status-reflects-policy -> AssertionError: expected a ClientError, but the call succeeded and returned {... 'PolicyStatus': {'IsPublic': False}}

## Steps to Reproduce
Replay the behavioral suite and run the s3 checks in trace_ids. Each issues the single call named in its cause and asserts the refusal: PutObject against a bucket whose policy Denies unencrypted writes; CopyObject with a source the caller cannot read; ListObjectsV2 with a mismatched ExpectedBucketOwner; CreateBucket with an uppercase and an over-length name; PutObject with StorageClass EXPRESS_ONEZONE and OUTPOSTS on a general-purpose bucket; RestoreObject Tier=Expedited on a DEEP_ARCHIVE object; PutObject with a NUL byte in the key; PutObject with a 1025-byte key; a PUT signed 1500 s behind the server clock; GetBucketPolicyStatus on a bucket with no policy.
