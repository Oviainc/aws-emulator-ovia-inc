Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.deep-s3-storage-class-restore-tier-downgrade-rejected s3.s3-oc-get-on-a-missing-bucket-is-not-shortcircuited-by-a-foreign-expected-bucket-owner
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|RestoreObject|RestoreAlreadyInProgress
Issue-Key: s3.s3-oc-get-on-a-missing-bucket-is-not-shortcircuited-by-a-foreign-expected-bucket-owner|run#5|AssertionError|assert code == "NoSuchBucket", (
Issue-Commit: 96b1cd775c53225a6d4bf8c1fc7ed76785b74794

# s3: 2 net-new root causes — RestoreObject refuses a restore-tier upgrade, and the ExpectedBucketOwner guard short-circuits the bucket-existence check on GetBucketOwnershipControls

> _Note: this issue was identified against the code base at commit `96b1cd775c53225a6d4bf8c1fc7ed76785b74794`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RestoreObject

## Description
REOPENS 20261025T032458Z-s3-restoreobject-stub.md: its fix left the call refused. Two net-new root causes in the s3 emulator, both grounded in behavioral checks that are not covered by any open issue's replay key. A fixer must close BOTH before submitting.

Cause 1 of 2: RestoreObject refuses a restore-tier upgrade of an in-flight restore.
AWS: while a restore of an archived (GLACIER) object is in progress, a second RestoreObject naming a STRICTLY FASTER retrieval tier is a documented restore-tier upgrade and is ACCEPTED with HTTP 202 Accepted; the restore stays ongoing (HEAD/GET keep reporting Restore: ongoing-request="true") and the object keeps its archive storage class. Only a second request for the SAME or a SLOWER tier is refused, with error code RestoreAlreadyInProgress, HTTP 409 Conflict, message "Object restore is already in progress". Tier order, fastest to slowest: Expedited, Standard, Bulk. Smithy: RestoreObject input RestoreRequest -> RestoreRequest (httpPayload), GlacierJobParameters.Tier; the model binds only ObjectAlreadyInActiveTierError (403), so RestoreAlreadyInProgress/409 and the 202-on-accept status are REST-documented and recorded behavior, not modeled. The message "Object restore is already in progress" is fixed text with no variable parts — the defect is not its wording but that it is applied to the upgrade case.
Emulator: the first RestoreObject with Tier=Bulk is accepted with 202 and HEAD reports ongoing-request="true" (correct), and a repeated Bulk request is correctly refused with RestoreAlreadyInProgress and that exact message (correct). But the subsequent RestoreObject with Tier=Standard — a faster tier, i.e. the upgrade — is ALSO refused: botocore.exceptions.ClientError: An error occurred (RestoreAlreadyInProgress) when calling the RestoreObject operation: Object restore is already in progress. The emulator treats 'a restore is in flight' as an unconditional refusal and never compares the requested tier with the in-flight tier. Because the upgrade is refused, the check never reaches its later assertions (that a Bulk request AFTER the upgrade is refused, and that HEAD still shows ongoing-request="true" with StorageClass GLACIER), so those remain unverified too.
Why: a client that needs an archived object sooner than the Bulk window — the documented reason the upgrade path exists — has no way to escalate. Its only AWS-sanctioned move (re-issue RestoreObject with a faster Tier) comes back 409, so the caller concludes its own request was a duplicate and waits out the slow restore, or retries forever against a call that can never succeed.
Fix: the RestoreObject handler's in-progress branch and the per-object restore state it reads. That state must record the in-flight tier; the handler must accept a strictly faster tier with 202 (updating the in-flight tier, leaving ongoing-request="true" and the storage class untouched) and refuse only an equal or slower tier with 409 RestoreAlreadyInProgress.
Traces: s3.deep-s3-storage-class-restore-tier-downgrade-rejected

Cause 2 of 2: the ExpectedBucketOwner guard is evaluated before the bucket-existence check, so a missing bucket is reported as AccessDenied.
AWS: a bucket control-plane read of a bucket that does not exist is refused NoSuchBucket, HTTP 404, message "The specified bucket does not exist" — regardless of what x-amz-expected-bucket-owner carries. The owner guard applies only once a bucket has been resolved and is owned by a different account, where it refuses AccessDenied (403). So the error class never depends on which account id a caller guessed, and GetBucketOwnershipControls on a never-created bucket is NoSuchBucket for both a foreign account id and the caller's own. Smithy: GetBucketOwnershipControls input ExpectedBucketOwner -> httpHeader x-amz-expected-bucket-owner; the operation models no errors, so both NoSuchBucket and AccessDenied are REST-documented error codes (recorded behavior only). In the message, "The specified bucket does not exist" is fixed text; the bucket name is the request-supplied part S3 reports in the error's BucketName element, not in the sentence.
Emulator: GetBucketOwnershipControls on a bucket that was never created, with ExpectedBucketOwner set to a foreign account id, is refused with error code AccessDenied instead of NoSuchBucket. The owner comparison is applied first and short-circuits the existence lookup, so the response leaks a different error class purely as a function of the header value.
Why: a client cannot tell 'this bucket does not exist' from 'this bucket belongs to someone else'. AccessDenied sends it down a permissions/ownership investigation — checking bucket policy, account id, cross-account grants — for a bucket that simply has to be created; and conversely a probe of a foreign-owned bucket is indistinguishable from a typo in the bucket name. Callers that branch on NoSuchBucket to create-on-demand never take that branch.
Fix: the shared ExpectedBucketOwner guard used by the bucket control-plane handlers (GetBucketOwnershipControls and its siblings): resolve the bucket first and refuse NoSuchBucket/404 when it is absent, and only compare x-amz-expected-bucket-owner against the resolved bucket's owner afterwards.
Traces: s3.s3-oc-get-on-a-missing-bucket-is-not-shortcircuited-by-a-foreign-expected-bucket-owner

Neither cause is covered by the open issues that look adjacent: 20261029T211510Z-s3-representation-divergences-lifecycle-transition-validator-restoreobject-202.md covers RestoreObject's 200-vs-202 status on the INITIAL restore (that part now passes) and does not carry the tier-upgrade refusal; 20261103T040901Z-s3-10-net-new-root-causes-bucket-policy.md and 20261102T111455Z-s3-public-access-block-acl-and-ownership-guards.md cover ownership-controls/PAB authorization, not the existence-before-owner ordering of the ExpectedBucketOwner guard.

## Evidence
Cause 1 — s3.deep-s3-storage-class-restore-tier-downgrade-rejected (verdict: error)
Sequence replayed: CreateBucket; PutObject(StorageClass=GLACIER); HeadObject -> StorageClass=GLACIER (ok); RestoreObject{Days:1, GlacierJobParameters:{Tier:"Bulk"}} -> HTTP 202 (ok); HeadObject -> Restore: ongoing-request="true" (ok); RestoreObject{Tier:"Bulk"} again -> RestoreAlreadyInProgress / "Object restore is already in progress" (ok); RestoreObject{Tier:"Standard"} (the upgrade).
Expected: HTTP 202 Accepted (tier upgrade accepted; restore stays ongoing; StorageClass stays GLACIER).
Actual: botocore.exceptions.ClientError: An error occurred (RestoreAlreadyInProgress) when calling the RestoreObject operation: Object restore is already in progress
  (raised out of the check body — the call is refused, not merely mis-serialized)

Cause 2 — s3.s3-oc-get-on-a-missing-bucket-is-not-shortcircuited-by-a-foreign-expected-bucket-owner (verdict: mismatch)
Call replayed: GetBucketOwnershipControls(Bucket=<never-created bucket>, ExpectedBucketOwner='111122223333').
Expected: error code NoSuchBucket, HTTP 404, message "The specified bucket does not exist" (and the same for ExpectedBucketOwner = the caller's own account id).
Actual: error code 'AccessDenied'.
AssertionError: a bucket that was never created must be NoSuchBucket regardless of ExpectedBucketOwner='111122223333'; got 'AccessDenied'
(The check's second iteration — the same call with the caller's own account id — and its positive control, GetBucketOwnershipControls on an existing bucket returning ['BucketOwnerEnforced'], were never reached.)

## Steps to Reproduce
Replay the behavioral suite and run the two s3 checks in trace_ids.

Cause 1: create a bucket; PutObject with StorageClass=GLACIER; RestoreObject with RestoreRequest={Days:1, GlacierJobParameters:{Tier:'Bulk'}} (expect 202); HeadObject and confirm Restore: ongoing-request="true"; repeat RestoreObject with Tier='Bulk' (expect 409 RestoreAlreadyInProgress, "Object restore is already in progress"); then RestoreObject with Tier='Standard' — expect HTTP 202 Accepted, observe RestoreAlreadyInProgress; finally re-issue Tier='Bulk' (expect 409) and HeadObject (expect Restore ongoing-request="true" and StorageClass GLACIER).

Cause 2: without creating it, call GetBucketOwnershipControls on a bucket name that does not exist, passing x-amz-expected-bucket-owner='111122223333' (a foreign account id) and then the caller's own account id — expect NoSuchBucket / 404 / "The specified bucket does not exist" in both cases, observe AccessDenied. Positive control: create the bucket and call GetBucketOwnershipControls with the caller's own account id; it must return Rules=[{ObjectOwnership: BucketOwnerEnforced}].
