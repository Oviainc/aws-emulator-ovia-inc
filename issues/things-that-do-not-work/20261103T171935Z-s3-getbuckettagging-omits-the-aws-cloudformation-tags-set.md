Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-refarch-s3ev-stack-lookup-from-bucket
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-refarch-s3ev-stack-lookup-from-bucket
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: s3|GetBucketTagging|2xx->2xx|$.Tagging.TagSet.Tag
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# S3 GetBucketTagging omits the aws:cloudformation tags set on a stack-created bucket

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketTagging

## Description
Cause 1 of 1 - the stack-ownership tags are absent from a CloudFormation-created bucket's tag set.
AWS: a bucket created by a CloudFormation stack carries the three reserved tags aws:cloudformation:stack-id (the stack ARN), aws:cloudformation:stack-name (the stack name) and aws:cloudformation:logical-id (the bucket's logical id), IN ADDITION to the tags the template declared, and GetBucketTagging returns all of them in Tagging.TagSet.
Emulator: TagSet contains only the template-declared tag, so the three reserved tags are missing.
Why: the bucket-to-stack reverse lookup starts from exactly these tags; without them a caller holding a bucket name cannot find its owning stack, and stack-scoped tag reporting on buckets is empty.
Fix: the AWS::S3::Bucket provisioning path must stamp the three reserved tags into the bucket tag set that GetBucketTagging reads, merged with the template's Tags.
Traces: cloudformation-refarch-s3ev-stack-lookup-from-bucket#task#3.

## Evidence
Replay of simparity-wire-aws-v1, fixture cloudformation-refarch-s3ev-stack-lookup-from-bucket step task#3 (stub_path='', owned_by=[]). Status 200 both sides. Divergence: $.Tagging.TagSet.Tag type_mismatch - expected four tags [aws:cloudformation:stack-id = the stack ARN, aws:cloudformation:stack-name = the stack name, aws:cloudformation:logical-id = the bucket's logical id, plus the template tag {Key 'team'}], got only the single template tag {Key 'team'}.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Fixture cloudformation-refarch-s3ev-stack-lookup-from-bucket, step task#3: deploy a stack declaring an AWS::S3::Bucket with one template tag, then call S3 GetBucketTagging on the bucket.
3. Check that Tagging.TagSet contains the three aws:cloudformation:* tags as well as the template tag.
