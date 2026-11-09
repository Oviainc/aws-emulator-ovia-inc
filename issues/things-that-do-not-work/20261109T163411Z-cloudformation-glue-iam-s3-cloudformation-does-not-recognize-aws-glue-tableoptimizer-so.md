Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__glue__iam__s3.deploy-glue-table-optimizer-lifecycle cloudformation__glue__iam__s3.deploy-glue-table-optimizer-invalid-type-rejected
Issue-Key: cloudformation__glue__iam__s3|CreateStack|ValidationError
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# CloudFormation does not recognize AWS::Glue::TableOptimizer, so every stack declaring one is refused at CreateStack

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation resource-type registry has no AWS::Glue::TableOptimizer.

AWS: CreateStack accepts a template declaring AWS::Glue::TableOptimizer (CatalogId, DatabaseName, TableName, Type, TableOptimizerConfiguration{RoleArn, Enabled}) and provisions it through Glue's CreateTableOptimizer; the stack reaches CREATE_COMPLETE, GetTableOptimizer reads it back, and DeleteStack removes it. A Type outside the supported set is refused by the resource handler (resource CREATE_FAILED, stack ROLLBACK_COMPLETE), not by template parsing. Smithy: CreateStack.TemplateBody is opaque, so the registered type set is a service fact and not modelled — recorded behavior only.

Emulator: CreateStack answers 400 with error code `ValidationError` and message `Template format error: Unrecognized resource types: [AWS::Glue::TableOptimizer]`. Fixed text: `Template format error: Unrecognized resource types: [` + `]`; the bracketed list comes from the submitted template.

Why: Iceberg table compaction/optimizer configuration cannot be declared in IaC; the client gets a syntax error instead of either a provisioned resource or a handler-level refusal of an invalid Type.

Fix: the CloudFormation resource-type registry plus a create/read/delete provisioner for AWS::Glue::TableOptimizer wired to Glue's table-optimizer state, with Type validation at handler level.

Traces: cloudformation__glue__iam__s3.deploy-glue-table-optimizer-lifecycle, cloudformation__glue__iam__s3.deploy-glue-table-optimizer-invalid-type-rejected.

## Evidence
Expected: CreateStack accepted; stack CREATE_COMPLETE, or ROLLBACK_COMPLETE for the invalid optimizer Type.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Glue::TableOptimizer]

## Steps to Reproduce
1. Create a Glue database/table over an S3 location and a role Glue can assume.
2. CreateStack with a Resources entry of Type AWS::Glue::TableOptimizer referencing them.
3. Observe the CreateStack call fail with ValidationError / 'Unrecognized resource types'.
