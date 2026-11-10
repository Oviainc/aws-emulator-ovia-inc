Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation__lambda.deploy-cfn-httpapi-lambda-integration-missing-permission-invoke-rejected
Issue-Key: apigatewayv2__cloudformation__lambda.deploy-cfn-httpapi-lambda-integration-missing-permission-invoke-rejected|run#30|AssertionError|assert denied_status == 500, (
Issue-Commit: 0c065617979d1ee76fa77b423bd58183fc04c861

# apigatewayv2: an HTTP API AWS_PROXY route invokes its Lambda with no lambda:InvokeFunction permission and answers 200 instead of 500

> _Note: this issue was identified against the code base at commit `0c065617979d1ee76fa77b423bd58183fc04c861`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause.

Cause 1 of 1: the API Gateway v2 (HTTP API) data plane does not consult the integration target's Lambda resource-based policy before invoking it.

AWS: recorded behavior only for the data plane (the execute-api invoke is not a modeled operation). The recording shows that a route whose AWS_PROXY integration targets a Lambda function for which CloudFormation created no AWS::Lambda::Permission is answered with HTTP 500 — API Gateway cannot assume permission to invoke the function, so the integration fails and the caller gets an Internal Server Error, never the function's own payload. The positive control in the same check (the route that does have a permission) returns the function's response, so only the unpermitted route must differ.

Emulator: the unpermitted route returns HTTP 200 with the function's own body (`{"marker": null}`). The function was invoked even though no resource policy grants `lambda:InvokeFunction` to `apigateway.amazonaws.com` for that API.

Why: the single most common API Gateway misconfiguration — a missing Lambda::Permission — is silently tolerated. A client validating its template against the emulator sees a working API and ships a stack that is broken (500 on every request) in real AWS. Worse, the emulator's data plane invokes a function no policy authorises, so any test that relies on the permission boundary to prove isolation proves nothing.

Fix: the API Gateway v2 data-plane integration dispatcher for AWS_PROXY integrations — before invoking the target, it must evaluate the target function's resource-based policy and, when no statement permits the invoke for this API, fail the integration and serve HTTP 500 instead of proxying the function's response.

Traces: apigatewayv2__cloudformation__lambda.deploy-cfn-httpapi-lambda-integration-missing-permission-invoke-rejected.

## Evidence
Expected: GET on a route whose AWS_PROXY integration target has no AWS::Lambda::Permission returns HTTP 500 (the function is not invoked).

Actual (suite report, check apigatewayv2__cloudformation__lambda.deploy-cfn-httpapi-lambda-integration-missing-permission-invoke-rejected, verdict mismatch):
```
AssertionError: GET /denied through a route with no Lambda::Permission returned status 200, expected 500: b'{"marker": null}'
```
The status code (200 vs 500) and the presence of the function's body are the divergence; the body text itself is the fixture's handler payload, not a requirement.

## Steps to Reproduce
1. CreateStack with an AWS::ApiGatewayV2::Api (ProtocolType HTTP), two AWS::Lambda::Function targets, two AWS::ApiGatewayV2::Integration (AWS_PROXY) + Route pairs, and an AWS::Lambda::Permission for only ONE of the two functions.
2. GET the route backed by the function that HAS a permission — it returns 200 with the function payload (positive control).
3. GET the route backed by the function with NO permission — the emulator returns 200 with the function payload; AWS returns 500.
