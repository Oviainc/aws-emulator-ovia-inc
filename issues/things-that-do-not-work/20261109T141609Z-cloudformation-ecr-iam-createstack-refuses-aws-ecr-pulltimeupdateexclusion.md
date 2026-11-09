Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecr__iam.deploy-ecr-pull-time-update-exclusion-duplicate-principal-rejected
Issue-Key: cloudformation__ecr__iam|CreateStack|ValidationError
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# cloudformation__ecr__iam: CreateStack refuses AWS::ECR::PullTimeUpdateExclusion as an unrecognized resource type, so the pull-time-update exclusion resource cannot be deployed

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::ECR::PullTimeUpdateExclusion is not a registered CloudFormation resource type.
AWS: CreateStack accepts a template declaring AWS::ECR::PullTimeUpdateExclusion and provisions it, so the registry's pull-time-update exclusion (and its duplicate-principal refusal) is reachable through the stack. Recorded behavior only — the recording shows the stack being created and the duplicate-principal case being refused by the resource handler.
Emulator: CreateStack answers 400 ValidationError with the message 'Template format error: Unrecognized resource types: [AWS::ECR::PullTimeUpdateExclusion]'. Fixed text 'Template format error: Unrecognized resource types: [' + the unrecognized type names + ']'.
Why: the stack never exists, so a client cannot declare pull-time-update exclusions in infrastructure-as-code and never reaches the duplicate-principal validation the resource is supposed to enforce.
Fix: the CloudFormation resource-type registry plus a handler for the AWS::ECR::PullTimeUpdateExclusion type.
Traces: cloudformation__ecr__iam.deploy-ecr-pull-time-update-exclusion-duplicate-principal-rejected.

## Evidence
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ECR::PullTimeUpdateExclusion]

Expected: the stack is created (and the duplicate-principal declaration is refused by the resource handler with its AlreadyExists/InvalidRequest reason).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay cloudformation__ecr__iam.deploy-ecr-pull-time-update-exclusion-duplicate-principal-rejected.
3. Or call CreateStack with a template whose Resources declare an AWS::ECR::PullTimeUpdateExclusion; observe the 400 ValidationError instead of a stack.
