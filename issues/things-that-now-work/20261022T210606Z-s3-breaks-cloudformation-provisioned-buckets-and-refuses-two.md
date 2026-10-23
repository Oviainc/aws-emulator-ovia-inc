Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.cfn-deploy-sagemaker-endpoint-config-stack-positive s3__sagemaker.cfn-deploy-sagemaker-missing-s3-bucket-policy-negative s3__sagemaker.cfn-deploy-sagemaker-model-s3-artifact-arn-resolution s3__sagemaker.cfn-deploy-sagemaker-model-s3-artifact-reference-positive s3__sagemaker.cfn-deploy-sagemaker-role-missing-s3-trust-negative s3__sagemaker.cfn-deploy-sagemaker-role-without-s3-trust-negative s3__sagemaker.cfn-deploy-sagemaker-training-job-stack-positive s3__sagemaker.cfn-deploy-sagemaker-training-job-stack-s3-data-positive s3__sagemaker.cfn-stack-update-sagemaker-model-data-url-positive s3__sagemaker.deploy-cfn-sagemaker-bucket-policy-denies-role-negative s3__sagemaker.deploy-cfn-sagemaker-role-missing-s3-permission-negative s3__sagemaker.deploy-cfn-sagemaker-training-job-s3-bucket-wiring-positive s3__sagemaker.cfn-deploy-sagemaker-endpoint-config-s3-kms-negative s3__sagemaker.cli-s3-upload-then-sagemaker-processing-job-workflow
Issue-Key: s3__sagemaker|PutObject|NoSuchBucket
Issue-Key: s3__sagemaker|PutObject|NotImplemented
Issue-Key: s3__sagemaker.cli-s3-upload-then-sagemaker-processing-job-workflow|_create_processing_job_with_role_retry#8|AssertionError|raise AssertionError(f"create-processing-job script failed: {proc.stderr[-500:]}")
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# S3 breaks CloudFormation-provisioned buckets and refuses two modeled request members: NoSuchBucket for a stack's bucket, empty error Message, SSE PutObject and encoding-type ListObjectsV2 stubs (4 causes)

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Four independent S3 defects, each surfaced by the SageMaker/S3 cross-service behavioral checks. Each is its own root cause and its own work item; this issue bundles FOUR causes and all four must be closed before submitting.

Cause 1 of 4: a bucket declared by a CloudFormation stack does not exist in S3 after the stack reports success.
AWS: recorded behavior only. The checks deploy a template whose Resources declare `DataBucket: {Type: AWS::S3::Bucket}` and whose Outputs expose `BucketName: !Ref DataBucket`; once the stack reaches CREATE_COMPLETE the returned bucket name is a real bucket and `PutObject`/`GetObject` against it return 200 (Smithy: PutObject PUT /{Bucket}/{Key+}, success 200; its modeled client errors are EncryptionTypeMismatch/InvalidRequest/InvalidWriteOffset/TooManyParts - NoSuchBucket is not one of them).
Emulator: the stack is reported created, but the first `PutObject` against the stack's BucketName output fails 404 `NoSuchBucket`, i.e. the name the stack hands back names no bucket in S3 (either the AWS::S3::Bucket resource was never provisioned, or !Ref resolves to a synthesized logical name S3 does not know).
Why: every deploy-then-use workflow is broken at step one - a client that provisions its data bucket by template and then writes training/model data into it cannot proceed at all, and the failure looks like an application bug (missing bucket) rather than a provisioning gap.
Fix: the CloudFormation resource provider for AWS::S3::Bucket (create the bucket in S3 state and return its real physical id from !Ref/!GetAtt), plus the stack-output resolution that feeds it. Related but distinct open issues that do NOT cover this resource type: 20261022T154846Z-ec2-emr-cloudformation-stack-outputs-return-synthesized-logical-names-instead.md and 20261022T175927Z-ds-ec2-e2e-cloudformation-reports-create-complete-but.md.
Traces: s3__sagemaker.cfn-deploy-sagemaker-endpoint-config-stack-positive, s3__sagemaker.cfn-deploy-sagemaker-missing-s3-bucket-policy-negative, s3__sagemaker.cfn-deploy-sagemaker-model-s3-artifact-arn-resolution, s3__sagemaker.cfn-deploy-sagemaker-model-s3-artifact-reference-positive, s3__sagemaker.cfn-deploy-sagemaker-role-missing-s3-trust-negative, s3__sagemaker.cfn-deploy-sagemaker-role-without-s3-trust-negative, s3__sagemaker.cfn-deploy-sagemaker-training-job-stack-positive, s3__sagemaker.cfn-deploy-sagemaker-training-job-stack-s3-data-positive, s3__sagemaker.cfn-stack-update-sagemaker-model-data-url-positive, s3__sagemaker.deploy-cfn-sagemaker-bucket-policy-denies-role-negative, s3__sagemaker.deploy-cfn-sagemaker-role-missing-s3-permission-negative, s3__sagemaker.deploy-cfn-sagemaker-training-job-s3-bucket-wiring-positive.

Cause 2 of 4: the NoSuchBucket error response carries no Message.
AWS: Smithy models NoSuchBucket as a client error (httpError 404, e.g. bound to ListObjectsV2); AWS's REST-XML error body always carries a <Message> element, and for this code the message is the documented fixed text "The specified bucket does not exist" - the rule is known, so a Message must be present.
Emulator: the error body has a Code but no usable Message; botocore renders the failure as "An error occurred (NoSuchBucket) when calling the PutObject operation: Unknown".
Why: any client, log pipeline or operator that surfaces the error message gets the literal string "Unknown"; error-reporting and message-based assertions downstream see nothing describing the fault.
Fix: the S3 error serializer that renders the XML <Error> envelope (emit Message for NoSuchBucket and the other canned S3 codes), not the individual handlers.
Traces: same checks as Cause 1 (the NoSuchBucket responses are visible in each), representatively s3__sagemaker.cfn-deploy-sagemaker-endpoint-config-stack-positive and s3__sagemaker.deploy-cfn-sagemaker-training-job-s3-bucket-wiring-positive.

Cause 3 of 4: PutObject carrying x-amz-server-side-encryption answers NotImplemented.
AWS: Smithy: PutObject.ServerSideEncryption is a modeled input member bound to the httpHeader `x-amz-server-side-encryption` (enum), with SSEKMSKeyId bound to `x-amz-server-side-encryption-aws-kms-key-id`; a PutObject that sets them returns 200 and echoes ServerSideEncryption in the response header. NotImplemented is not a modeled error for this operation.
Emulator: returns the scaffold error `NotImplemented` with message "operation PutObject with x-amz-server-side-encryption is scaffolded but not implemented in this emulator", i.e. the variant is gated off rather than handled.
Why: no client can write an SSE/SSE-KMS-encrypted object, and the unmodeled error code is treated by SDK retry logic as a server fault rather than a client error; every encryption-at-rest scenario is unreachable.
Fix: the PutObject handler plus its SSE header handling and object metadata state in the s3 service crate.
Traces: s3__sagemaker.cfn-deploy-sagemaker-endpoint-config-s3-kms-negative.

Cause 4 of 4: ListObjectsV2 carrying the encoding-type query parameter answers NotImplemented.
AWS: Smithy: ListObjectsV2.EncodingType is a modeled input member bound to httpQuery `encoding-type`, and the output echoes EncodingType; the only modeled error is NoSuchBucket (404). The AWS CLI sends encoding-type=url by default, so this is the ordinary listing path.
Emulator: returns `NotImplemented` ("operation ListObjectsV2 with encoding-type is scaffolded but not implemented in this emulator"), which the CLI reports as a fatal error.
Why: plain `aws s3 ls`/`list-objects-v2` fails, so any workflow that enumerates a prefix (verifying uploaded training data, draining a bucket) cannot run.
Fix: the ListObjectsV2 handler - accept encoding-type, URL-encode the key/prefix/delimiter fields it covers, and echo EncodingType in the response.
Traces: s3__sagemaker.cli-s3-upload-then-sagemaker-processing-job-workflow.

## Evidence
Cause 1: "botocore.errorfactory.NoSuchBucket: An error occurred (NoSuchBucket) when calling the PutObject operation: Unknown" raised on the first put_object against the CFN stack's BucketName output, in all 12 listed cfn-deploy/deploy-cfn checks; the template declares AWS::S3::Bucket and Outputs BucketName: !Ref DataBucket.
Cause 2: the same responses carry no Message - botocore prints "... : Unknown" where AWS sends "The specified bucket does not exist".
Cause 3: "botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutObject operation: operation PutObject with x-amz-server-side-encryption is scaffolded but not implemented in this emulator" (s3__sagemaker.cfn-deploy-sagemaker-endpoint-config-s3-kms-negative).
Cause 4: "AssertionError: create-processing-job script failed: fatal error: An error occurred (NotImplemented) when calling the ListObjectsV2 operation: operation ListObjectsV2 with encoding-type is scaffolded but not implemented in this emulator" (s3__sagemaker.cli-s3-upload-then-sagemaker-processing-job-workflow).

## Steps to Reproduce
1. CreateStack with a template declaring `DataBucket: {Type: AWS::S3::Bucket}` and `Outputs: {BucketName: {Value: !Ref DataBucket}}`; wait for CREATE_COMPLETE; read the BucketName output.
2. PutObject(Bucket=<that output>, Key='train/train.csv', Body=b'...') -> expect 200; observe 404 NoSuchBucket whose body has no Message.
3. Create a bucket directly, then PutObject(Bucket=..., Key=..., Body=..., ServerSideEncryption='aws:kms', SSEKMSKeyId=<key>) -> expect 200 with x-amz-server-side-encryption echoed; observe NotImplemented.
4. ListObjectsV2(Bucket=..., EncodingType='url') (or `aws s3api list-objects-v2 --bucket ...`, which sends encoding-type by default) -> expect 200 ListBucketResult; observe NotImplemented.
