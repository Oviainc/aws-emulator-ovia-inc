Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__personalize__s3.cfn-personalize-metric-attribution-round-trip-and-delete cloudformation__iam__personalize__s3.cfn-personalize-metric-attribution-invalid-expression-rejected-leaving-valid-attribution
Issue-Key: cloudformation__iam__personalize__s3|CreateStack|ValidationError
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# CloudFormation does not recognize AWS::Personalize::MetricAttribution, so every stack declaring one is refused at CreateStack

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation resource-type registry has no AWS::Personalize::MetricAttribution.

AWS: CreateStack accepts a template declaring AWS::Personalize::MetricAttribution (Name, DatasetGroupArn, Metrics[]{EventType, Expression, MetricName}, MetricsOutputConfig) and provisions it through Personalize's CreateMetricAttribution; the stack reaches CREATE_COMPLETE, the attribution is readable and DeleteStack removes it. A metric Expression that is not a valid aggregate is refused by the resource handler (resource CREATE_FAILED, stack ROLLBACK_COMPLETE) leaving a previously valid attribution intact — a handler outcome, not a template-parse error. Smithy: CreateStack.TemplateBody is opaque, so the registered type set is a service fact and not modelled — recorded behavior only.

Emulator: CreateStack answers 400 with error code `ValidationError` and message `Template format error: Unrecognized resource types: [AWS::Personalize::MetricAttribution]`. Fixed text: `Template format error: Unrecognized resource types: [` + `]`; the bracketed list comes from the submitted template.

Why: a client cannot declare Personalize metric attributions in IaC, and gets a template-syntax error for a template AWS accepts.

Fix: the CloudFormation resource-type registry plus a create/read/delete provisioner for AWS::Personalize::MetricAttribution wired to Personalize's metric-attribution state, with expression validation at handler level.

Traces: cloudformation__iam__personalize__s3.cfn-personalize-metric-attribution-round-trip-and-delete, cloudformation__iam__personalize__s3.cfn-personalize-metric-attribution-invalid-expression-rejected-leaving-valid-attribution.

## Evidence
Expected: CreateStack accepted; stack CREATE_COMPLETE, or ROLLBACK_COMPLETE for the invalid metric expression.
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Personalize::MetricAttribution]

## Steps to Reproduce
1. Create a Personalize dataset group and an S3 output location with a role.
2. CreateStack with a Resources entry of Type AWS::Personalize::MetricAttribution referencing them.
3. Observe the CreateStack call fail with ValidationError / 'Unrecognized resource types'.
