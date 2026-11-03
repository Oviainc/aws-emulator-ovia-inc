Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-cfns3cnt-bucket-side-reverse-lookup
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-cfns3cnt-bucket-side-reverse-lookup
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: s3tables|ListTableBuckets|2xx->2xx|$.tableBuckets[*]
Issue-Commit: 2d97abf0bacad2832802b49de9c950da1a09c669

# S3 Tables ListTableBuckets omits a table bucket created by a CloudFormation stack

> _Note: this issue was identified against the code base at commit `2d97abf0bacad2832802b49de9c950da1a09c669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTableBuckets

## Description
Cause 1 of 1 - ListTableBuckets returns no entry for a table bucket that a CloudFormation stack created.
AWS: a table bucket exists independently of how it was created, so ListTableBuckets includes it with arn (arn:<partition>:s3tables:<region>:<account>:bucket/<name>), name, ownerAccountId, type 'customer' and createdAt.
Emulator: tableBuckets has no entry for the bucket at all, so the bucket-side reverse lookup finds nothing.
Why: the bucket-side view is how a caller goes from a live resource back to its stack; an unlisted bucket makes a resource CloudFormation reports as created invisible to its own service, and any S3 Tables call that enumerates buckets misses it.
Fix: the AWS::S3Tables::TableBucket provisioning path must register the bucket in the S3 Tables service state that ListTableBuckets reads (the handler lists only buckets created through the S3 Tables API).
Traces: cloudformation-cfns3cnt-bucket-side-reverse-lookup#task#4.

## Evidence
Replay of simparity-wire-aws-v1, fixture cloudformation-cfns3cnt-bucket-side-reverse-lookup step task#4 (stub_path='', owned_by=[]). Status 200 both sides. Divergence: $.tableBuckets[0] missing - expected an entry with arn 'arn:aws:s3tables:<region>:<account>:bucket/<name>', name '<name>', ownerAccountId '<account>', type 'customer', createdAt and tableBucketId; got None.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 corpus against the emulator built from this checkout.
2. Fixture cloudformation-cfns3cnt-bucket-side-reverse-lookup, step task#4: deploy a stack declaring AWS::S3Tables::TableBucket, then call S3 Tables ListTableBuckets.
3. Check that tableBuckets contains the stack-created bucket with its ARN, name, ownerAccountId and type.
