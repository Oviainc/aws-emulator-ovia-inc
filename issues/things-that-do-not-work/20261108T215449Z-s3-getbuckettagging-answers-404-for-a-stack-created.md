Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-cfns3cnt-bucket-side-reverse-lookup
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-cfns3cnt-bucket-side-reverse-lookup
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: s3|GetBucketTagging|2xx->4xx|$.Tagging
Issue-Key: s3|GetBucketTagging|2xx->4xx|status
Issue-Commit: 6211fb78e0841381478152dba6893fe342be7489

# s3: GetBucketTagging answers 404 for a stack-created bucket whose only tags are the aws:cloudformation reserved keys

> _Note: this issue was identified against the code base at commit `6211fb78e0841381478152dba6893fe342be7489`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketTagging

## Description
One net-new root cause of service `s3`. The cluster reports stub_path='' and owned_by=[], so no open issue carries its replay key; the open issue 20261106T193353Z-s3-stack-created-bucket-tag-set-lacks-the-three.md describes the same missing reserved tags on a 200->200 body key and does NOT own this one, where the omission escalates to a 404 status divergence.

Cause 1 of 1: a bucket provisioned by a CloudFormation stack and carrying no template tags is served as having NO tag set at all, so GetBucketTagging fails instead of returning the stack-ownership tags.
AWS: recorded behavior only for the tag provenance — the S3 Smithy model states just the container (GetBucketTagging, GET /{Bucket}?tagging, output @xmlName Tagging with required member TagSet, a list of Key/Value Tag pairs) and says nothing about reserved keys. AWS CloudFormation stamps every resource it provisions — here an AWS::S3::Bucket — with three reserved tags: aws:cloudformation:stack-id = the owning stack's ARN, aws:cloudformation:stack-name = the stack's name, aws:cloudformation:logical-id = the resource's logical id in the template. Because those tags exist, the bucket's tag set is never empty and GetBucketTagging answers HTTP 200 with Tagging.TagSet holding all three Tag entries. The three KEYS are the fixed requirement; the values come from the owning stack (ARN, name) and from the template (logical id) — the recording's stack ARN, stack name and logical id 'Reg' are per-run values. (AWS reserves the 404 NoSuchTagSet answer for a bucket that genuinely has no tags.)
Emulator: GetBucketTagging answers HTTP 404 and no Tagging element at all, because it stamped none of the three reserved tags and the bucket therefore looks untagged.
Why: worse than missing metadata — the call FAILS. A client holding a bucket name cannot discover the owning stack, cannot tell a stack-managed bucket from a hand-made one, and any tag read over stack-provisioned buckets errors out instead of returning the stack attribution; code that treats 404 NoSuchTagSet as 'this bucket is untagged' records the wrong fact.
Fix: the AWS::S3::Bucket provisioning path in the CloudFormation resource provider must merge the three reserved tags into the bucket tag state that the S3 GetBucketTagging handler serializes, which also removes the empty-tag-set 404.
Traces: cloudformation-cfns3cnt-bucket-side-reverse-lookup#task#11.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout. Divergence cluster: divergence_key 'service=s3|GetBucketTagging|status=200->404|keys=$.Tagging,status', verdict mismatch, affected_count 1, stub_path='', owned_by=[]. Fixture cloudformation-cfns3cnt-bucket-side-reverse-lookup, step task#11, operation GetBucketTagging.
- status: expected 200, got 404.
- $.Tagging: missing — expected {'TagSet': {'Tag': [{'Key': 'aws:cloudformation:stack-id', 'Value': the owning stack's ARN}, {'Key': 'aws:cloudformation:stack-name', 'Value': the stack's name}, {'Key': 'aws:cloudformation:logical-id', 'Value': the resource's logical id in the template}]}}, got None.
The stack ARN and uuid, the generated stack name and the logical id in this recording are per-run values filling those slots; the requirement is HTTP 200 with the three reserved keys present.

## Steps to Reproduce
1. Clone this checkout, build the emulator and replay the simparity-wire-aws-v1 wire corpus against it.
2. Replay fixture cloudformation-cfns3cnt-bucket-side-reverse-lookup step task#11: deploy the fixture's CloudFormation stack declaring an AWS::S3::Bucket with no template-declared tags, wait for the stack to settle, then call S3 GetBucketTagging on the created bucket.
3. Expect HTTP 200 with Tagging.TagSet containing aws:cloudformation:stack-id, aws:cloudformation:stack-name and aws:cloudformation:logical-id. Observe HTTP 404 with no Tagging element.
