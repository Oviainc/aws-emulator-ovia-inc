Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-bucket-compare-e2e-structure-sizes-lifecycle s3.s3-csvcols-e2e-download-identify-columns-lifecycle s3.deploy-s3-1-bucket-update-versioning-policy-rename
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetBucketPolicy|NoSuchBucketPolicy
Issue-Key: s3.deploy-s3-1-bucket-update-versioning-policy-rename|_wait_stack#24|AssertionError|raise AssertionError(
Issue-Commit: 56c3e2d5db4619f120a986f75a3622f8f9af8979

# s3: 2 net-new root causes — a CloudFormation-declared AWS::S3::BucketPolicy never materialises, and AWS::S3::Bucket cannot be updated in place

> _Note: this issue was identified against the code base at commit `56c3e2d5db4619f120a986f75a3622f8f9af8979`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketPolicy

## Description
Two distinct net-new root causes under `### s3`, both in the CloudFormation → S3 provisioning path. Close both before submitting.

Cause 1 of 2: a CloudFormation-declared AWS::S3::BucketPolicy is never applied to the bucket.
AWS: recorded behavior. When a stack reaches CREATE_COMPLETE and declares an AWS::S3::BucketPolicy for one of its buckets, GetBucketPolicy on that bucket returns the policy document the template declared (HTTP 200 with the `Policy` payload). `NoSuchBucketPolicy` / "The bucket policy does not exist" is AWS's refusal only when no policy was ever put — the fixed message, with no variable part.
Emulator: the stack reports success, but the very first read of the bucket's policy fails with "An error occurred (NoSuchBucketPolicy) when calling the GetBucketPolicy operation: The bucket policy does not exist", i.e. the BucketPolicy resource was accepted into the stack but nothing was written to the bucket. Both checks abort in `[phase 1/5 deploy]`.
Why: every stack-deployed access rule (TLS-only Deny, cross-service grant, prefix scoping) is silently absent from the bucket, so the deployed estate is wide open relative to its own template and no later phase of a deploy-and-verify workflow can run.
Fix: the AWS::S3::BucketPolicy resource provider (its create/update must call the bucket-policy state), and the ordering that binds it to its AWS::S3::Bucket.
Traces: s3.s3-bucket-compare-e2e-structure-sizes-lifecycle, s3.s3-csvcols-e2e-download-identify-columns-lifecycle.

Cause 2 of 2: CloudFormation cannot update the properties of an existing AWS::S3::Bucket, so any stack update touching a bucket rolls back.
AWS: recorded behavior. An UpdateStack that changes mutable AWS::S3::Bucket properties (VersioningConfiguration, the attached policy, and a bucket rename that triggers replacement) is applied and the stack reaches UPDATE_COMPLETE; the bucket then reads back the new property values.
Emulator: the stack reaches `UPDATE_ROLLBACK_COMPLETE`, with the resource event `Bucket / UPDATE_FAILED / "NotImplemented: this emulator does not change the properties of an existing AWS::S3::Bucket in place; the resource was left as it is."` and the stack event `UPDATE_FAILED / "The following resource(s) failed to update: [Bucket]. "`.
Why: no S3 stack can ever be evolved — the second deployment of any template that owns a bucket fails, so the whole update/rollback lifecycle of an S3-bearing stack is unreachable.
Fix: the AWS::S3::Bucket resource provider's update path (in-place property update, and replacement where the property requires it).
Traces: s3.deploy-s3-1-bucket-update-versioning-policy-rename.

## Evidence
Cause 1 — s3.s3-bucket-compare-e2e-structure-sizes-lifecycle: "_PhaseError: [phase 1/5 deploy] ClientError: An error occurred (NoSuchBucketPolicy) when calling the GetBucketPolicy operation: The bucket policy does not exist"; s3.s3-csvcols-e2e-download-identify-columns-lifecycle: the identical PhaseError in phase 1/5. Expected: HTTP 200 with the declared Policy document.
Cause 2 — s3.deploy-s3-1-bucket-update-versioning-policy-rename: "stack <name> reached 'UPDATE_ROLLBACK_COMPLETE', expected 'UPDATE_COMPLETE'; failures: [(<stack>, 'UPDATE_FAILED', 'The following resource(s) failed to update: [Bucket]. '), ('Bucket', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::S3::Bucket in place; the resource was left as it is.')]".

## Steps to Reproduce
1. Deploy a stack containing an AWS::S3::Bucket and an AWS::S3::BucketPolicy that attaches a Deny statement to it; wait for CREATE_COMPLETE, then call GetBucketPolicy on the bucket — expect 200 with the declared document, observe NoSuchBucketPolicy.
2. Deploy a stack with an AWS::S3::Bucket, then update the template to enable VersioningConfiguration and change the attached policy and the bucket name; run UpdateStack — expect UPDATE_COMPLETE, observe UPDATE_ROLLBACK_COMPLETE with the NotImplemented resource event above.
3. Re-replay the two check ids in trace_ids.
