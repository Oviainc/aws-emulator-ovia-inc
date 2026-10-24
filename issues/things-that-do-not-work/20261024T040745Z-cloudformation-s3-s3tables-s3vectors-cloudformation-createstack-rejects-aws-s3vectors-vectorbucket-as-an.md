Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__s3tables__s3vectors.cfn-s3-bucket-variant-physical-ids-resolve-in-owning-services cloudformation__s3__s3tables__s3vectors.cfn-s3-lookalike-s3-resource-types-do-not-make-a-stack-count cloudformation__s3__s3tables__s3vectors.cfn-s3-template-summary-and-described-resources-agree-on-variants cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back
Issue-Key: cloudformation__s3__s3tables__s3vectors|CreateStack|ValidationError
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# CloudFormation CreateStack rejects AWS::S3Vectors::VectorBucket as an unrecognized resource type, so every S3-bucket-variant template is refused before provisioning

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause, reached by all four failing checks in this service set.

=== Cause 1 of 1: the CloudFormation template resource-type registry does not contain AWS::S3Vectors::VectorBucket, so CreateStack refuses the whole template with ValidationError ===

AWS: AWS::S3Vectors::VectorBucket is a supported CloudFormation resource type (AWS CloudFormation resource type reference for Amazon S3 Vectors; the emulator itself already ships S3 Vectors — services/service-com-amazonaws-s3vectors-s3vectors). CreateStack therefore ACCEPTS a template declaring it and returns a StackId, and the stack then proceeds to provision (or fail) that resource. Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json) models only AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException and TokenAlreadyExistsException on CreateStack; 'ValidationError' is CloudFormation's generic 400 request-validation code and is correct ONLY for a template the service genuinely cannot parse or whose types it genuinely does not support — recorded behavior only for the exact text. Note the sibling variant type in the SAME templates, AWS::S3Tables::TableBucket, is NOT named in the emulator's unrecognized-types list, so the registry is selectively missing the S3 Vectors entry rather than missing bucket variants wholesale.

Emulator: every CreateStack carrying that type is refused up front with HTTP 400, error code ValidationError, message example 'Template format error: Unrecognized resource types: [AWS::S3Vectors::VectorBucket]'. No StackId is minted, no stack exists, and no resource row, event, Output or rollback is ever produced. All four checks die on their first CreateStack call, before any assertion they were written to grade can run — the physical-id/Ref identity check, the lookalike-type stack-count derivation, the template-summary vs described-resources agreement check, and the invalid-vector-bucket-name rollback check.

Why: a client that deploys its vector store by template cannot deploy at all. The failure is not a wrong value a caller could work around — it is a hard refusal of a valid template, so every IaC flow that mixes S3 bucket variants (regular / vector / table) in one stack is unreachable, and so is every negative-path flow that depends on the stack being created in order to then roll back. CloudFormation is also the surface that is supposed to surface a bad template specifically; refusing a GOOD one makes that signal untrustworthy.

Also note: this changes the previously observed behavior recorded in the open issue 20261023T014718Z-cloudformation-s3-s3tables-s3vectors-cloudformation-does-not-propagate-an-s3-vectors-resource.md ('a stack declaring an invalid upper-case vector bucket name reaches CREATE_COMPLETE instead of ROLLBACK_COMPLETE'), and in Cause 2/3 of 20261023T014718Z-s3vectors-6-behavioural-divergences-no-iam-authorization-on.md ('a CloudFormation-deployed AWS::S3Vectors::VectorBucket reaches CREATE_COMPLETE but no vector bucket exists'). Those issues assume the type is accepted and the stack is created; it no longer is. This issue must be fixed first, or those issues cannot be re-replayed at all. Neither of them covers this refusal and the suite did not mark these checks as owned.

Fix: CloudFormation's template validation / supported-resource-type registry on the CreateStack path — register AWS::S3Vectors::VectorBucket (and its AWS::S3Vectors::Index sibling) alongside the already-registered AWS::S3Tables::TableBucket, and wire it to a provisioning handler that creates the bucket in s3vectors' state, exports its ARN as the PhysicalResourceId, and surfaces a CreateVectorBucket refusal as CREATE_FAILED / stack ROLLBACK_COMPLETE.

Traces: cloudformation__s3__s3tables__s3vectors.cfn-s3-bucket-variant-physical-ids-resolve-in-owning-services, cloudformation__s3__s3tables__s3vectors.cfn-s3-lookalike-s3-resource-types-do-not-make-a-stack-count, cloudformation__s3__s3tables__s3vectors.cfn-s3-template-summary-and-described-resources-agree-on-variants, cloudformation__s3__s3tables__s3vectors.cfn-s3vectors-invalid-vector-bucket-name-rejected-and-stack-rolls-back.

## Evidence
Expected: CreateStack with TemplateBody containing {"Vectors": {"Type": "AWS::S3Vectors::VectorBucket", "Properties": {"VectorBucketName": "<lower-case-name>"}}} returns a StackId (HTTP 200) and the stack proceeds to a terminal status (CREATE_COMPLETE for a valid name, ROLLBACK_COMPLETE for a name S3 Vectors refuses).

Actual, identically for all four checks: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::S3Vectors::VectorBucket]  (HTTP 400, error code ValidationError; message quoted as a recorded example). The sibling type AWS::S3Tables::TableBucket present in the same templates is not listed as unrecognized.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite) against the built emulator.
2. Call cloudformation CreateStack with TemplateBody = {"AWSTemplateFormatVersion": "2010-09-09", "Resources": {"Vectors": {"Type": "AWS::S3Vectors::VectorBucket", "Properties": {"VectorBucketName": "sim2real-probe-vec"}}}}.
3. Observe HTTP 400 / error code ValidationError with 'Template format error: Unrecognized resource types: [AWS::S3Vectors::VectorBucket]' instead of a StackId.
4. Repeat with a template mixing AWS::S3::Bucket + AWS::S3Vectors::VectorBucket + AWS::S3Tables::TableBucket: only the S3Vectors type is reported unrecognized, confirming the registry gap is specific to it.
5. Once registered, confirm the downstream expectations: ListStackResources shows PhysicalResourceId as the vector bucket ARN ending in ':bucket/<name>', Ref output equals that id, s3vectors GetVectorBucket(vectorBucketArn=<id>) resolves it to the declared name, and an upper-case VectorBucketName drives the resource to CREATE_FAILED and the stack to ROLLBACK_COMPLETE.
