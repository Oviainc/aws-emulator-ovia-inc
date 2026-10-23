Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda__logs__s3.delete-bucket-is-rejected-while-agent-script-versions-remain
Issue-Key: apigatewayv2__dynamodb__lambda__logs__s3.delete-bucket-is-rejected-while-agent-script-versions-remain|_error#7|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# S3 DeleteBucket deletes a versioning-enabled bucket that still holds object versions and delete markers instead of refusing it with BucketNotEmpty

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteBucket

## Description
Cause 1 of 1: DeleteBucket's emptiness test only considers CURRENT objects, so a versioning-enabled bucket whose keys have been deleted (leaving noncurrent versions plus delete markers) is treated as empty and destroyed.

AWS: a bucket must be empty before it can be deleted, and for a versioned bucket "empty" includes every version and every delete marker. Smithy (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json), DeleteBucket documentation: "Deletes the S3 bucket. All objects (including all object versions and delete markers) in the bucket must be deleted before the bucket itself can be deleted." DeleteBucket is DELETE /{Bucket} with success code 204 and the model declares no error shapes for it, so the refusal itself is recorded behavior only: the recording shows the call refused with error code BucketNotEmpty (S3 documents this code as HTTP 409 Conflict), message (example) "The bucket you tried to delete is not empty. You must delete all versions in the bucket." — the versioned wording, distinct from the unversioned one. Only after every version and delete marker is removed does DeleteBucket succeed, after which list_object_versions on that bucket answers NoSuchBucket.

Emulator: after a versioning-enabled bucket's three objects are deleted with DeleteObject (which correctly leaves ListObjectsV2 KeyCount == 0, i.e. delete markers were laid), DeleteBucket SUCCEEDS instead of being refused. The handler's guard is `if !bucket.objects().is_empty()` — the current-object map only — so noncurrent versions and delete markers are invisible to it and the bucket record is removed with its version history still attached.

Why: a client relying on AWS's behavior loses data and loses an error it is written to handle. Real teardown code (and every CDK autoDeleteObjects-style custom resource) depends on DeleteBucket refusing until the version history is drained: on AWS a forgotten version keeps the bucket alive and surfaces as BucketNotEmpty, which the client catches and then empties versions before retrying. Against the emulator the delete silently succeeds, so (a) the retry/empty-versions code path is never exercised and ships untested, (b) code that asserts a 409 on a still-versioned bucket sees a 204, and (c) the versions and delete markers the caller believed were still protected are discarded. It also makes cleanup phases look correct in emulation while they would fail on AWS.

Fix: the DeleteBucket handler's emptiness predicate in the S3 state layer — it must consider the bucket's full version history (every object version AND every delete marker), not just current objects, and refuse with BucketNotEmpty (409) while any remain, leaving the bucket and its versions intact. Secondary, in the same guard: the refusal message does not vary with versioning, where AWS's versioned-bucket wording adds "You must delete all versions in the bucket." (recorded example; the code and status are the requirement). No open issue covers this — the S3 issues now open concern ListObjectsV2 encoding-type, CreateMultipartUpload/PutBucketCors/PutBucketWebsite stubs, GetObject authorization and PutObject default SSE-KMS.

Traces: failing check apigatewayv2__dynamodb__lambda__logs__s3.delete-bucket-is-rejected-while-agent-script-versions-remain (mismatch; the check declares SERVICES=['s3'] and the diverging call is S3 DeleteBucket — the composite check-id prefix is the cross-service suite it was carved into, so the finding is attributed to s3, which is bootstrapped). Handler: services/service-com-amazonaws-s3-amazons3/src/ops/delete_bucket.rs, handle() lines 223-231 (`if !bucket.objects().is_empty()`).

## Evidence
Expected (AWS, from the recording): with versioning Enabled and every key deleted once,
  s3.delete_bucket(Bucket=b) -> ClientError, Error.Code == 'BucketNotEmpty' (HTTP 409), message example 'The bucket you tried to delete is not empty. You must delete all versions in the bucket.'
  list_object_versions(Bucket=b) then still lists 2 entries per key (one version + one delete marker);
  after deleting every version and marker, delete_bucket succeeds and list_object_versions answers NoSuchBucket.

Actual (emulator): delete_bucket succeeded with no error. Replay output:
  File "check_apigatewayv2__dynamodb__lambda__logs__s3_delete_bucket_is_rejected_while_agent_script_versions_remain.py", line 36, in run
    err = _error(lambda: s3.delete_bucket(Bucket=bucket))
  AssertionError: the call succeeded; real AWS rejects it, so there is no error
(the preceding assertion in the same check — list_objects_v2 KeyCount == 0 after the deletes — passed, so the delete markers were created; only the emptiness test ignores them.)

Code: services/service-com-amazonaws-s3-amazons3/src/ops/delete_bucket.rs lines 223-231 guard on `bucket.objects().is_empty()` (current objects), while the file's own doc comment at lines 6-8 states the intended rule ("All objects ... in the bucket must be deleted before the bucket itself can be deleted": refused with BucketNotEmpty (409)).

## Steps to Reproduce
1. s3 = client('s3'); create a bucket b (with CreateBucketConfiguration.LocationConstraint outside us-east-1).
2. s3.put_bucket_versioning(Bucket=b, VersioningConfiguration={'Status': 'Enabled'}).
3. put_object three keys into b.
4. delete_object each of the three keys (no VersionId) -> this lays a delete marker per key; list_objects_v2(Bucket=b)['KeyCount'] == 0.
5. s3.delete_bucket(Bucket=b)
   -> emulator: succeeds (HTTP 204), bucket gone.
   -> AWS: ClientError with code BucketNotEmpty (HTTP 409); list_object_versions(Bucket=b) still shows 6 entries (3 versions + 3 delete markers).
6. After removing every version and marker via delete_objects with explicit VersionIds, delete_bucket must succeed and list_object_versions(Bucket=b) must then raise NoSuchBucket.

Replay: run_behavioral_suite and re-run check apigatewayv2__dynamodb__lambda__logs__s3.delete-bucket-is-rejected-while-agent-script-versions-remain.
