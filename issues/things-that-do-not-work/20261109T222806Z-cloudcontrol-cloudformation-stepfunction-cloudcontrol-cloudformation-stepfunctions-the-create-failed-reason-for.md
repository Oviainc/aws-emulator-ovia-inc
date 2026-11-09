Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__cloudformation__stepfunctions.cfn-statemachine-missing-rolearn-rejected-with-positive-control
Issue-Key: cloudcontrol__cloudformation__stepfunctions.cfn-statemachine-missing-rolearn-rejected-with-positive-control|run#13|AssertionError|assert match is not None, (
Issue-Commit: 785433b7ada0bbe790b60a1839b6b0233a99f365

# cloudcontrol__cloudformation__stepfunctions: the CREATE_FAILED reason for an AWS::StepFunctions::StateMachine missing RoleArn is emulator-authored prose, not AWS's model-validation message

> _Note: this issue was identified against the code base at commit `785433b7ada0bbe790b60a1839b6b0233a99f365`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the resource-handler failure reason for a required-property omission is re-worded.

AWS:      a CFN AWS::StepFunctions::StateMachine declared without the required RoleArn fails its resource handler's model validation, the stack ends ROLLBACK_COMPLETE, and the CREATE_FAILED event of that logical resource carries ResourceStatusReason —
          `Resource handler returned message: "Model validation failed (#: required key [RoleArn] not found)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`
          Fixed text: `Resource handler returned message: "Model validation failed (#: required key [` ... `] not found)" (RequestToken: ` ... `, HandlerErrorCode: InvalidRequest)`. Variable parts: the missing member name as the resource type's schema names it (here RoleArn) and the per-request RequestToken (a 36-char UUID the service mints). Recorded behavior only for this text — it is the handler's message relayed verbatim by CloudFormation, not a Smithy-modeled field. No state machine is created (DescribeStateMachine of the declared name answers StateMachineDoesNotExist with `State Machine Does Not Exist: '<arn>'`).

Emulator: the stack does fail and the envelope (`Resource handler returned message: "..." (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`) is right, but the quoted handler message is replaced with the emulator's own sentence: `AWS::StepFunctions::StateMachine requires a RoleArn, which this declaration does not state`. The schema-validation phrasing `Model validation failed (#: required key [RoleArn] not found)` — including the JSON-schema pointer `#:` and the bracketed member name — is absent, so the check cannot even parse the reason and the downstream assertions (no state machine created; the positive-control deploy) are never reached.

Why:      a client or operator that matches on CloudFormation's relayed handler message to classify a failure (a missing required property vs any other InvalidRequest) cannot distinguish them; the member name is no longer carried in the machine-readable `required key [<name>] not found` form, so tooling that extracts the offending property from the reason gets nothing.

Fix:      the AWS::StepFunctions::StateMachine resource handler's required-property validation in the CloudFormation provisioning path must emit AWS's model-validation message text (with the missing member name substituted) as the quoted handler message, leaving the RequestToken/HandlerErrorCode envelope as is.

Traces:   cloudcontrol__cloudformation__stepfunctions.cfn-statemachine-missing-rolearn-rejected-with-positive-control.

## Evidence
Expected ResourceStatusReason on the CREATE_FAILED row of logical resource 'SM' (the RequestToken is read back from the served message):
  'Resource handler returned message: "Model validation failed (#: required key [RoleArn] not found)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)'

Actual:
  'Resource handler returned message: "AWS::StepFunctions::StateMachine requires a RoleArn, which this declaration does not state" (RequestToken: cd2bf0a7-3048-f55b-f1dc-a16dd280ef2d, HandlerErrorCode: InvalidRequest)'

AssertionError: the missing-RoleArn failure reason no longer has the shape this check knows. The RequestToken value is per-run; the quoted handler sentence is the divergence.

## Steps to Reproduce
1. cloudformation CreateStack with Resources.SM = {Type: AWS::StepFunctions::StateMachine, Properties: {StateMachineName: <n>, DefinitionString: '{"StartAt":"S","States":{"S":{"Type":"Pass","End":true}}}'}} — no RoleArn.
2. Poll DescribeStacks until terminal (ROLLBACK_COMPLETE).
3. Page DescribeStackEvents for the CREATE_FAILED row with LogicalResourceId='SM' and read ResourceStatusReason.
4. The quoted handler message is the emulator's own sentence instead of 'Model validation failed (#: required key [RoleArn] not found)'.
