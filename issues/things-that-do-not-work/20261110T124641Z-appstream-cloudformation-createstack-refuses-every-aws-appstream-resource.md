Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream__cloudformation.deploy-appstream-fleet-stack-association-round-trip-and-delete appstream__cloudformation.deploy-appstream-imagebuilder-bad-image-rejected-and-valid-image-round-trip-and-delete
Issue-Key: appstream__cloudformation|CreateStack|ValidationError
Issue-Commit: 22d8facbc3b601c7c60aed2bc6871542e8162809

# appstream__cloudformation: CreateStack refuses every AWS::AppStream::* resource type as unrecognized, so no AppStream stack can be created

> _Note: this issue was identified against the code base at commit `22d8facbc3b601c7c60aed2bc6871542e8162809`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
1 net-new root cause in the CloudFormation resource-type registry for AppStream 2.0.

Cause 1 of 1: AWS::AppStream::Fleet, AWS::AppStream::Stack, AWS::AppStream::StackFleetAssociation and AWS::AppStream::ImageBuilder are not registered CloudFormation resource types, so CreateStack rejects the whole template at parse time instead of provisioning the resources.

AWS: CreateStack accepts a template whose Resources declare AWS::AppStream::Fleet, AWS::AppStream::Stack, AWS::AppStream::StackFleetAssociation or AWS::AppStream::ImageBuilder — all four are resource types in CloudFormation's public registry — and answers 200 with a StackId; the stack then provisions real AppStream fleets/stacks/associations/image builders which read back through the AppStream API, and a bad property value fails at the individual resource (CREATE_FAILED with the AppStream handler's reason, stack ROLLBACK_COMPLETE), never at template parse. Smithy: cloudformation#CreateStack declares only AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException and TokenAlreadyExistsException; the "Template format error: Unrecognized resource types: [...]" refusal is the service-common ValidationError (HTTP 400) and AWS emits it only for types its registry does not contain. Which types the registry contains is recorded behavior only.

Emulator: CreateStack answers HTTP 400 ValidationError with message "Template format error: Unrecognized resource types: [AWS::AppStream::Fleet, AWS::AppStream::Stack, AWS::AppStream::StackFleetAssociation]" for the fleet/stack/association template, and "Template format error: Unrecognized resource types: [AWS::AppStream::ImageBuilder]" for the image-builder template. Fixed text of that message is "Template format error: Unrecognized resource types: [" + the comma-separated list of the offending type names from the request + "]"; the bracketed names are values taken from the submitted template, not part of the requirement. Because the call is refused, no stack is ever created, so neither the happy-path round trip nor the bad-image rejection path can be exercised at all.

Why: a client deploying any AppStream 2.0 estate through CloudFormation (console, CLI, CDK, SAM) cannot create the stack at all — it receives a template-format error naming types that AWS supports, instead of a StackId and a provisioned fleet/stack pair. Negative-path fidelity is lost too: a caller cannot see the per-resource CREATE_FAILED/ROLLBACK_COMPLETE outcome AWS gives for an invalid ImageName, because the template never gets past parsing.

Fix: the CloudFormation template/resource-type registry and the AppStream resource provisioners — register AWS::AppStream::Fleet, AWS::AppStream::Stack, AWS::AppStream::StackFleetAssociation and AWS::AppStream::ImageBuilder in the type registry used by CreateStack's template validation, and back them with handlers that create/read/delete the corresponding AppStream entities and surface AppStream's own refusal as the resource's CREATE_FAILED reason.

Traces: appstream__cloudformation.deploy-appstream-fleet-stack-association-round-trip-and-delete, appstream__cloudformation.deploy-appstream-imagebuilder-bad-image-rejected-and-valid-image-round-trip-and-delete.

## Evidence
Expected (AWS): CreateStack returns 200 with a StackId; the stack reaches CREATE_COMPLETE with the AppStream fleet/stack/association readable through AppStream's DescribeFleets/DescribeStacks/DescribeAssociations, and the image-builder template with an invalid image ends CREATE_FAILED on the ImageBuilder resource with ROLLBACK_COMPLETE for the stack.

Actual (emulator), both checks fail in the very first CreateStack call:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::AppStream::Fleet, AWS::AppStream::Stack, AWS::AppStream::StackFleetAssociation]
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::AppStream::ImageBuilder]

Diverging status/code/message: HTTP 400, Error.Code "ValidationError", Error.Message "Template format error: Unrecognized resource types: [<types from the template>]" where AWS returns no error at all.

## Steps to Reproduce
1. Call cloudformation:CreateStack with a template whose Resources include an AWS::AppStream::Stack, an AWS::AppStream::Fleet and an AWS::AppStream::StackFleetAssociation that wires them together.
2. Observe the call is refused with ValidationError "Template format error: Unrecognized resource types: [AWS::AppStream::Fleet, AWS::AppStream::Stack, AWS::AppStream::StackFleetAssociation]" instead of returning a StackId.
3. Repeat with a template declaring a single AWS::AppStream::ImageBuilder; the same refusal names [AWS::AppStream::ImageBuilder].
4. Re-replay the two checks listed in trace_ids to confirm the fix.
