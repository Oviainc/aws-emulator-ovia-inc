Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__sagemaker.deploy-sagemaker-cfn-image-nonexistent-role-arn-rejected cloudformation__iam__sagemaker.deploy-sagemaker-cfn-image-with-role-read-back-then-gone
Issue-Key: cloudformation__iam__sagemaker|CreateStack|ValidationError
Issue-Commit: e901ea078181297e70b2ac3a5c8be54768ac76d3

# cloudformation (SageMaker Image stacks): CreateStack refuses AWS::SageMaker::Image as an unrecognized resource type

> _Note: this issue was identified against the code base at commit `e901ea078181297e70b2ac3a5c8be54768ac76d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, reached by both failing checks under this service heading.

Cause 1 of 1: AWS::SageMaker::Image is absent from the emulator's CloudFormation resource-type registry, so any stack that declares a SageMaker Image is refused at template validation and neither the positive round-trip nor the role-validation negative can be exercised.

AWS: recorded behavior only — resource-type support is a CloudFormation registry fact. AWS::SageMaker::Image fronts the SageMaker control plane's CreateImage/DescribeImage/DeleteImage (Smithy: services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json, CreateImage input ImageName + RoleArn required, errors ResourceInUse/ResourceLimitExceeded). In AWS, CreateStack on such a template returns 200 with a StackId; with a valid RoleArn the stack reaches CREATE_COMPLETE, DescribeImage resolves the image and DeleteStack removes it; with a RoleArn that names no role the Image resource goes CREATE_FAILED with the SageMaker refusal as its ResourceStatusReason and the stack ends ROLLBACK_COMPLETE. Being refused before any resource is attempted is not one of those outcomes.

Emulator: CreateStack returns HTTP 400 ValidationError with message "Template format error: Unrecognized resource types: [AWS::SageMaker::Image]" — fixed text "Template format error: Unrecognized resource types: [" ... "]" with the type list taken from the submitted template. No stack id is issued, so the checks cannot even reach DescribeStacks.

Why: a client that provisions Studio/SageMaker custom images via CloudFormation gets a template-format error that blames its own template, with no parameter it can change to succeed; and a client testing the negative path cannot observe the rollback-and-reason contract it relies on to distinguish a bad execution role from a transient failure.

Fix: the CloudFormation resource-type registry plus the AWS::SageMaker::Image provisioner (create -> CreateImage, read -> DescribeImage, delete -> DeleteImage), and the stack resource lifecycle that must surface the underlying SageMaker refusal as the resource's ResourceStatusReason and drive the stack to ROLLBACK_COMPLETE.

Traces: cloudformation__iam__sagemaker.deploy-sagemaker-cfn-image-with-role-read-back-then-gone (positive round trip), cloudformation__iam__sagemaker.deploy-sagemaker-cfn-image-nonexistent-role-arn-rejected (negative, nonexistent role ARN). The identical class of refusal for other AWS::SageMaker::* types is filed separately under the cloudformation__sagemaker and cloudformation__ec2__iam__sagemaker headings, since an issue carries one service token.

## Evidence
Both checks fail inside CreateStack with the same exception:

botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::SageMaker::Image]

- cloudformation__iam__sagemaker.deploy-sagemaker-cfn-image-nonexistent-role-arn-rejected [error]
- cloudformation__iam__sagemaker.deploy-sagemaker-cfn-image-with-role-read-back-then-gone [error]

Expected in AWS: CreateStack 200 + StackId; then DescribeStacks StackStatus CREATE_COMPLETE for the valid-role template (with DescribeImage resolving the created image and the image gone after DeleteStack), and CREATE_FAILED -> ROLLBACK_COMPLETE for the template whose RoleArn names no IAM role, the Image resource's ResourceStatusReason carrying SageMaker's refusal. Actual: HTTP 400 ValidationError at CreateStack, no stack and no image. Stack/image/role names in the recordings are per-run values; the requirement is that AWS::SageMaker::Image is a recognized type.

## Steps to Reproduce
1. Build and boot the emulator, run the behavioral suite and re-run the two checks in trace_ids.
2. Manually: CreateStack with {"Resources":{"Image":{"Type":"AWS::SageMaker::Image","Properties":{"ImageName":"demo-image","ImageRoleArn":"<an existing SageMaker execution role ARN>"}}}}. AWS returns a StackId and the stack reaches CREATE_COMPLETE; the emulator returns 400 ValidationError "Template format error: Unrecognized resource types: [AWS::SageMaker::Image]".
3. Repeat with ImageRoleArn pointing at a role that does not exist: AWS drives the stack to CREATE_FAILED then ROLLBACK_COMPLETE with the SageMaker refusal as the Image resource's ResourceStatusReason; the emulator again refuses the CreateStack call itself.
