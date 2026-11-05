Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.create-session-no-such-bucket s3.directory-buckets-listing s3.list-directory-buckets-isolation s3.list-directory-buckets-rejects-regular-bucket-ops
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.create-session-no-such-bucket|run#12|AssertionError|assert err == ("NoSuchBucket", "The specified bucket does not exist."), (
Issue-Key: s3.directory-buckets-listing|run#8|AssertionError|assert regular not in dir_names, (
Issue-Key: s3.list-directory-buckets-isolation|run#8|AssertionError|assert dir_bucket not in gp_listed, f"directory bucket {dir_bucket} leaked into ListBuckets"
Issue-Key: s3.list-directory-buckets-rejects-regular-bucket-ops|run#10|AssertionError|assert bucket not in directory_bucket_names, (
Issue-Commit: 475d7014ed423f1edb047e4bfa101e91e3c4464d

# s3: the directory-bucket (S3 Express One Zone) namespace is not partitioned from the general-purpose one — ListDirectoryBuckets and ListBuckets each return the other's buckets — and CreateSession's NoSuchBucket message drops its final period (2 causes)

> _Note: this issue was identified against the code base at commit `475d7014ed423f1edb047e4bfa101e91e3c4464d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDirectoryBuckets

## Description
Two net-new root causes in the S3 Express One Zone / directory-bucket surface of the s3 emulator. A fixer must close BOTH before submitting.

Note on neighbouring work: the open issue 20261029T211510Z-s3-express-one-zone-is-unreachable-createsession-answers.md describes an earlier, different state of this surface (CreateSession answering NoSuchBucket for EVERY directory bucket, and InvalidRequest instead of NoSuchBucket for an absent one). That is no longer what the emulator does: in this replay a directory bucket is created and CreateSession on it returns 200 with Credentials (the checks' positive controls pass), and CreateSession on an absent bucket already returns the modeled NoSuchBucket code. Neither cause below is covered by that issue; both are new behaviour reached only now that directory buckets can be created at all.

=== Cause 1 of 2: ListDirectoryBuckets and ListBuckets are served from one undivided bucket namespace, so each lists buckets of the other kind ===
AWS: a directory bucket and a general-purpose bucket live in two disjoint listings. ListBuckets (Smithy: ListBuckets, GET /?x-id=ListBuckets, output xmlName ListAllMyBucketsResult, output member Buckets) enumerates ONLY general-purpose buckets. ListDirectoryBuckets (Smithy: ListDirectoryBuckets, GET /?x-id=ListDirectoryBuckets, output xmlName ListAllMyDirectoryBucketsResult, output members Buckets + ContinuationToken, paginated by MaxDirectoryBuckets range 0..1000) enumerates ONLY S3 Express One Zone directory buckets — those created with CreateBucketConfiguration{Location:{Type:"AvailabilityZone",Name:<az-id>}, Bucket:{Type:"Directory",DataRedundancy:"SingleAvailabilityZone"}} and named <base>--<az-id>--x-s3. The two result sets are disjoint for the same account and region: a directory bucket never appears in ListBuckets, and a general-purpose bucket never appears in ListDirectoryBuckets. The separate operation, separate result element name and separate pagination members exist precisely because the bucket type is part of a bucket's identity. (Both operations model no errors; this is a response-content requirement, so it is 'recorded behavior' on top of the model's two distinct output shapes.)
Emulator: both operations answer 200 and return the SAME set of buckets — every bucket the account has, regardless of type. Replayed: with one general-purpose bucket and one directory bucket created in the same region, ListDirectoryBuckets returns both names (the general-purpose bucket is present in the ListAllMyDirectoryBucketsResult Buckets list), and ListBuckets likewise returns the directory bucket. Paginating ListDirectoryBuckets with MaxDirectoryBuckets=1000 and following ContinuationToken does not change this — the leak is in which buckets are selected, not in paging.
Why: a client cannot tell the two bucket classes apart from the API. Express-only tooling that enumerates with ListDirectoryBuckets will pick up general-purpose buckets and then drive the Express data plane against them (CreateSession, x-amz-write-offset-bytes appends, RenameObject), which AWS refuses; general-purpose tooling that enumerates with ListBuckets will pick up directory buckets and issue general-purpose-only bucket operations (versioning, replication, website, lifecycle) against them. Inventory, cost-attribution and cleanup logic that treats the two listings as disjoint sets double-counts every bucket, and a 'delete everything ListBuckets reports' teardown reaches buckets it was never meant to touch.
Fix: the ListDirectoryBuckets and ListBuckets handlers and the bucket state they read — the bucket record must carry its type (directory vs general-purpose, as decided by CreateBucket's CreateBucketConfiguration.Bucket.Type) and each listing must filter on it, rather than both projecting the whole bucket map.
Traces: s3.directory-buckets-listing, s3.list-directory-buckets-isolation, s3.list-directory-buckets-rejects-regular-bucket-ops

=== Cause 2 of 2: CreateSession's NoSuchBucket refusal message is missing its terminating period ===
AWS: CreateSession (Smithy: GET /{Bucket}?session, output Credentials required, single modeled error NoSuchBucket with httpError 404) refuses a directory bucket that does not exist with HTTP 404, error code NoSuchBucket, and the message 'The specified bucket does not exist.' The whole message is fixed text for this error class — it interpolates nothing from the request, so there is no variable part to fill; the terminating period is part of that fixed text.
Emulator: the status (404) and the code (NoSuchBucket) are right, but the message is 'The specified bucket does not exist' — the same sentence with the final '.' dropped. Replayed assertion: expected ('NoSuchBucket', 'The specified bucket does not exist.'), got ('NoSuchBucket', 'The specified bucket does not exist'). The bucket name used is a well-formed directory-bucket name in the region under test (<base>-missing--<az-id>--x-s3) that ListDirectoryBuckets confirms is absent both before and after the call, and no bucket is created as a side effect — only the message text diverges.
Why: the error message is part of the contract. A client, test harness or log-matching rule that compares the message against AWS's sentence never matches, so a correct NoSuchBucket refusal is treated as an unknown error.
Fix: the CreateSession handler's NoSuchBucket construction (the message literal it passes for the modeled NoSuchBucket error).
Traces: s3.create-session-no-such-bucket

## Evidence
Cause 1 of 2 — one undivided bucket namespace across the two listings:
- s3.directory-buckets-listing -> AssertionError: regular bucket sim2real-s3-behav-<id> unexpectedly in ListDirectoryBuckets: ['sim2real-s3-behav-<id>', 'sim2real-s3exp-<id>--<az-id>--x-s3'] (the generated names and az-id differ per run; what matters is that BOTH the general-purpose and the directory bucket are listed).
- s3.list-directory-buckets-isolation -> AssertionError: directory bucket sim2realdb<id>--<az-id>--x-s3 leaked into ListBuckets. The same run's positive controls pass: the general-purpose bucket IS in ListBuckets and the directory bucket IS in ListDirectoryBuckets, so this is not an empty/unimplemented listing — it is an unfiltered one.
- s3.list-directory-buckets-rejects-regular-bucket-ops -> AssertionError: general-purpose bucket sim2real-s3-<id> incorrectly appeared in ListDirectoryBuckets result (directory-bucket-only namespace leaked general-purpose bucket). Reached after fully paginating ListDirectoryBuckets with MaxDirectoryBuckets=1000 and ContinuationToken.
Expected in all three: ListBuckets returns the general-purpose bucket and NOT the directory bucket; ListDirectoryBuckets returns the directory bucket and NOT the general-purpose bucket. Both calls answer 200 in the emulator, so the divergence is purely in the Buckets list each returns.

Cause 2 of 2 — CreateSession NoSuchBucket message:
- s3.create-session-no-such-bucket -> AssertionError: expected NoSuchBucket with AWS's exact message, got ('NoSuchBucket', 'The specified bucket does not exist')
Expected: HTTP 404, Code NoSuchBucket, Message 'The specified bucket does not exist.' (fixed text, no interpolated values). Actual: HTTP 404, Code NoSuchBucket, Message 'The specified bucket does not exist' (final period absent). In the same check, CreateSession against the real directory bucket returns 200 with Credentials.AccessKeyId populated, and ListDirectoryBuckets shows the missing name absent both before and after the refused call.

## Steps to Reproduce
Replay the behavioral suite and run the four s3 checks named in trace_ids.

Cause 1: in a region with S3 Express availability zones, (a) CreateBucket a general-purpose bucket (with CreateBucketConfiguration.LocationConstraint outside us-east-1); (b) CreateBucket a directory bucket named <base>--<az-id>--x-s3 with CreateBucketConfiguration={Location:{Type:'AvailabilityZone',Name:<az-id>}, Bucket:{Type:'Directory',DataRedundancy:'SingleAvailabilityZone'}}. Then call ListBuckets and ListDirectoryBuckets (the latter with MaxDirectoryBuckets=1000, following ContinuationToken to exhaustion) and compare the two Buckets lists: each currently contains both names; each must contain only its own bucket class.

Cause 2: create a directory bucket as above so the positive control succeeds, then call CreateSession(Bucket='<base>-missing--<az-id>--x-s3') for a name ListDirectoryBuckets does not report, and compare the response's Error.Code and Error.Message against NoSuchBucket / 'The specified bucket does not exist.' — the emulator's message omits the trailing period.
