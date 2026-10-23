Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lightsail.bucket-object-put-get-roundtrip lightsail.bucket-access-key-lifecycle lightsail.bucket-delete-nonempty-without-force-rejected
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail.bucket-object-put-get-roundtrip|_put_object_with_retry#13|Timeout|time.sleep(interval_s)
Issue-Key: lightsail.bucket-access-key-lifecycle|_list_objects_with_retry#14|Timeout|time.sleep(interval_s)
Issue-Key: lightsail.bucket-delete-nonempty-without-force-rejected|_put_object_with_retry#9|Timeout|time.sleep(interval_s)
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Lightsail bucket object data plane is not served: a bucket's access key reaches no S3-compatible endpoint, so PutObject/GetObject/ListObjectsV2 never succeed and three bucket checks burn their whole budget

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBucketAccessKey

## Description
Cause 1 of 1: a Lightsail bucket created by CreateBucket exists only as a control-plane record; nothing in the emulator serves the bucket's OBJECT data plane, so the access key CreateBucketAccessKey mints cannot be used to read or write objects. Failing checks: lightsail.bucket-object-put-get-roundtrip, lightsail.bucket-access-key-lifecycle, lightsail.bucket-delete-nonempty-without-force-rejected.

AWS: a Lightsail bucket IS an S3-compatible object store. Smithy (com.amazonaws.lightsail, 2016-11-28): `Bucket$url` — "The URL of the bucket" (NonEmptyString, pattern \S), i.e. an address the caller can actually reach; `AccessKey` — "Access keys grant full programmatic access to the specified bucket and its objects", `AccessKey$secretAccessKey` — "The secret access key used to sign requests", `AccessKey$status` — "A status of Active means that the key is valid". So after CreateBucket + CreateBucketAccessKey, signing an S3 request (ListObjectsV2 / PutObject / GetObject / DeleteObject) against that bucket with the returned accessKeyId/secretAccessKey succeeds, and `AccessKey$lastUsed` is then populated. Recorded behavior in the checks: ListObjectsV2 answers with Name == the bucket name; PutObject then GetObject round-trips the body; after DeleteObject, GetObject answers error code NoSuchKey (message example: "The specified key does not exist."); and DeleteBucket on a bucket that still holds an object is refused with code InvalidInputException (message example: "Bucket is not empty: <bucketName>").

Emulator: CreateBucket (services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_bucket.rs) and CreateBucketAccessKey (.../src/ops/create_bucket_access_key.rs) both succeed and the key is minted into the credential store, but the bucket is registered with no object endpoint. The service's own state module says so explicitly: "Nothing claims or answers this path space yet: a bucket's object data plane (put, get, list) is not implemented here" and "a bucket's object data plane is not served here, so no request against this bucket can have been signed with the key". Consequently every first S3 data-plane call in these checks fails persistently; each check wraps that call in a bounded retry loop that only loops while the call keeps raising a client error, so all three exhaust the 60s check budget and the suite reports "Check timed out after 60.0s" for each. The assertions beyond that first data-plane call are never reached at all, including the DeleteBucket-not-empty rejection (InvalidInputException) and the key-revocation listing in GetBucketAccessKeys.

Why: a client that uses Lightsail object storage the way AWS documents it — create bucket, mint an access key, then store and fetch objects with that key — cannot store or fetch anything. The control plane reports a healthy bucket (state OK), an Active access key and a `url` that nothing answers, so the failure surfaces only at the data plane and looks like a credential or endpoint problem rather than a missing feature. Anything built on top (object-presence gating of DeleteBucket, access-log/CORS behavior, `AccessKey$lastUsed`) is unreachable too.

Fix: the bucket resource's data plane in the lightsail service — the bucket object store in the service state plus the handler/route that claims the bucket's published path space (the module's `can_handle` seam, the way the container-service public path prefix is already claimed), and the binding that makes a bucket access key from the credential store a valid signer for S3-style object requests against that bucket. The control-plane handlers themselves (create_bucket.rs, create_bucket_access_key.rs) look correct; what is missing is anything serving objects, and `Bucket$url` must name the address the emulator actually answers on.

Related but distinct open issue: 20261022T170908Z-lightsail-container-service-public-endpoint-answers-501-so.md covers the container-service public endpoint, a different resource's data plane; it does not cover buckets.

Traces: lightsail.bucket-object-put-get-roundtrip (times out in the PutObject retry loop), lightsail.bucket-access-key-lifecycle (times out in the ListObjectsV2 retry loop), lightsail.bucket-delete-nonempty-without-force-rejected (times out in the PutObject retry loop, before the DeleteBucket rejection is ever asserted). Code citations: services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs lines 668-686 and 765-771; .../src/ops/create_bucket.rs lines 204-276; .../src/ops/create_bucket_access_key.rs lines 183-275.

## Evidence
Suite: lightsail 1/25 passed. Three surviving failures, all verdict 'mismatch' with detail "Check timed out after 60.0s":

- lightsail.bucket-object-put-get-roundtrip: expected PutObject(bucket, 'roundtrip-test.txt', body) to succeed, GetObject to return the same body, and GetObject after DeleteObject to fail with code NoSuchKey. Actual: the check never gets past `_put_object_with_retry(..., timeout_s=60.0)`, which retries only while PutObject raises a ClientError; the 60s budget expires there.
- lightsail.bucket-access-key-lifecycle: expected s3.list_objects_v2(Bucket=bucket) signed with the freshly minted accessKeyId/secretAccessKey to return Name == bucket. Actual: the check never gets past `_list_objects_with_retry(..., timeout_s=60.0)`; the 60s budget expires there. The control plane itself answered: create_bucket_access_key returned status 'Active' and get_bucket_access_keys listed the key.
- lightsail.bucket-delete-nonempty-without-force-rejected: expected PutObject to place a blocking object, then DeleteBucket without forceDelete to be refused with code InvalidInputException (message example "Bucket is not empty: <bucketName>"), with the bucket and object surviving. Actual: times out in `_put_object_with_retry(..., timeout_s=60.0)`; the rejection contract is never exercised.

Emulator source confirms the cause rather than a flaky environment — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs: "Nothing claims or answers this path space yet: a bucket's object data plane (put, get, list) is not implemented here, and it is `can_handle`'s business to claim the path once it is, as `CONTAINER_SERVICE_PUBLIC_PATH_PREFIX` already does for that resource"; and on the access-key view: "this emulator records no use of a credential — a bucket's object data plane is not served here, so no request against this bucket can have been signed with the key".

## Steps to Reproduce
1. ls = client('lightsail'); bundle = ls.get_bucket_bundles()['bundles'][0]['bundleId']
2. ls.create_bucket(bucketName='<lowercase-unique-name>', bundleId=bundle)  -> succeeds
3. key = ls.create_bucket_access_key(bucketName=<name>)['accessKey']  -> status 'Active', carries accessKeyId + secretAccessKey
4. s3 = client('s3', creds={aws_access_key_id: key['accessKeyId'], aws_secret_access_key: key['secretAccessKey']})
5. s3.put_object(Bucket=<name>, Key='roundtrip-test.txt', Body=b'payload')  -> on AWS this succeeds; against the emulator it keeps failing, so a bounded retry loop never completes and the check hits its 60s budget
6. Same for s3.list_objects_v2(Bucket=<name>) (expected Name == <name>) and, after a successful put, s3.get_object / s3.delete_object / DeleteBucket-without-forceDelete (expected InvalidInputException 'Bucket is not empty: <name>')
