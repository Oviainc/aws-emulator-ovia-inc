Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-cfns3cnt-bucket-side-reverse-lookup
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-cfns3cnt-bucket-side-reverse-lookup
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: s3vectors|ListTagsForResource|2xx->2xx|$.tags.aws
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# S3 Vectors ListTagsForResource omits the aws:cloudformation tags set on a stack-created vector bucket

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Cause 1 of 1 - the stack-ownership tags are absent from a CloudFormation-created vector bucket.
AWS: every resource CloudFormation creates for a stack carries the three reserved tags aws:cloudformation:stack-id (the stack ARN), aws:cloudformation:stack-name (the stack name) and aws:cloudformation:logical-id (the resource's logical id in the template), and the owning service returns them from its tag-read API - here S3 Vectors ListTagsForResource, alongside any template-declared tags.
Emulator: ListTagsForResource returns none of the three aws:cloudformation:* tags.
Why: those tags are the documented way to go from a live resource back to its stack and template position; without them the resource-to-stack reverse lookup is impossible and tag-based cost/ownership reporting loses the stack attribution.
Fix: the AWS::S3Vectors::VectorBucket provisioning path must stamp the three reserved tags into the resource's tag set that S3 Vectors ListTagsForResource reads.
Traces: cloudformation-cfns3cnt-bucket-side-reverse-lookup#task#9.

## Evidence
Replay of simparity-wire-aws-v1, fixture cloudformation-cfns3cnt-bucket-side-reverse-lookup step task#9 (stub_path='', owned_by=[]). Status 200 both sides. Divergences: $.tags.aws:cloudformation:stack-id missing (expected the stack ARN, got None); $.tags.aws:cloudformation:logical-id missing (expected the resource's logical id, got None); $.tags.aws:cloudformation:stack-name missing (expected the stack name, got None).

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Fixture cloudformation-cfns3cnt-bucket-side-reverse-lookup, step task#9: deploy a stack declaring an AWS::S3Vectors::VectorBucket, then call S3 Vectors ListTagsForResource on the bucket.
3. Check that tags contains aws:cloudformation:stack-id, aws:cloudformation:stack-name and aws:cloudformation:logical-id.
