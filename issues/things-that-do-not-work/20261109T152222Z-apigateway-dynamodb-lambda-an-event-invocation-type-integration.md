Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-lambda-event-invocation-returns-200-empty-and-runs-async apigateway__dynamodb__lambda.apigw-token-authorizer-identity-validation-expression-rejects-without-invoking
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-event-invocation-returns-200-empty-and-runs-async|_poll#21|TimeoutError|raise TimeoutError(f"{description}: condition not met within {timeout_s:.0f}s; last {last!r}")
Issue-Key: apigateway__dynamodb__lambda.apigw-token-authorizer-identity-validation-expression-rejects-without-invoking|run#26|AssertionError|assert status == 401, (
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway__dynamodb__lambda: an Event-invocation-type integration answers 500, and a token failing identityValidationExpression is admitted (2 causes)

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Two net-new root causes in the execute-api integration and authorizer paths. Numbered; each carries its own failing check ids.

Cause 1 of 2: an AWS integration with the Event invocation type answers 500 instead of 200.
AWS: an API Gateway AWS/AWS_PROXY integration that sets the X-Amz-Invocation-Type: Event request parameter invokes the Lambda asynchronously; API Gateway answers the caller immediately with HTTP 200 and an empty body while the function runs in the background.
Emulator: the stage answers HTTP 500 with the default gateway-response body {"message":"Internal server error"} (content-type application/json) for the whole 45s window — the asynchronous invocation never produces the 200.
Why: the fire-and-forget pattern (API Gateway accepts, function processes later) is unavailable, and the caller is told the API failed when AWS reports success.
Fix: the execute-api integration path's handling of the Event invocation type (async invoke, immediate 200 with an empty body).
Traces: apigateway__dynamodb__lambda.apigw-lambda-event-invocation-returns-200-empty-and-runs-async

Cause 2 of 2: a TOKEN authorizer's identityValidationExpression is not applied, so a non-matching token is admitted.
AWS: when a TOKEN authorizer declares identityValidationExpression, API Gateway tests the incoming identity source against that regular expression BEFORE invoking the authorizer; a token that does not match is refused with HTTP 401 and the authorizer function is never invoked.
Emulator: the request is served 200 with the integration's body ('{"ok": true}') — the expression is ignored and the request admitted.
Why: an authorizer relying on identityValidationExpression as its first-line filter is bypassed; malformed tokens reach the backend instead of being refused 401, so an over-permissive emulator hides a real rejection.
Fix: the execute-api authorizer path's identityValidationExpression evaluation (before the authorizer invocation).
Traces: apigateway__dynamodb__lambda.apigw-token-authorizer-identity-validation-expression-rejects-without-invoking

## Evidence
Cause 1: "TimeoutError: POST /notify through the Event-invocation-type integration returning 200: condition not met within 45s; last (500, {'content-type': 'application/json', ...}, '{\"message\":\"Internal server error\"}')".
Cause 2: "a token failing identityValidationExpression must be refused with 401 before any authorizer invocation, got 200: '{\"ok\": true}'".

## Steps to Reproduce
1. Cause 1: PutIntegration (AWS or AWS_PROXY to a Lambda) with requestParameters {'integration.request.header.X-Amz-Invocation-Type': "'Event'"}, deploy, and POST the stage path. Expect 200 with an empty body and the function to have run.
2. Cause 2: CreateAuthorizer(type='TOKEN', identitySource='method.request.header.Authorization', identityValidationExpression='^Bearer [A-Za-z0-9]+$'), attach it to a method, deploy, and call with an Authorization header that does not match. Expect 401 and no authorizer invocation.
