Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.cli-workflow-cfn-outputs-drive-apigateway-stage-update
Issue-Key: apigateway__cloudformation__lambda.cli-workflow-cfn-outputs-drive-apigateway-stage-update|_run_cli#14|AssertionError|raise AssertionError(f"CLI command failed: {' '.join(cmd)}\nstderr: {proc.stderr}")
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__cloudformation__lambda: the CLI workflow that drives a stage update from stack outputs fails because apigateway UpdateStage answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: UpdateStage answers 501 NotImplemented, so `aws apigateway update-stage` fails and the CloudFormation-outputs-driven stage-variable update never happens.
AWS: Smithy: UpdateStage takes patchOperations and returns the updated Stage (HTTP 200); its modeled errors are BadRequestException 400, ConflictException 409, LimitExceededException 429, NotFoundException 404, TooManyRequestsException 429 and UnauthorizedException 401. Replacing a stage variable with a value read from a stack output is an ordinary `op=replace,path=/variables/<name>,value=<arn>` patch.
Emulator: the CLI exits non-zero with "An error occurred (NotImplemented) when calling the UpdateStage operation: operation UpdateStage is scaffolded but not implemented in this emulator".
Why: `NotImplemented` is outside UpdateStage's modeled error set, so no client branch matches it, and the common deployment pattern of feeding a CloudFormation output into a stage variable cannot be carried out at all.
Fix: the UpdateStage handler, services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/update_stage.rs. Open issue 20261023T125454Z-apigateway-updatestage-stub.md names that file but does not carry this replay key.
Traces: apigateway__cloudformation__lambda.cli-workflow-cfn-outputs-drive-apigateway-stage-update.

## Evidence
AssertionError: CLI command failed: aws apigateway update-stage --rest-api-id <id> --stage-name prod --patch-operations op=replace,path=/variables/lambdaArn,value=arn:aws:lambda:us-west-2:...:function:... --region us-west-2 / stderr: aws: [ERROR]: An error occurred (NotImplemented) when calling the UpdateStage operation: operation UpdateStage is scaffolded but not implemented in this emulator. Expected: HTTP 200 with the updated Stage carrying the new variable.

## Steps to Reproduce
1. Deploy a CloudFormation stack that outputs a Lambda function ARN and creates a REST API with a stage.
2. Run `aws apigateway update-stage --rest-api-id <id> --stage-name prod --patch-operations op=replace,path=/variables/lambdaArn,value=<output arn>`.
3. Observe the 501 NotImplemented. Or replay apigateway__cloudformation__lambda.cli-workflow-cfn-outputs-drive-apigateway-stage-update.
