Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__stepfunctions.invoke-resource-policy-denial-fails-with-not-authorized-cause
Issue-Key: apigateway__stepfunctions.invoke-resource-policy-denial-fails-with-not-authorized-cause|run#15|AssertionError|assert desc["error"] == "ApiGateway.AccessDeniedException", f"the task's error was {desc['error']!r}"
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway__stepfunctions: a Task state using arn:aws:states:::apigateway:invoke is not executed, so a resource-policy denial surfaces as States.Runtime

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: the Step Functions optimized integration for API Gateway is not implemented, so a Task state naming arn:aws:states:::apigateway:invoke never performs the call and the API's own refusal is never surfaced.
AWS: a Task state whose Resource is arn:aws:states:::apigateway:invoke calls the named API Gateway stage. When the API's resource policy denies the call, the stage answers 403 and the task fails with an ApiGateway error named for that status (an "ApiGateway.403"-family error) whose Cause carries the API's not-authorized response, so a Catch on that error name matches and the cause names the denial.
Emulator: the task's error is 'States.Runtime' — the integration call is never made, so no API Gateway status or not-authorized cause is produced at all.
Why: a caller's Catch/Retry written for the ApiGateway error family never matches, and the resource-policy denial that the state machine is supposed to observe is replaced by a generic runtime failure, so the denial is indistinguishable from an interpreter error.
Fix: the Step Functions Task-state integration dispatcher — add the apigateway:invoke optimized integration and map the API's refusal status onto an ApiGateway.<status> task error carrying the response as Cause.
Traces: apigateway__stepfunctions.invoke-resource-policy-denial-fails-with-not-authorized-cause

## Evidence
"AssertionError: the task's error was 'States.Runtime'". Expected: the task fails with an ApiGateway.<status> error whose Cause carries the API Gateway not-authorized response produced by the resource policy.

## Steps to Reproduce
1. Create and deploy a REST API stage and attach a resource policy that denies the caller.
2. CreateStateMachine with a Task state: Resource 'arn:aws:states:::apigateway:invoke' pointing at that stage, with a Catch on the ApiGateway error family.
3. StartExecution, then DescribeExecution / GetExecutionHistory.
Expected: the task fails with an ApiGateway.<status> error and a Cause naming the resource-policy denial. Actual: error 'States.Runtime'.
