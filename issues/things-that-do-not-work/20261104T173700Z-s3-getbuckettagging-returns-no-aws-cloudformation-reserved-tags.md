Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-refarch-s3ev-stack-lookup-from-bucket
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-refarch-s3ev-stack-lookup-from-bucket
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: s3|GetBucketTagging|2xx->2xx|$.Tagging.TagSet.Tag[Key="aws
Issue-Commit: 62368bc9dd377bdd15bb70a03317a8f201bb87d7

# S3 GetBucketTagging returns no aws:cloudformation reserved tags on a stack-created bucket

> _Note: this issue was identified against the code base at commit `62368bc9dd377bdd15bb70a03317a8f201bb87d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketTagging

## Description
One net-new root cause of service `s3`. The replay reports this cluster with stub_path='' and owned_by=[], so no open issue carries its replay key. Open issue 20261103T171935Z-s3-getbuckettagging-omits-the-aws-cloudformation-tags-set.md describes the same shape on the same fixture but does NOT own this key (its recorded Issue-Key is 's3|GetBucketTagging|2xx->2xx|$.Tagging.TagSet.Tag', while the cluster reported now keys on the per-tag paths $.Tagging.TagSet.Tag[Key="aws:cloudformation:*"]), so the divergence is still live and must be closed here; closing both together is correct.

Cause 1 of 1: the three reserved stack-ownership tags are absent from the tag set of a bucket created by a CloudFormation stack.
AWS: recorded behavior only - the S3 Smithy model has nothing to say about tag provenance (GetBucketTagging returns Tagging.TagSet, a list of Key/Value pairs with no reserved-key semantics). AWS CloudFormation stamps every resource it provisions, including an AWS::S3::Bucket, with three reserved tags IN ADDITION to the tags the template declared: aws:cloudformation:stack-id = the owning stack's ARN, aws:cloudformation:stack-name = the stack's name, and aws:cloudformation:logical-id = the resource's logical id in the template. GetBucketTagging answers 200 and returns all of them in Tagging.TagSet. The tag KEYS are the fixed requirement; the values are taken from the owning stack (its ARN and name) and from the template (the bucket's logical id).
Emulator: GetBucketTagging answers 200 but Tagging.TagSet contains only the template-declared tag - all three aws:cloudformation:* entries are missing.
Why: those three tags ARE the bucket-to-stack reverse lookup. A client holding only a bucket name cannot discover the stack that owns it, cannot tell a stack-managed bucket from a hand-made one, and any stack-scoped tag report or cost/ownership attribution over buckets comes back empty.
Fix: the AWS::S3::Bucket provisioning path in the CloudFormation resource provider must merge the three reserved tags into the bucket tag set that the S3 GetBucketTagging handler reads out of bucket state, alongside the template's Tags.
Traces: cloudformation-refarch-s3ev-stack-lookup-from-bucket#task#3.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout; 1 divergence cluster, stub_path='' and owned_by=[]. Fixture cloudformation-refarch-s3ev-stack-lookup-from-bucket, step task#3, operation GetBucketTagging, divergence_key 'service=s3|GetBucketTagging|status=200->200|keys=$.Tagging.TagSet.Tag[Key="aws...'. Status 200 on both sides. Missing entries:
- $.Tagging.TagSet.Tag[Key="aws:cloudformation:stack-id"]: expected {Key: 'aws:cloudformation:stack-id', Value: <the owning stack's ARN>}, got None.
- $.Tagging.TagSet.Tag[Key="aws:cloudformation:stack-name"]: expected {Key: 'aws:cloudformation:stack-name', Value: <the owning stack's name>}, got None.
- $.Tagging.TagSet.Tag[Key="aws:cloudformation:logical-id"]: expected {Key: 'aws:cloudformation:logical-id', Value: the bucket's logical id in the template}, got None.
The stack ARN/uuid, the generated stack name and the probe suffixes in the recording are per-run values filling those slots; the requirement is the presence of the three reserved keys with values derived from the owning stack and template.

## Steps to Reproduce
1. Clone this checkout, build the emulator and replay the simparity-wire-aws-v1 wire corpus against it.
2. Replay fixture cloudformation-refarch-s3ev-stack-lookup-from-bucket step task#3: deploy a CloudFormation stack declaring an AWS::S3::Bucket (logical id EventNotificationBucket) with one template tag, wait for CREATE_COMPLETE, then call S3 GetBucketTagging on the created bucket.
3. Check Tagging.TagSet: it must contain aws:cloudformation:stack-id (the stack ARN), aws:cloudformation:stack-name (the stack name) and aws:cloudformation:logical-id (EventNotificationBucket) in addition to the template tag. Observe that only the template tag is returned.
