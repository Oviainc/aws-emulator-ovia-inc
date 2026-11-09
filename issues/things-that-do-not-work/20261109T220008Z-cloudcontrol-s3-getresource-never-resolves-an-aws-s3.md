Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__s3.native-s3-bucket-delete-leaves-cloudcontrol-not-found
Issue-Key: cloudcontrol__s3.native-s3-bucket-delete-leaves-cloudcontrol-not-found|_poll_until#9|AssertionError|raise AssertionError(
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# cloudcontrol__s3: GetResource never resolves an AWS::S3::Bucket that exists but was not created through Cloud Control, so the post-delete ResourceNotFoundException can never be observed

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
One cause.

Cause 1 of 1: Cloud Control reads only resources it minted itself instead of reading the owning service's state.

AWS: Cloud Control API is a uniform read/write surface over the live resource, not a private registry. GetResource(TypeName='AWS::S3::Bucket', Identifier=<bucket>) describes any existing bucket in the account/region regardless of how it was created, and after the bucket is deleted natively (s3:DeleteBucket) the same call fails with ResourceNotFoundException (Smithy: cloudcontrol GetResource errors include ResourceNotFoundException, HTTP 404-class client error). The documented rule is that the identifier is the resource's primary identifier in the owning service.

Emulator: GetResource for a bucket created with s3:CreateBucket never resolves at all — polled for 30s, 'last observation None' — so the emulator neither describes the live bucket nor answers a definite ResourceNotFoundException. The check cannot even reach the delete step whose refusal it is asserting.

Why: Cloud Control is the single surface IaC tools (and `aws cloudcontrol get-resource`) use to reconcile drift. If it sees only Cloud-Control-created resources, every reconciliation of pre-existing infrastructure reports nothing, and callers cannot distinguish 'resource absent' from 'Cloud Control has no record'.

Fix: the Cloud Control GetResource handler (and its state lookup) must resolve the identifier against the owning service's resource state — the S3 bucket store — rather than a Cloud-Control-local registry, and raise ResourceNotFoundException once the owning service no longer has it.

Traces: cloudcontrol__s3.native-s3-bucket-delete-leaves-cloudcontrol-not-found.

## Evidence
Expected: GetResource(TypeName='AWS::S3::Bucket', Identifier=<bucket created by s3:CreateBucket>) describes the bucket; after s3:DeleteBucket the same call raises ResourceNotFoundException.
Actual: AssertionError: waited 30.0s for Cloud Control GetResource to resolve 'sim2real-behav-ccnat-<id>', a bucket it never created and it never happened; last observation None.

## Steps to Reproduce
1. s3.create_bucket(Bucket=b) (natively, not through Cloud Control).
2. cloudcontrol.get_resource(TypeName='AWS::S3::Bucket', Identifier=b) -> never resolves (no description, no error).
3. (Unreachable) s3.delete_bucket(Bucket=b) then get_resource again -> expect ResourceNotFoundException.
