Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__sqs.obs-cloudformation-2-rollback-trigger-invalid-type-rejected
Issue-Key: cloudformation__cloudwatch__sqs.obs-cloudformation-2-rollback-trigger-invalid-type-rejected|_client_error#12|AssertionError|raise AssertionError("expected a ClientError, the call succeeded instead")
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# CreateStack accepts a RollbackTrigger with an unsupported Type instead of refusing it with ValidationError 'Rollback Trigger Type not supported'

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: `RollbackConfiguration.RollbackTriggers[].Type` is accepted as an opaque string.
AWS: CloudFormation declares only two RollbackTrigger types; `AWS::CloudWatch::Alarm` and `AWS::CloudWatch::CompositeAlarm`. A CreateStack naming any other type — here `AWS::CloudWatch::Metric` on an otherwise valid template and a real alarm ARN — is refused SYNCHRONOUSLY with error code `ValidationError` and the fixed message `Rollback Trigger Type not supported` (no variable part), and nothing is created: a subsequent DescribeStacks for that stack name reports it as absent rather than any CREATE_*/ROLLBACK_* state.
Emulator: the CreateStack call is accepted (no ClientError at all), so the stack is created with an unusable rollback trigger; the refusal never happens and the "nothing was created" invariant is violated.
Why: an IaC pipeline that mistypes a rollback-trigger type gets a green create against the emulator and a hard synchronous ValidationError against the real account; worse, the stack it creates in the emulator has rollback monitoring that can never fire. The positive control in the same check — the identical template with Type `AWS::CloudWatch::Alarm` — reaches CREATE_COMPLETE, proving the refusal is about the Type value alone.
Fix: CreateStack's (and UpdateStack's) validation of RollbackConfiguration.RollbackTriggers, in the CloudFormation create-stack handler, before any resource provisioning begins.
Traces: cloudformation__cloudwatch__sqs.obs-cloudformation-2-rollback-trigger-invalid-type-rejected.

## Evidence
Replay output for cloudformation__cloudwatch__sqs.obs-cloudformation-2-rollback-trigger-invalid-type-rejected:

  AssertionError: expected a ClientError, the call succeeded instead

The check calls CreateStack with `rollback_triggers=[{"Arn": <alarm arn>, "Type": "AWS::CloudWatch::Metric"}]` and asserts:
  code == "ValidationError"
  message == "Rollback Trigger Type not supported"
  DescribeStacks for the stack name reports it absent ('GONE')
Actual: CreateStack returned success, so no code/message existed to compare.

## Steps to Reproduce
Replay cloudformation__cloudwatch__sqs.obs-cloudformation-2-rollback-trigger-invalid-type-rejected. It creates a CloudWatch metric alarm, reads its AlarmArn, then calls CreateStack on a one-resource (AWS::SQS::Queue) template with RollbackConfiguration.RollbackTriggers[0].Type = 'AWS::CloudWatch::Metric' and expects a synchronous ValidationError; a control create with Type 'AWS::CloudWatch::Alarm' must succeed.
