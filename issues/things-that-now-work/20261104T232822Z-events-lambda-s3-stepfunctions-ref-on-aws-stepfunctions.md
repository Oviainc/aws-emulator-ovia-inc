Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.cfn-deploy-sfn-task-token-lambda-callback-stack
Issue-Key: sfn|StartExecution|InvalidArn
Issue-Commit: 6ff397d47b7f23b010f8590f84afedb6b986e178

# events__lambda__s3__stepfunctions: Ref on AWS::StepFunctions::StateMachine yields the state-machine name, so StartExecution on the stack output is refused InvalidArn

> _Note: this issue was identified against the code base at commit `6ff397d47b7f23b010f8590f84afedb6b986e178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: the physical resource id of an AWS::StepFunctions::StateMachine (what Ref and the stack output resolve to) is the generated state-machine NAME instead of its ARN.
AWS: Smithy: states#StartExecution requires `stateMachineArn` (target Arn, @length 1..256) and answers InvalidArn only for a value that is not a Step Functions ARN. Ref on an AWS::StepFunctions::StateMachine returns the state machine ARN `arn:aws:states:<region>:<account>:stateMachine:<name>` (recorded behavior for the CFN resource), so a stack that exports it can hand it straight to StartExecution and get back executionArn/startDate.
Emulator: the stack output is the bare name (shaped `<stack-name>-CallbackStateMachine-<suffix>`) and StartExecution is refused 400 InvalidArn with message "Invalid Arn: 'Invalid ARN prefix: <value>'". The name in the message is a per-run generated value; the defect is that the resolved reference is a name rather than an ARN.
Why: every consumer of a CloudFormation-provisioned state machine — StartExecution from a client, an EventBridge target, a Lambda that starts the workflow — receives an identifier AWS would never return and is refused. The task-token/callback deploy pattern cannot be exercised at all.
Fix: the CloudFormation stack provisioner's AWS::StepFunctions::StateMachine resource handler — its physical resource id (and therefore Ref and any Output that uses it) must be the stateMachineArn returned by CreateStateMachine.
Traces: events__lambda__s3__stepfunctions.cfn-deploy-sfn-task-token-lambda-callback-stack.

## Evidence
Expected: StartExecution(stateMachineArn=<stack output>) returns 200 with executionArn and startDate, because the output is `arn:aws:states:<region>:<account>:stateMachine:<name>`.
Actual: botocore.errorfactory.InvalidArn: An error occurred (InvalidArn) when calling the StartExecution operation: Invalid Arn: 'Invalid ARN prefix: <stack-name>-CallbackStateMachine-<suffix>' — the reference resolved to the state machine's name.

## Steps to Reproduce
Replay events__lambda__s3__stepfunctions.cfn-deploy-sfn-task-token-lambda-callback-stack: CreateStack for the task-token callback template (state machine + callback Lambda + bucket), wait for CREATE_COMPLETE, read the state-machine reference the stack exposes, and call states StartExecution with it.
