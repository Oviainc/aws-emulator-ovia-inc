Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__iam__lambda.deploy-lambda-capacity-provider-round-trip-and-delete cloudformation__ec2__iam__lambda.deploy-lambda-capacity-provider-invalid-architecture-rejected-and-rolled-back
Issue-Key: cloudformation__ec2__iam__lambda|CreateStack|ValidationError
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# CloudFormation does not recognize AWS::Lambda::CapacityProvider, so every stack declaring one is refused at CreateStack

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation resource-type registry has no AWS::Lambda::CapacityProvider.

AWS: CreateStack accepts a template declaring AWS::Lambda::CapacityProvider and provisions it; the stack reaches CREATE_COMPLETE, the resource is readable through DescribeStackResource/GetAtt and DeleteStack removes it. A capacity provider declaring an architecture the service does not support is refused by the resource handler, so the resource goes CREATE_FAILED and the stack ROLLBACK_COMPLETE — a handler outcome, not a template-parse error. Smithy: CreateStack.TemplateBody is opaque, so the registered resource-type set is a service fact and not modelled — recorded behavior only.

Emulator: CreateStack answers 400 with error code `ValidationError` and message `Template format error: Unrecognized resource types: [AWS::Lambda::CapacityProvider]`. Fixed text: `Template format error: Unrecognized resource types: [` + `]`; the bracketed list comes from the submitted template.

Why: the client cannot create the stack, and the invalid-architecture contract is unobservable because the template never reaches a handler.

Fix: the CloudFormation resource-type registry plus a create/read/delete provisioner for AWS::Lambda::CapacityProvider, with architecture validation at handler level.

Traces: cloudformation__ec2__iam__lambda.deploy-lambda-capacity-provider-round-trip-and-delete, cloudformation__ec2__iam__lambda.deploy-lambda-capacity-provider-invalid-architecture-rejected-and-rolled-back.

## Evidence
Expected: CreateStack accepted; stack CREATE_COMPLETE, or ROLLBACK_COMPLETE after the handler refuses the unsupported architecture.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Lambda::CapacityProvider]

## Steps to Reproduce
1. CreateStack with a Resources entry of Type AWS::Lambda::CapacityProvider.
2. Observe the CreateStack call fail with ValidationError / 'Unrecognized resource types'.
