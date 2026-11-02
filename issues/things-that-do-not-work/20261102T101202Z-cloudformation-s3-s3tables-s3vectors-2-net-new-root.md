Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__s3tables__s3vectors.cfn-s3-omitted-encryption-defaults-on-vector-and-table-buckets cloudformation__s3__s3tables__s3vectors.e2e-count-cfn-stacks-with-s3-buckets-lifecycle cloudformation__s3__s3tables__s3vectors.cfn-s3tables-invalid-table-bucket-name-rejected-and-stack-rolls-back
Issue-Key: s3tables|GetTableBucketEncryption|BadRequestException
Issue-Key: cloudformation__s3__s3tables__s3vectors|ListNamespaces|BadRequestException
Issue-Key: cloudformation__s3__s3tables__s3vectors.cfn-s3tables-invalid-table-bucket-name-rejected-and-stack-rolls-back|run#9|AssertionError|assert status == "ROLLBACK_COMPLETE", f"the invalid table stack reached {status!r}"
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation/s3/s3tables/s3vectors: 2 net-new root causes — a CFN-created table bucket's physical id is not a valid ARN, and an invalid table-bucket name is accepted

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTableBucketEncryption

## Description
Two distinct net-new root causes. Open issue 20261102T051310Z-cloudformation-s3-s3tables-s3vectors-cloudformation-aws-s3tables-tablebucket-physical-resource-id-ref-15f3b6.md describes the same physical-id family for a different replay key; the two keys below are not carried by any open issue.

Cause 1 of 2: AWS::S3Tables::TableBucket's physical resource id is not a table-bucket ARN, so every s3tables call on it is refused.
AWS: recorded behavior only — the resource's physical id (and Ref) is the table bucket's ARN, so GetTableBucket/GetTableBucketEncryption/ListNamespaces on it succeed and the omitted-encryption default (SSE-S3 / AES256) is readable.
Emulator: those calls answer BadRequestException 'The specified table bucket ARN is not valid.' — the physical id the stack reports is not an ARN.
Why: nothing a stack provisions in s3tables can be read or used afterwards; multi-service flows that pivot from CloudFormation into s3tables all fail at the first call.
Fix: the AWS::S3Tables::TableBucket resource handler must return the created bucket's ARN as its physical resource id.
Traces: cloudformation__s3__s3tables__s3vectors.cfn-s3-omitted-encryption-defaults-on-vector-and-table-buckets, cloudformation__s3__s3tables__s3vectors.e2e-count-cfn-stacks-with-s3-buckets-lifecycle.

Cause 2 of 2: an invalid table-bucket name is accepted and the stack completes.
AWS: recorded behavior only — a table-bucket name that violates the naming rules is refused by s3tables, the resource fails, and the stack rolls back to ROLLBACK_COMPLETE.
Emulator: the stack reaches CREATE_COMPLETE.
Why: a client gets a 'successful' stack holding a resource that cannot exist, and loses the name-validation refusal.
Fix: name validation in the AWS::S3Tables::TableBucket resource handler (plus rollback on failure).
Traces: cloudformation__s3__s3tables__s3vectors.cfn-s3tables-invalid-table-bucket-name-rejected-and-stack-rolls-back.

## Evidence
C1: botocore BadRequestException on GetTableBucketEncryption — 'The specified table bucket ARN is not valid.'; and [phase 3/5 solve] BadRequestException on ListNamespaces — 'The specified table bucket ARN is not valid.'
C2: "the invalid table stack reached 'CREATE_COMPLETE'" — expected ROLLBACK_COMPLETE.

## Steps to Reproduce
C1: deploy a stack with AWS::S3Tables::TableBucket (and an AWS::S3Vectors vector bucket) omitting encryption, take the resource's PhysicalResourceId and call GetTableBucketEncryption / ListNamespaces with it. C2: deploy a stack whose AWS::S3Tables::TableBucket name violates the naming rules and poll DescribeStacks.
