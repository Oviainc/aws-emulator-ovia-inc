Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__glue__iam.deploy-glue-ml-transform-lifecycle
Issue-Key: cloudformation__glue__iam|CreateStack|ValidationError
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# CloudFormation does not recognize AWS::Glue::MLTransform, so every stack declaring one is refused at CreateStack

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation resource-type registry has no AWS::Glue::MLTransform.

AWS: CreateStack accepts a template declaring AWS::Glue::MLTransform (Name, Role, InputRecordTables, TransformParameters, GlueVersion, …) and provisions the transform through Glue's CreateMLTransform; the stack reaches CREATE_COMPLETE, the transform is readable through GetMLTransform/GetAtt, an update is applied and DeleteStack deletes it. Smithy: CreateStack.TemplateBody is opaque, so the registered resource-type set is a service fact and not modelled — recorded behavior only; the recording shows the full create/read/update/delete lifecycle.

Emulator: CreateStack answers 400 with error code `ValidationError` and message `Template format error: Unrecognized resource types: [AWS::Glue::MLTransform]`. Fixed text: `Template format error: Unrecognized resource types: [` + `]`; the bracketed list comes from the submitted template.

Why: the Glue ML-transform lifecycle cannot be driven through CloudFormation at all; a client gets a template-syntax error for a template AWS accepts.

Fix: the CloudFormation resource-type registry plus a create/read/update/delete provisioner for AWS::Glue::MLTransform wired to Glue's ML-transform state.

Traces: cloudformation__glue__iam.deploy-glue-ml-transform-lifecycle.

## Evidence
Expected: CreateStack accepted and the ML transform lifecycle observable through the stack.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Glue::MLTransform]

## Steps to Reproduce
1. Create a Glue database/table and a Glue service role.
2. CreateStack with a Resources entry of Type AWS::Glue::MLTransform referencing them.
3. Observe the CreateStack call fail with ValidationError / 'Unrecognized resource types'.
