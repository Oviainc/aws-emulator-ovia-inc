Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__s3__s3control.deploy-s3express-directorybucket-bucketpolicy-accesspoint-round-trip-then-delete
Issue-Key: cloudformation__ec2__s3__s3control|CreateStack|ValidationError
Issue-Commit: 5469355dc4ba582ea958345f64905a5aea7e37f0

# cloudformation__ec2__s3__s3control: no AWS::S3Express::* resource type is registered — CreateStack refuses every S3 Express directory-bucket template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `5469355dc4ba582ea958345f64905a5aea7e37f0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's template processor does not know the AWS::S3Express::* resource types, so a stack that declares an S3 Express One Zone directory bucket, its bucket policy and its access point is refused at CreateStack and nothing is ever provisioned.

AWS: CloudFormation supports the resource types AWS::S3Express::DirectoryBucket, AWS::S3Express::BucketPolicy and AWS::S3Express::AccessPoint. CreateStack on a syntactically valid template that declares them is ACCEPTED (HTTP 200 with a StackId) and the stack proceeds to CREATE_COMPLETE, provisioning the directory bucket in S3 and the access point in S3 Control (recorded behavior only for the CloudFormation registry — the resource-type registry is not part of any Smithy model; the target surfaces are modelled, e.g. S3 Control CreateAccessPoint PUT /v20180820/accesspoint/{Name} -> 200 with AccessPointArn/Alias, and GetAccessPoint GET /v20180820/accesspoint/{Name} -> 200 with Name/Bucket/NetworkOrigin/AccessPointArn/Alias). CloudFormation only answers ValidationError "Template format error: Unrecognized resource types: [...]" for a type it genuinely does not support; the fixed text is "Template format error: Unrecognized resource types: " followed by the bracketed list of the offending type names taken from the request's template.

Emulator: CreateStack is refused with HTTP 400, Error.Code "ValidationError", Error.Message "Template format error: Unrecognized resource types: [AWS::S3Express::DirectoryBucket, AWS::S3Express::BucketPolicy, AWS::S3Express::AccessPoint]" — i.e. all three S3 Express types are absent from the emulator's resource-type registry. The stack is never created, so no directory bucket, bucket policy or access point exists and the round-trip-then-delete check cannot proceed past its first call.

Why: a client (console, CLI, CDK or any IaC pipeline) deploying an S3 Express One Zone stack gets a hard template-validation failure instead of a stack. The failure is indistinguishable from a genuine typo in the template, so the user is told their template is malformed when it is correct; nothing downstream of the stack (directory-bucket data path, access-point-scoped reads, stack deletion semantics) can be exercised at all.

Fix: the CloudFormation resource-type registry and the provisioning handlers for the three types — register AWS::S3Express::DirectoryBucket, AWS::S3Express::BucketPolicy and AWS::S3Express::AccessPoint, and back each with a handler that creates/updates/deletes the real underlying resource (S3 directory bucket, its policy, and the S3 Control access point) and resolves Ref and the types' GetAtt attributes.

Traces: cloudformation__ec2__s3__s3control.deploy-s3express-directorybucket-bucketpolicy-accesspoint-round-trip-then-delete. Sibling unregistered-type issues are already open for other services (e.g. 20261109T153103Z-cloudformation-cloudfront-ec2-elbv2-..., 20261109T155230Z-cloudformation-groundstation-...); none of them covers the AWS::S3Express::* family.

## Evidence
Expected: CreateStack accepts the template (200, StackId returned) and the stack reaches CREATE_COMPLETE with a real S3 Express directory bucket, bucket policy and access point; DeleteStack then removes them.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::S3Express::DirectoryBucket, AWS::S3Express::BucketPolicy, AWS::S3Express::AccessPoint]
Fixed text of the diverging message: "Template format error: Unrecognized resource types: "; variable part: the bracketed list of type names, taken from the request's Resources section.

## Steps to Reproduce
1. Build and boot the emulator, then point a botocore CloudFormation client at it.
2. Call CreateStack with a TemplateBody whose Resources declare an AWS::S3Express::DirectoryBucket, an AWS::S3Express::BucketPolicy for it and an AWS::S3Express::AccessPoint over it.
3. Observe the call is refused with ValidationError / "Template format error: Unrecognized resource types: [AWS::S3Express::DirectoryBucket, AWS::S3Express::BucketPolicy, AWS::S3Express::AccessPoint]" instead of returning a StackId.
4. Replay check cloudformation__ec2__s3__s3control.deploy-s3express-directorybucket-bucketpolicy-accesspoint-round-trip-then-delete.
