Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-lcp-short-transition-days-accepted-and-round-trip s3.deep-s3-storage-class-lifecycle-transition-invalid-target-rejected s3.deep-s3-storage-class-lifecycle-transition-ia-to-glacier-gap-rejected s3.s3-lifecycle-tag-e2e-tag-filter-expiration-lifecycle s3.restore-object-ttl-expiry-reverts s3.deep-s3-storage-class-restore-tier-downgrade-rejected s3.deep-s3-storage-class-restore-zero-days-rejected s3.head-object-on-delete-marker-rejected s3.object-storage-class-reflected s3.op-contract-getObjectTagging s3.s3-old-buckets-unfiltered-listing-omits-bucket-region s3.s3-old-buckets-regional-view-advances-creation-date-on-a-config-write s3.s3-conditional-precedence-and-lifecycle-headers-copy s3.s3-conditional-precedence-and-lifecycle-headers-get s3.s3checksum-copy-object-computes-the-requested-algorithm-from-the-bytes s3.s3checksum-upload-part-mismatch-rejected-and-the-part-is-absent
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketLifecycleConfiguration|InvalidArgument
Issue-Key: s3.deep-s3-storage-class-lifecycle-transition-invalid-target-rejected|run#10|AssertionError|assert err.get("Code") == "InvalidArgument", f"Transition to STANDARD returned {err!r}, expected InvalidArgument"
Issue-Key: s3.deep-s3-storage-class-lifecycle-transition-ia-to-glacier-gap-rejected|run#11|AssertionError|assert err.get("Message") == "'Days' in the 'Transition' action for StorageClass 'GLACIER' for filter '(prefix=tiered/)' must be 30 days more than 'filter '(pre
Issue-Key: s3.s3-lifecycle-tag-e2e-tag-filter-expiration-lifecycle|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: s3.restore-object-ttl-expiry-reverts|run#8|AssertionError|assert first["ResponseMetadata"]["HTTPStatusCode"] == 202, (
Issue-Key: s3.deep-s3-storage-class-restore-tier-downgrade-rejected|run#10|AssertionError|assert first == 202, f"initial Bulk restore returned HTTP {first}, expected 202"
Issue-Key: s3.deep-s3-storage-class-restore-zero-days-rejected|run#9|AssertionError|assert refusal == ("InvalidArgument", _ZERO_DAYS_MESSAGE), (
Issue-Key: s3.head-object-on-delete-marker-rejected|run#17|AssertionError|assert headers.get("x-amz-delete-marker") == "true", (
Issue-Key: s3.object-storage-class-reflected|run#8|AssertionError|assert "StorageClass" not in std_head, (
Issue-Key: s3.op-contract-getObjectTagging|run#14|AssertionError|assert "VersionId" in resp1, f"expected a VersionId on a versioned bucket, got keys {list(resp1)}"
Issue-Key: s3.s3-old-buckets-unfiltered-listing-omits-bucket-region|run#3|AssertionError|assert sorted(bare) == ["BucketArn", "CreationDate", "Name"], (
Issue-Key: s3.s3-old-buckets-regional-view-advances-creation-date-on-a-config-write|_poll#11|Timeout|time.sleep(interval_s)
Issue-Key: s3.s3-conditional-precedence-and-lifecycle-headers-copy|rejected#15|AssertionError|assert outcome == (
Issue-Key: s3.s3-conditional-precedence-and-lifecycle-headers-get|run#14|AssertionError|assert ius_alone[:2] == (412, "PreconditionFailed"), (
Issue-Key: s3.s3checksum-copy-object-computes-the-requested-algorithm-from-the-bytes|run#12|AssertionError|assert result.get("ChecksumSHA256") == want_sha256, (
Issue-Key: s3.s3checksum-upload-part-mismatch-rejected-and-the-part-is-absent|run#12|KeyError|assert p1["ChecksumSHA256"] == want_p1, (
Issue-Commit: 482573dd66a8fdd59e0387c95d89889e67d3b0e1

# S3 representation divergences: lifecycle transition validator, RestoreObject 202, delete-marker header, HTTP-date conditionals, ListBuckets BucketArn, checksums

> _Note: this issue was identified against the code base at commit `482573dd66a8fdd59e0387c95d89889e67d3b0e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketLifecycleConfiguration

## Description
Ten net-new root causes in what S3 reports back (statuses, headers and members), as opposed to whether a call is allowed. A fixer must close all ten.

Cause 1 of 10: the lifecycle transition validator both over-rejects and misclassifies.
AWS: PutBucketLifecycleConfiguration accepts a transition to STANDARD_IA after as few as 1 day (there is no minimum-days floor on the configuration; the minimum-storage-duration charge is a billing rule, not a validation rule), refuses a transition whose target is STANDARD with InvalidArgument, and words its minimum-duration/ordering refusals in AWS's own text.
Emulator: rejects 'transitions to "STANDARD_IA" after 1 days' with InvalidArgument 'Rule 1: transitions to "STANDARD_IA" after 1 days, before the 30 days an object is stored in its current class first'; refuses a STANDARD target with MalformedXML ('The XML you provided was not well-formed or did not validate against our published schema') instead of InvalidArgument; and emits its own sentences for the IA-to-GLACIER gap ('Rule 1: transitions to "GLACIER" 10 days after the transition to "STANDARD_IA", which has a minimum storage duration of 30 days'). A tag-filtered expiration lifecycle never converges and the check is killed at 60 s.
Why: valid configurations are refused outright, so a client cannot install the lifecycle it wants; an invalid target is reported as malformed XML, so the caller looks for a serialization bug instead of fixing the class; and the messages are not AWS's.
Fix: the PutBucketLifecycleConfiguration validator (its rules and messages) and the lifecycle engine that must apply a tag-filtered expiration.
Traces: s3.s3-lcp-short-transition-days-accepted-and-round-trip, s3.deep-s3-storage-class-lifecycle-transition-invalid-target-rejected, s3.deep-s3-storage-class-lifecycle-transition-ia-to-glacier-gap-rejected, s3.s3-lifecycle-tag-e2e-tag-filter-expiration-lifecycle

Cause 2 of 10: RestoreObject answers 200 where AWS answers 202.
AWS: the first RestoreObject for an archived object is accepted with HTTP 202 Accepted (the restore has been initiated); 200 is reserved for a restore that is already in progress or already complete. (Smithy: RestoreObject http code 200 is the modeled default; the 202-on-initiation status is recorded behavior.)
Emulator: returns HTTP 200 on the initial restore.
Why: a client that distinguishes 'I started this restore' (202) from 'someone already did' (200) -- the documented way to make restore idempotent -- cannot, so it double-counts restores or skips waiting for one it actually initiated.
Fix: the RestoreObject handler's status selection.
Traces: s3.restore-object-ttl-expiry-reverts, s3.deep-s3-storage-class-restore-tier-downgrade-rejected

Cause 3 of 10: RestoreObject Days=0 is refused with the emulator's own message.
AWS: refuses InvalidArgument with AWS's wording for a non-positive Days; the limit and the offending value are the variable parts.
Emulator: 'Days must be a positive number of days, got 0'.
Why: the refusal message is part of the error class; this one is not reproducible against AWS.
Fix: the RestoreObject Days validation's message.
Traces: s3.deep-s3-storage-class-restore-zero-days-rejected

Cause 4 of 10: a 404 for a key hidden by a delete marker omits x-amz-delete-marker.
AWS: HeadObject/GetObject on a key whose latest version is a delete marker answers 404 carrying x-amz-delete-marker: true (and x-amz-version-id of the marker). That header is the only thing separating 'the latest version is a delete marker' from 'this key never existed', since both are 404.
Emulator: the 404 carries no x-amz-delete-marker header at all (response headers: content-type, x-amz-request-id, x-amz-id-2, content-length, date).
Why: a client cannot tell a deleted object from one that never existed, so it cannot decide whether to undelete (remove the marker) or to create.
Fix: the HeadObject/GetObject not-found serializer, which must emit x-amz-delete-marker when the head of the version chain is a delete marker.
Traces: s3.head-object-on-delete-marker-rejected

Cause 5 of 10: HeadObject reports StorageClass for a STANDARD object.
AWS: x-amz-storage-class is omitted entirely when the object is in STANDARD; it is present only for non-STANDARD classes.
Emulator: reports StorageClass 'STANDARD'.
Why: a client that treats presence of the member as 'not STANDARD' misclassifies every object, and the response shape differs from AWS's for the commonest case.
Fix: the HeadObject/GetObject metadata serializer, which must suppress the storage-class header for STANDARD.
Traces: s3.object-storage-class-reflected

Cause 6 of 10: GetObjectTagging omits the version id.
AWS: GetObjectTagging returns VersionId -- the version whose tag set was read -- as x-amz-version-id. (Smithy: GetObjectTaggingOutput.VersionId -> httpHeader x-amz-version-id, alongside the required TagSet.)
Emulator: the response carries only ResponseMetadata and TagSet; VersionId is absent even on a versioned bucket.
Why: on a versioned bucket a caller cannot tell which version's tags it just read, so it cannot write them back to the same version.
Fix: the GetObjectTagging serializer.
Traces: s3.op-contract-getObjectTagging

Cause 7 of 10: ListBuckets entries omit BucketArn.
AWS: each Buckets member carries Name, CreationDate and BucketArn; a parameterless ListBuckets returns exactly those three.
Emulator: returns only CreationDate and Name.
Why: a caller that builds a resource ARN from the listing -- for a policy, a tag request or a cross-service reference -- has nothing to build from.
Fix: the ListBuckets serializer and the bucket state it reads, which must carry the bucket's ARN.
Traces: s3.s3-old-buckets-unfiltered-listing-omits-bucket-region

Cause 8 of 10: a bucket's CreationDate changes when a configuration is written.
AWS: CreationDate is set once, when the bucket is created, and never moves; writing a bucket configuration (versioning, encryption, lifecycle, ...) does not touch it, and the regional view reports the same instant as the global one.
Emulator: the check that reads CreationDate across a configuration write never completes and is killed at the 60 s budget, so the invariant is never satisfied.
Why: a client that orders or ages buckets by CreationDate, or that uses it as a stable identity, sees it move under configuration writes.
Fix: the bucket state behind the configuration-write handlers, which must leave CreationDate untouched, and the regional ListBuckets view that reports it.
Traces: s3.s3-old-buckets-regional-view-advances-creation-date-on-a-config-write

Cause 9 of 10: HTTP-date conditional headers are rejected as invalid timestamps.
AWS: If-Modified-Since, If-Unmodified-Since, x-amz-copy-source-if-modified-since and x-amz-copy-source-if-unmodified-since are HTTP-dates (RFC 7231, e.g. 'Thu, 29 Oct 2026 20:56:50 GMT'); S3 parses them and answers 412 PreconditionFailed (or 304 Not Modified) according to the object's LastModified. (Smithy binds these members with timestampFormat http-date.)
Emulator: answers HTTP 400 ValidationException 'invalid timestamp `Thu, 29 Oct 2026 20:56:50 GMT`: invalid date-time: Smithy does not support timezone offsets in RFC-3339 date times' -- it parses the header as RFC-3339 instead of http-date.
Why: every date-based conditional GET and conditional copy -- caching, incremental sync, optimistic copy -- is rejected as a malformed request instead of being evaluated, and the error even names RFC-3339, a format the header never uses.
Fix: the conditional-header deserialization for the object read/copy handlers, which must decode http-date-formatted timestamp members.
Traces: s3.s3-conditional-precedence-and-lifecycle-headers-copy, s3.s3-conditional-precedence-and-lifecycle-headers-get

Cause 10 of 10: requested checksums are not computed or returned on CopyObject and UploadPart.
AWS: when x-amz-sdk-checksum-algorithm (or an explicit algorithm) is requested, CopyObject computes the checksum over the copied bytes and returns it (x-amz-checksum-sha256 etc.), and UploadPart returns the part's checksum in the matching header -- which is also what makes a mismatched part checksum refusable with the part absent. (Smithy: UploadPartOutput and CopyObject both model ChecksumSHA256/ChecksumCRC32/... bound to x-amz-checksum-* headers; UploadPart carries httpChecksum{requestAlgorithmMember: ChecksumAlgorithm}.)
Emulator: CopyObject reports ChecksumSHA256 = None even though the client can compute it over the source bytes, and UploadPart's response has no ChecksumSHA256 member at all (KeyError).
Why: a client cannot verify integrity of a copy or of a part, and cannot drive the mismatch-rejection path at all; related to the open issue 20261029T072224Z-s3-uploadpart-does-not-return-x-amz-checksum.md, which does not carry these replay keys.
Fix: the CopyObject and UploadPart handlers' checksum computation and the x-amz-checksum-* serialization.
Traces: s3.s3checksum-copy-object-computes-the-requested-algorithm-from-the-bytes, s3.s3checksum-upload-part-mismatch-rejected-and-the-part-is-absent

## Evidence
s3.s3-lcp-short-transition-days-accepted-and-round-trip -> botocore ClientError (InvalidArgument) on PutBucketLifecycleConfiguration: 'Rule 1: transitions to "STANDARD_IA" after 1 days, before the 30 days an object is stored in its current class first'
s3.deep-s3-storage-class-lifecycle-transition-invalid-target-rejected -> AssertionError: Transition to STANDARD returned {'Code': 'MalformedXML', ...}, expected InvalidArgument
s3.deep-s3-storage-class-lifecycle-transition-ia-to-glacier-gap-rejected -> AssertionError: {'Code': 'InvalidArgument', 'Message': 'Rule 1: transitions to "GLACIER" 10 days after the transition to "STANDARD_IA", which has a minimum storage duration of 30 days'}
s3.s3-lifecycle-tag-e2e-tag-filter-expiration-lifecycle -> Error: Check timed out after 60.0s
s3.restore-object-ttl-expiry-reverts -> AssertionError: first RestoreObject call should be accepted with 202, got 200
s3.deep-s3-storage-class-restore-tier-downgrade-rejected -> AssertionError: initial Bulk restore returned HTTP 200, expected 202
s3.deep-s3-storage-class-restore-zero-days-rejected -> AssertionError: RestoreObject Days=0 returned ('InvalidArgument', 'Days must be a positive number of days, got 0'), expected InvalidArgument
s3.head-object-on-delete-marker-rejected -> AssertionError: the 404 must carry x-amz-delete-marker: true ... got None
s3.object-storage-class-reflected -> AssertionError: HeadObject must omit StorageClass for STANDARD objects, got 'STANDARD'
s3.op-contract-getObjectTagging -> AssertionError: expected a VersionId on a versioned bucket, got keys ['ResponseMetadata', 'TagSet']
s3.s3-old-buckets-unfiltered-listing-omits-bucket-region -> AssertionError: via us-east-1, a parameterless ListBuckets entry must carry exactly Name, CreationDate and BucketArn; got keys ['CreationDate', 'Name']
s3.s3-old-buckets-regional-view-advances-creation-date-on-a-config-write -> Error: Check timed out after 60.0s
s3.s3-conditional-precedence-and-lifecycle-headers-copy -> AssertionError: CopySourceIfUnmodifiedSince=LM-1s alone: expected 412 PreconditionFailed, got (400, 'ValidationException', 'invalid timestamp `Thu, 29 Oct 2026 20:56:50 GMT`: invalid date-time: Smithy does not support timezone offsets in RFC-3339 date times')
s3.s3-conditional-precedence-and-lifecycle-headers-get -> AssertionError: If-Unmodified-Since=LastModified-1s alone must fail 412 PreconditionFailed ...; got (400, 'ValidationException')
s3.s3checksum-copy-object-computes-the-requested-algorithm-from-the-bytes -> AssertionError: CopyObject reported ChecksumSHA256=None; the client computed 'Q+j18bNlm2TNy7riNEE8ub98jtRKQO9WLpw/g4owrfQ=' with hashlib over the source's 3600 bytes
s3.s3checksum-upload-part-mismatch-rejected-and-the-part-is-absent -> KeyError: 'ChecksumSHA256'

## Steps to Reproduce
Replay the behavioral suite and run the s3 checks in trace_ids. Representative calls: PutBucketLifecycleConfiguration with a STANDARD_IA transition at 1 day, with a STANDARD target, and with a STANDARD_IA -> GLACIER gap of 10 days; a tag-filtered expiration rule; RestoreObject on an archived object (first call) and with Days=0; HeadObject on a key hidden by a delete marker; HeadObject on a STANDARD object; GetObjectTagging on a versioned bucket; a parameterless ListBuckets; a bucket-configuration write followed by re-reading CreationDate; GetObject with If-Unmodified-Since and CopyObject with x-amz-copy-source-if-unmodified-since as HTTP-dates; CopyObject with ChecksumAlgorithm=SHA256 and UploadPart with an explicit x-amz-checksum-sha256.
