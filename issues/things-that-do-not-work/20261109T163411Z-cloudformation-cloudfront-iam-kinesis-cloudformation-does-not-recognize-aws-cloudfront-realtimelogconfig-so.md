Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudfront__iam__kinesis.deploy-cloudfront-realtimelogconfig-round-trip-and-delete cloudformation__cloudfront__iam__kinesis.deploy-cloudfront-realtimelogconfig-sampling-rate-over-maximum-rejected
Issue-Key: cloudformation__cloudfront__iam__kinesis|CreateStack|ValidationError
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# CloudFormation does not recognize AWS::CloudFront::RealtimeLogConfig, so every stack declaring one is refused at CreateStack

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation resource-type registry has no AWS::CloudFront::RealtimeLogConfig, so a template declaring one never reaches a provisioner.

AWS: CreateStack accepts a template whose Resources declare AWS::CloudFront::RealtimeLogConfig (Name, SamplingRate, EndPoints naming a Kinesis stream and a role) and provisions the real-time log config; the stack reaches CREATE_COMPLETE, GetAtt/DescribeStackResource read the config back, and DeleteStack removes it. A SamplingRate above the documented maximum is refused by the resource handler (resource CREATE_FAILED, stack ROLLBACK_COMPLETE), not by template parsing. Smithy: CreateStack.TemplateBody is an opaque string, so the set of registered resource types is a service fact and not modelled — recorded behavior only; the recording shows the round-trip stack completing and the over-maximum stack rolling back.

Emulator: CreateStack answers 400 with error code `ValidationError` and the message `Template format error: Unrecognized resource types: [AWS::CloudFront::RealtimeLogConfig]`. Fixed text: `Template format error: Unrecognized resource types: [` + `]`; the variable part is the list of type names taken from the submitted template.

Why: a client that provisions CloudFront real-time logging through CloudFormation (the only supported path in most IaC stacks) cannot create the stack at all. The failure is also mis-attributed: the client sees a template-syntax error rather than a resource-handler outcome, so the negative-path contract (rollback on an invalid SamplingRate) is unobservable too.

Fix: the CloudFormation resource-type registry plus a create/read/delete provisioner for AWS::CloudFront::RealtimeLogConfig wired to the CloudFront real-time-log-config state.

Traces: cloudformation__cloudfront__iam__kinesis.deploy-cloudfront-realtimelogconfig-round-trip-and-delete, cloudformation__cloudfront__iam__kinesis.deploy-cloudfront-realtimelogconfig-sampling-rate-over-maximum-rejected.

## Evidence
Expected: CreateStack accepts the template; stack reaches CREATE_COMPLETE (round-trip check) or ROLLBACK_COMPLETE after a resource-level refusal of the over-maximum SamplingRate (negative check).
Actual: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CloudFront::RealtimeLogConfig]

## Steps to Reproduce
1. Create a Kinesis stream and a role CloudFront can assume to write to it.
2. Call CreateStack with a template containing a Resources entry of Type AWS::CloudFront::RealtimeLogConfig (Properties: Name, SamplingRate, EndPoints[0].KinesisStreamConfig.{StreamArn,RoleArn}).
3. Observe the CreateStack call itself fail with ValidationError / 'Unrecognized resource types'.
