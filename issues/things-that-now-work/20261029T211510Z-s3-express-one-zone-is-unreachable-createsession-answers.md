Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.create-session-express-zone s3.create-session-roundtrip s3.create-session-token-ttl s3.create-session-nosuchbucket s3.directory-buckets-listing s3.list-directory-buckets-isolation s3.put-object-append-conditional-offset-contention s3.put-object-append-write-offset-rejected s3.put-object-invalid-write-offset s3.put-object-invalid-write-offset-rejected s3.rename-object-concurrent-idempotency s3.rename-object-destination-exists-rejected s3.rename-object-idempotency-mismatch s3.rename-object-idempotency-token s3.rename-object-lifecycle s3.rename-object-state-transition s3.s3x-s3express-session-and-listing-cross-bucket-token-denied s3.s3x-s3express-session-and-listing-non-slash-delimiter-rejected s3.s3x-s3express-session-and-listing-start-after-rejected s3.s3x-s3express-session-and-listing-unterminated-prefix-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.create-session-express-zone|_create_directory_bucket#3|AssertionError|raise AssertionError(f"could not create an S3 Express directory bucket in {ctx.region}: {last_err}")
Issue-Key: s3|CreateSession|NoSuchBucket
Issue-Key: s3|CreateSession|InvalidRequest
Issue-Key: s3.directory-buckets-listing|_create_directory_bucket#3|AssertionError|raise AssertionError(
Issue-Key: s3.list-directory-buckets-isolation|_create_directory_bucket#5|AssertionError|raise AssertionError(
Issue-Key: s3.put-object-append-conditional-offset-contention|_create_directory_bucket_or_skip#2|AssertionError|raise AssertionError(
Issue-Key: s3.put-object-invalid-write-offset|_create_directory_bucket#2|AssertionError|raise AssertionError(
Issue-Key: s3.rename-object-concurrent-idempotency|_create_directory_bucket_or_skip#2|AssertionError|raise AssertionError(
Issue-Key: s3.rename-object-destination-exists-rejected|_create_directory_bucket#2|AssertionError|raise AssertionError(f"directory buckets unavailable in region {ctx.region}: {last_err}")
Issue-Key: s3.rename-object-idempotency-mismatch|_create_directory_bucket#2|AssertionError|raise AssertionError(
Issue-Key: s3.rename-object-state-transition|_create_directory_bucket#1|AssertionError|raise AssertionError(f"S3 Express directory buckets unavailable in {ctx.region}: {last_err}")
Issue-Key: s3.s3x-s3express-session-and-listing-cross-bucket-token-denied|_directory_bucket#2|AssertionError|raise AssertionError(f"could not create a directory bucket in any of {azs!r}: {last}")
Issue-Key: s3.s3x-s3express-session-and-listing-non-slash-delimiter-rejected|_directory_bucket#2|AssertionError|raise AssertionError(f"could not create a directory bucket in any of {azs!r}: {last}")
Issue-Key: s3.s3x-s3express-session-and-listing-start-after-rejected|_directory_bucket#2|AssertionError|raise AssertionError(f"could not create a directory bucket in any of {azs!r}: {last}")
Issue-Key: s3.s3x-s3express-session-and-listing-unterminated-prefix-rejected|_directory_bucket#2|AssertionError|raise AssertionError(f"could not create a directory bucket in any of {azs!r}: {last}")
Issue-Commit: 482573dd66a8fdd59e0387c95d89889e67d3b0e1

# S3 Express One Zone is unreachable: CreateSession answers NoSuchBucket for every directory bucket, and InvalidRequest where AWS answers NoSuchBucket

> _Note: this issue was identified against the code base at commit `482573dd66a8fdd59e0387c95d89889e67d3b0e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSession

## Description
Two net-new root causes in the S3 Express One Zone (directory bucket) surface; a fixer must close both.

Cause 1 of 2: a directory bucket created in any availability zone cannot be sessioned -- CreateSession answers NoSuchBucket.
AWS: CreateBucket with a directory-bucket name (<base>--<az-id>--x-s3) and CreateBucketConfiguration{Location:{Type:AvailabilityZone,Name:<az-id>},Bucket:{Type:Directory,DataRedundancy:SingleAvailabilityZone}} creates the bucket; CreateSession (GET /{Bucket}?session) then returns 200 with Credentials (AccessKeyId, SecretAccessKey, SessionToken, Expiration) scoped to that bucket, and ListDirectoryBuckets lists it. The session token is what every subsequent Express data-plane call (PutObject with x-amz-write-offset-bytes, RenameObject, ListObjectsV2 on the directory bucket) is signed with. (Smithy: CreateSession http GET /{Bucket}?session, output Credentials required, only modeled error NoSuchBucket (404).)
Emulator: for every zone tried (usw2-az1, usw2-az2, usw2-az3, usw2-az4) CreateSession answers NoSuchBucket 'The specified bucket does not exist', so no directory bucket can be used at all. Nineteen checks across CreateSession, ListDirectoryBuckets, append-writes (x-amz-write-offset-bytes) and RenameObject are stopped on this one refusal, and one of them gets a bare 404 rather than even an XML error.
Why: the whole S3 Express One Zone surface is unusable -- a client cannot obtain session credentials, cannot list directory buckets, cannot append to an object and cannot rename one; the bucket the client just created is reported not to exist.
Fix: the CreateSession handler and the directory-bucket state CreateBucket writes (the bucket must be registered under the name/zone CreateSession resolves), plus the Express data-plane operations that depend on the session token.
Traces: s3.create-session-express-zone, s3.create-session-roundtrip, s3.create-session-token-ttl, s3.directory-buckets-listing, s3.list-directory-buckets-isolation, s3.put-object-append-conditional-offset-contention, s3.put-object-append-write-offset-rejected, s3.put-object-invalid-write-offset, s3.put-object-invalid-write-offset-rejected, s3.rename-object-concurrent-idempotency, s3.rename-object-destination-exists-rejected, s3.rename-object-idempotency-mismatch, s3.rename-object-idempotency-token, s3.rename-object-lifecycle, s3.rename-object-state-transition, s3.s3x-s3express-session-and-listing-cross-bucket-token-denied, s3.s3x-s3express-session-and-listing-non-slash-delimiter-rejected, s3.s3x-s3express-session-and-listing-start-after-rejected, s3.s3x-s3express-session-and-listing-unterminated-prefix-rejected

Cause 2 of 2: CreateSession on a bucket that genuinely does not exist is refused with the wrong error.
AWS: NoSuchBucket, HTTP 404, message 'The specified bucket does not exist' -- the only error CreateSession models (Smithy: CreateSession errors = [NoSuchBucket, httpError 404]).
Emulator: InvalidRequest with the generic placeholder message "A parameter or header in your request isn't valid. For details, see the description of this API operation."
Why: a client that retries or falls back on NoSuchBucket never sees it, and the message says the request was malformed when it was well formed.
Fix: the CreateSession handler's bucket lookup, which must raise the modeled NoSuchBucket rather than a generic InvalidRequest.
Traces: s3.create-session-nosuchbucket

## Evidence
Cause 1, representative replay details:
- s3.create-session-express-zone -> AssertionError: could not create an S3 Express directory bucket in us-west-2: An error occurred (NoSuchBucket) when calling the CreateSession operation: The specified bucket does not exist
- s3.list-directory-buckets-isolation -> AssertionError: S3 Express directory buckets unavailable in this environment across all 3 discovered availability zone(s): usw2-az1: ... NoSuchBucket ...; usw2-az2: ... NoSuchBucket ...; usw2-az3: ... NoSuchBucket ...
- s3.put-object-invalid-write-offset -> AssertionError: ... no AZ accepted one; refusals were {'usw2-az1': 'NoSuchBucket', 'usw2-az2': 'NoSuchBucket', 'usw2-az3': 'NoSuchBucket'}
- s3.s3x-s3express-session-and-listing-* -> AssertionError: could not create a directory bucket in any of ['usw2-az1', 'usw2-az3', 'usw2-az4']: An error occurred (NoSuchBucket) when calling the CreateSession operation: The specified bucket does not exist
- s3.rename-object-idempotency-token-mismatch family / s3.rename-object-idempotency-token -> directory bucket creation not supported here: ... NoSuchBucket ...
Expected: CreateSession 200 with Credentials, and the directory bucket visible in ListDirectoryBuckets.

Cause 2:
- s3.create-session-nosuchbucket -> botocore.errorfactory.InvalidRequest: An error occurred (InvalidRequest) when calling the CreateSession operation: A parameter or header in your request isn't valid. For details, see the description of this API operation.
Expected: NoSuchBucket / 404 / 'The specified bucket does not exist'.

## Steps to Reproduce
Replay the behavioral suite and run the s3 checks in trace_ids. For cause 1: CreateBucket with a directory-bucket name and CreateBucketConfiguration Location{Type:AvailabilityZone,Name:<az-id>} + Bucket{Type:Directory,DataRedundancy:SingleAvailabilityZone}, then call CreateSession(Bucket=<that name>) -- observe NoSuchBucket for every AZ, and that ListDirectoryBuckets, PutObject with x-amz-write-offset-bytes and RenameObject are consequently unreachable. For cause 2: call CreateSession(Bucket='a-name-that-was-never-created') and compare the error code against NoSuchBucket/404.
