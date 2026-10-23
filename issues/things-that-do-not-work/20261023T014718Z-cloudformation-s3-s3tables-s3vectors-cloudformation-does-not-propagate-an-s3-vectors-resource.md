Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back
Issue-Key: cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back|run#9|AssertionError|assert status == "ROLLBACK_COMPLETE", f"the invalid vector stack reached {status!r}"
Issue-Commit: c1cdf4b562f81ba1ac84179127e5df6b90873b05

# CloudFormation does not propagate an S3 Vectors resource refusal: a stack declaring an invalid (upper-case) vector bucket name reaches CREATE_COMPLETE instead of ROLLBACK_COMPLETE

> _Note: this issue was identified against the code base at commit `c1cdf4b562f81ba1ac84179127e5df6b90873b05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause.

=== Cause 1 of 1: an AWS::S3Vectors::VectorBucket whose name S3 Vectors refuses still succeeds as a stack resource ===
AWS: a vector bucket name is constrained to the lower-case DNS-style form the service's own ARN pattern spells out — Smithy VectorBucketArn/IndexArn carry ^arn:aws[-a-z0-9]*:s3vectors:[a-z0-9-]+:[0-9]{12}:bucket/[a-z0-9][a-z0-9-.]{1,61}[a-z0-9]…$ and VectorBucketName is length 3..63 — so CreateVectorBucket refuses an upper-case name (HTTP 400 ValidationException). When that refusal happens inside a CloudFormation resource handler, the failure propagates: the resource goes CREATE_FAILED, the stack ends at ROLLBACK_COMPLETE, the resource row is left DELETE_COMPLETE, and S3 Vectors holds nothing under that name afterwards. A valid sibling stack in the same run still reaches CREATE_COMPLETE and counts.
Emulator: the stack reaches CREATE_COMPLETE — "AssertionError: the invalid vector stack reached 'CREATE_COMPLETE'". The CloudFormation resource handler for AWS::S3Vectors::VectorBucket accepts a name the service itself rejects, so no resource failure and no rollback ever occur.
Why: CloudFormation is the mechanism that is supposed to surface a bad template before anything is live. A client that depends on rollback to leave an account clean after a bad deploy gets the opposite signal here: a green stack whose declared resource does not exist (or exists under a name the data plane cannot address), and the error that would have named the offending property is never reported.
Fix: the CloudFormation provisioning path for the AWS::S3Vectors::VectorBucket resource type — route creation through S3 Vectors' CreateVectorBucket validation (or apply the same name constraint), surface the refusal as CREATE_FAILED on the resource, and drive the stack to ROLLBACK_COMPLETE. Note: this check's service set also names s3tables, which is NOT yet bootstrapped; the assertion that fails is the stack-status one above, before any s3tables use.
Traces: cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back.

## Evidence
Expected: a stack whose AWS::S3Vectors::VectorBucket declares an upper-case vector bucket name ends at StackStatus 'ROLLBACK_COMPLETE' (resource CREATE_FAILED, resource row DELETE_COMPLETE, no such vector bucket in S3 Vectors), while a valid sibling stack reaches CREATE_COMPLETE.
Actual: AssertionError: the invalid vector stack reached 'CREATE_COMPLETE' (check_cloudformation__s3__s3tables__s3vectors_cfn_s3vectors_invalid_vector_bucket_name_rejected_and_stack_rolls_back.py line 40: assert status == "ROLLBACK_COMPLETE").

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite) against the built emulator.
2. CreateStack with a template containing a single AWS::S3Vectors::VectorBucket whose VectorBucketName contains upper-case characters (a name CreateVectorBucket refuses).
3. Poll DescribeStacks for a terminal status — observe 'CREATE_COMPLETE' where AWS reports 'ROLLBACK_COMPLETE'.
4. Check the stack's resource status and then ListVectorBuckets: the resource should be CREATE_FAILED/DELETE_COMPLETE and no bucket should exist under that name.
5. Repeat with a valid lower-case name to confirm the valid sibling still reaches CREATE_COMPLETE.
