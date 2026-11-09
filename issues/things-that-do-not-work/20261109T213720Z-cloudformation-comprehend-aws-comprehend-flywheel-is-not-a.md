Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__comprehend.deploy-comprehend-flywheel-round-trip-and-delete
Issue-Key: cloudformation__comprehend|CreateStack|ValidationError
Issue-Commit: 8139116c3695d3bac7d26cbae8c8bb3d87daa4c0

# cloudformation__comprehend: AWS::Comprehend::Flywheel is not a registered resource type — CreateStack refuses every flywheel template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `8139116c3695d3bac7d26cbae8c8bb3d87daa4c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's template processor does not know the resource type AWS::Comprehend::Flywheel, so a template that declares one is rejected at CreateStack and no flywheel is ever provisioned. Failing check: cloudformation__comprehend.deploy-comprehend-flywheel-round-trip-and-delete.

AWS: AWS::Comprehend::Flywheel is a supported CloudFormation resource type backed by Comprehend's flywheel API. Real CloudFormation accepts the template, provisions the flywheel through Comprehend (Smithy, com-amazonaws-comprehend-comprehend-20171127: CreateFlywheel — required input FlywheelName (ComprehendArnName, @length 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$), DataAccessRoleArn (IamRoleArn, pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$), DataLakeS3Uri (FlywheelS3Uri, pattern ^s3://...), plus TaskConfig, ModelType and Tags; output FlywheelArn matches ^arn:aws(-[^:]+)?:comprehend:[a-zA-Z0-9-]*:[0-9]{12}:flywheel/[a-zA-Z0-9](-*[a-zA-Z0-9])*$), and resolves Ref and Fn::GetAtt Arn to that flywheel ARN. Once CREATE_COMPLETE, Comprehend's own DescribeFlywheel (output FlywheelProperties) answers 200 with Status ACTIVE and echoes the declared DataAccessRoleArn, DataLakeS3Uri (nested as <declared-prefix>/<flywheel-name>/schemaVersion=<n>/<timestamp>/), ModelType and TaskConfig, ListFlywheels lists the flywheel once, and ListTagsForResource returns the template Tags. Deleting the stack deletes the flywheel, after which DescribeFlywheel refuses with 404 ResourceNotFoundException and the message "RESOURCE_NOT_FOUND: Could not find specified resource." (fixed text; no request-derived parts) — recorded behavior for the delete half of the round trip.

Emulator: CreateStack refuses the template outright with HTTP 400 and error code ValidationError, message "Template format error: Unrecognized resource types: [AWS::Comprehend::Flywheel]". The fixed text is "Template format error: Unrecognized resource types: [...]"; the bracketed list is the set of declared types the emulator's registry does not know — here AWS::Comprehend::Flywheel, taken from the request template. Because the stack is never created, every downstream assertion (stack Outputs Ref/GetAtt Arn, ListFlywheels, DescribeFlywheel properties and Status ACTIVE, ListTagsForResource, and the post-delete ResourceNotFoundException) is unreachable.

Why: a client that provisions Comprehend flywheels as infrastructure-as-code (CloudFormation, CDK, SAM) cannot deploy at all against the emulator — the deploy fails at template validation with a format error, which a client reads as "my template is malformed" rather than "this emulator lacks the type". No flywheel exists, so Comprehend reads that the template was supposed to make addressable return ResourceNotFoundException, and stack rollback/delete semantics for the type are never exercised.

Fix: CloudFormation's template resource-type registry plus a new AWS::Comprehend::Flywheel resource handler in the cloudformation service crate (services/service-com-amazonaws-cloudformation-cloudformation): register the type, map Properties (FlywheelName, DataAccessRoleArn, DataLakeS3Uri, ModelType, TaskConfig, Tags) onto Comprehend's CreateFlywheel, resolve Ref and GetAtt Arn to the flywheel ARN, record the physical id so stack delete calls DeleteFlywheel, and have the handler write through to Comprehend's flywheel state so DescribeFlywheel/ListFlywheels/ListTagsForResource see it.

Traces: cloudformation__comprehend.deploy-comprehend-flywheel-round-trip-and-delete.

## Evidence
Check cloudformation__comprehend.deploy-comprehend-flywheel-round-trip-and-delete.

Expected (real AWS): CreateStack with a template containing Resources.Flywheel of Type AWS::Comprehend::Flywheel (plus AWS::S3::Bucket and AWS::IAM::Role) is accepted; the stack reaches CREATE_COMPLETE; Outputs FlywheelArn (Ref) and FlywheelArnAttr (Fn::GetAtt Flywheel.Arn) both equal arn:<partition>:comprehend:<region>:<account>:flywheel/<FlywheelName>; ListFlywheels lists that ARN exactly once; DescribeFlywheel(FlywheelArn=<that arn>) returns FlywheelProperties with Status 'ACTIVE', the stack role's DataAccessRoleArn, a DataLakeS3Uri under s3://<bucket>/datalake/<FlywheelName>/schemaVersion=<n>/..., ModelType 'DOCUMENT_CLASSIFIER' and TaskConfig {LanguageCode 'en', DocumentClassificationConfig {Mode 'MULTI_CLASS', Labels ['NEGATIVE','POSITIVE']}}; ListTagsForResource returns the template Tags; after DeleteStack, DescribeFlywheel raises ResourceNotFoundException with message 'RESOURCE_NOT_FOUND: Could not find specified resource.'

Actual (emulator): the very first call fails —
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Comprehend::Flywheel]
HTTP 400 / ValidationError. No stack, no flywheel, no subsequent assertion reached.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Call CloudFormation CreateStack with a template whose Resources include an AWS::S3::Bucket, an AWS::IAM::Role trusting comprehend.amazonaws.com, and a resource of Type "AWS::Comprehend::Flywheel" with Properties FlywheelName, DataAccessRoleArn (Fn::GetAtt on the role), DataLakeS3Uri (Fn::Sub s3://${Bucket}/datalake), ModelType "DOCUMENT_CLASSIFIER" and TaskConfig {LanguageCode: en, DocumentClassificationConfig: {Mode: MULTI_CLASS, Labels: [POSITIVE, NEGATIVE]}}, with Capabilities=["CAPABILITY_IAM"].
3. Observe the call fails with 400 ValidationError "Template format error: Unrecognized resource types: [AWS::Comprehend::Flywheel]" instead of returning a StackId.
4. Expected continuation once accepted: poll DescribeStacks to CREATE_COMPLETE, read Outputs Ref/GetAtt Arn, call comprehend ListFlywheels / DescribeFlywheel / ListTagsForResource, then DeleteStack and confirm DescribeFlywheel answers ResourceNotFoundException "RESOURCE_NOT_FOUND: Could not find specified resource."
