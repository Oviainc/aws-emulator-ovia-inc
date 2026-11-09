Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__iam__stepfunctions__sts.apigateway-invoke-surfaces-backend-status-as-apigateway-dot-code-error
Issue-Key: apigateway__iam__stepfunctions__sts.apigateway-invoke-surfaces-backend-status-as-apigateway-dot-code-error|run#15|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway__iam__stepfunctions__sts: a Task state using arn:aws:states:::apigateway:invoke is not executed and fails States.Runtime

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: the Step Functions optimized integration for API Gateway is not implemented, so a Task state naming arn:aws:states:::apigateway:invoke is never run.
AWS: a Task state whose Resource is arn:aws:states:::apigateway:invoke calls the named API Gateway stage with the state's parameters and returns the HTTP response. When the API answers a non-2xx status, the task fails with an error named for that status in the ApiGateway error family (an "ApiGateway.<code>" error, e.g. ApiGateway.403) and a Cause carrying the response, so a Catch/Retry on that error name matches.
Emulator: the execution ends FAILED with error 'States.Runtime' and cause "state '<StateName>': 'arn:aws:states:::apigateway:invoke' names an integration with 'apigateway', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it" — the integration call is never made.
Why: no state machine can call an API Gateway stage. Worse, the failure surfaces as States.Runtime, which no Catch/Retry written for ApiGateway.<code> matches, so a caller's error handling silently does not fire and the backend status is never surfaced as an ApiGateway.<code> error.
Fix: the Step Functions Task-state integration dispatcher — add the apigateway:invoke optimized integration and map the API's non-2xx status onto an ApiGateway.<status> task error with the response as Cause.
Traces: apigateway__iam__stepfunctions__sts.apigateway-invoke-surfaces-backend-status-as-apigateway-dot-code-error

## Evidence
"AssertionError: execution ended 'FAILED': 'States.Runtime' \"state 'InvokeOk': 'arn:aws:states:::apigateway:invoke' names an integration with 'apigateway', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it\"". Expected: the task performs the API call, and a non-2xx backend status fails the task with an ApiGateway.<code> error name rather than States.Runtime. The state name is a per-run value.

## Steps to Reproduce
1. Create and deploy a REST API stage.
2. CreateStateMachine with a Task state: Resource 'arn:aws:states:::apigateway:invoke', Parameters {ApiEndpoint, Method, Stage, Path, AuthType}.
3. StartExecution and DescribeExecution.
Expected: the task invokes the stage; a non-2xx answer fails the task with an ApiGateway.<status> error and the response as Cause. Actual: FAILED with States.Runtime and "...is not one of the services this emulator makes a 'Task' state's integration call to".
