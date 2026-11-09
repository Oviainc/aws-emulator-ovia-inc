Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr__signer.deploy-ecr-signing-configuration-duplicate-rejected
Issue-Key: cloudformation__ecr__signer|CreateStack|ValidationError
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# cloudformation__ecr__signer: CreateStack refuses AWS::ECR::SigningConfiguration as an unrecognized resource type, so no ECR signing configuration can be deployed

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::ECR::SigningConfiguration is not a registered CloudFormation resource type.
AWS: CreateStack accepts a template declaring AWS::ECR::SigningConfiguration and provisions it; a second declaration of the same configuration is then refused by the resource handler with its already-exists reason. Recorded behavior only.
Emulator: CreateStack answers 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::ECR::SigningConfiguration]'. Fixed text 'Template format error: Unrecognized resource types: [' + the unrecognized type names + ']'.
Why: signing configurations cannot be declared by template, and the duplicate-configuration refusal the check grades is unreachable; a client's deploy fails at template validation rather than at the resource.
Fix: the CloudFormation resource-type registry plus a handler for the AWS::ECR::SigningConfiguration type.
Traces: cloudformation__ecr__signer.deploy-ecr-signing-configuration-duplicate-rejected.

## Evidence
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ECR::SigningConfiguration]

Expected: the first stack reaches CREATE_COMPLETE and the duplicate declaration is refused by the resource handler.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay cloudformation__ecr__signer.deploy-ecr-signing-configuration-duplicate-rejected.
3. Or call CreateStack with a template declaring AWS::ECR::SigningConfiguration.
