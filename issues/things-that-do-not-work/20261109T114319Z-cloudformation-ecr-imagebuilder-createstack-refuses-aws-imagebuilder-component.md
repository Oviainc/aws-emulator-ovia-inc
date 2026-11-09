Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr__imagebuilder.deploy-imagebuilder-container-recipe-round-trip-and-delete cloudformation__ecr__imagebuilder.deploy-imagebuilder-container-recipe-unknown-repository-rejected-and-rolled-back
Issue-Key: cloudformation__ecr__imagebuilder|CreateStack|ValidationError
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# cloudformation__ecr__imagebuilder: CreateStack refuses AWS::ImageBuilder::Component / ContainerRecipe as unrecognized resource types, so container-recipe stacks cannot deploy

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's template validation does not know the AWS::ImageBuilder::* resource types, so a stack that declares an Image Builder container recipe (component + ContainerRecipe targeting an ECR repository) is refused at submit time. Failing checks: cloudformation__ecr__imagebuilder.deploy-imagebuilder-container-recipe-round-trip-and-delete, cloudformation__ecr__imagebuilder.deploy-imagebuilder-container-recipe-unknown-repository-rejected-and-rolled-back.

AWS: recorded behavior only for the registry contents — Smithy (services/service-com-amazonaws-cloudformation-cloudformation) models CreateStack -> CreateStackOutput{StackId} with only AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException and TokenAlreadyExistsException; which types are recognized is CloudFormation's registry. AWS's registry contains AWS::ImageBuilder::Component and AWS::ImageBuilder::ContainerRecipe, so the template is accepted (HTTP 200 + StackId) and the stack walks to CREATE_COMPLETE; in the unknown-ECR-repository case the failure is raised by Image Builder when the ContainerRecipe resource is created, so the resource ends CREATE_FAILED with the service's refusal as its ResourceStatusReason and the stack reaches ROLLBACK_COMPLETE, after which the rolled-back component is no longer listed. 'Template format error: Unrecognized resource types: [<types>]' with code ValidationError at HTTP 400 is reserved for types absent from the registry — fixed sentence, bracketed list taken from the submitted template.
Emulator: CreateStack answers HTTP 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::ImageBuilder::Component, AWS::ImageBuilder::ContainerRecipe]' for both checks. No stack is created, so the ECR-repository wiring, the imagebuilder container-recipe read-back, the rollback assertions and the delete/not-found verification never run.
Why: a client provisioning container recipes through CloudFormation/CDK cannot deploy at all and is told its template is malformed rather than that the provider is missing; the negative scenario cannot distinguish 'bad repository' from 'bad template', since the refusal arrives before any resource is attempted.
Fix: CloudFormation's supported-resource-type registry used by template validation in the CreateStack path, plus AWS::ImageBuilder::Component and AWS::ImageBuilder::ContainerRecipe resource handlers (create/read/delete, Ref/Fn::GetAtt) that provision through the imagebuilder service state and relay its refusal into the resource status reason.
Traces: the two cloudformation__ecr__imagebuilder.deploy-imagebuilder-container-recipe-* checks listed above.

## Evidence
Expected (AWS): CreateStack with a template declaring AWS::ImageBuilder::Component + AWS::ImageBuilder::ContainerRecipe (targetRepository pointing at an ECR repository) -> HTTP 200 {"StackId": ...}; DescribeStacks reaches CREATE_COMPLETE for the round-trip check and ROLLBACK_COMPLETE for the unknown-repository check, whose failed resource carries Image Builder's own refusal text.
Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ImageBuilder::Component, AWS::ImageBuilder::ContainerRecipe] — HTTP 400, no StackId, no stack.
Fixed text: 'Template format error: Unrecognized resource types: ' with code ValidationError; the bracketed list is request-derived. The defect is that these two registry types are listed as unrecognized.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Run cloudformation__ecr__imagebuilder.deploy-imagebuilder-container-recipe-round-trip-and-delete: it calls CreateStack with a template declaring an AWS::ImageBuilder::Component and an AWS::ImageBuilder::ContainerRecipe plus the ECR repository it targets.
3. Observe HTTP 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::ImageBuilder::Component, AWS::ImageBuilder::ContainerRecipe]' instead of HTTP 200 with a StackId.
4. Repeat with deploy-imagebuilder-container-recipe-unknown-repository-rejected-and-rolled-back, which must be accepted and then roll back on the ContainerRecipe resource.
