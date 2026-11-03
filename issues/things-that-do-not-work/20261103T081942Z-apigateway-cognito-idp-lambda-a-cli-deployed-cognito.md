Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative
Issue-Key: apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative|_http_get_status#50|Timeout|raise AssertionError(f"GET {url} never resolved within {timeout_s}s: {last_exc}")
Issue-Commit: 34e92f1303a639c9cc6fe5d3111ec2463d97f8c0

# apigateway__cognito-idp__lambda: a CLI-deployed COGNITO_USER_POOLS method never answers on the execute-api data plane, so the unauthorized-negative check hangs to the 60 s cap

> _Note: this issue was identified against the code base at commit `34e92f1303a639c9cc6fe5d3111ec2463d97f8c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
execute-api:Invoke (GET on a deployed REST API stage guarded by a COGNITO_USER_POOLS authorizer)

## Description
Cause 1 of 1: an execute-api GET against a deployed REST API stage whose GET method has authorizationType=COGNITO_USER_POOLS (identitySource method.request.header.Authorization, authorizerResultTtlInSeconds=0, AWS_PROXY integration onto a Lambda) never produces an HTTP response at all. The whole check is consumed waiting on that one request and is killed by the 60 s suite cap before it can make any assertion.

AWS: a REST API method whose authorizationType is COGNITO_USER_POOLS refuses the request at the edge, before the integration, whenever the configured identity source is absent or does not carry a token the named user pool can verify. The refusal is HTTP 401 with the JSON body {"message":"Unauthorized"} (fixed text; this error class has no request- or state-derived variable part, and AWS adds an x-amzn-ErrorType: UnauthorizedException header). Smithy: apigateway AuthorizerType enum includes COGNITO_USER_POOLS ("for using an Amazon Cognito user pool"); apigateway UnauthorizedException is modelled errorKind=client with httpError 401 and the documentation "The request is denied because the caller has insufficient permissions." The integration must not be invoked on that leg, and the same URL with a valid pool ID token must answer the integration's 200. Recorded behavior for the positive leg: 200 with the proxy integration's body and exactly one side-channel SQS message.

Emulator: no response is returned on any leg. The check's _http_get_status() retries URLError/5xx for up to 45 s and only returns immediately on 200/401/403; it never got one of those, and the check was cut off with "Check timed out after 60.0s". Every control-plane step before it succeeded (create-rest-api / create-resource / create-authorizer / put-method / put-integration / lambda:AddPermission / create-deployment all returned), so the divergence is in serving the deployed stage, not in configuring it.

Why: a client cannot observe the authorizer at all. Negative authorization is unverifiable (no 401 is ever produced, so an unauthenticated caller is neither admitted nor refused — the request simply never completes), and the positive path is equally unusable because the stage URL answers nothing. Any SDK/CLI/HTTP client hangs until its own socket timeout instead of receiving a modelled 401/200, which is indistinguishable from a dead endpoint.

Fix: the execute-api data-plane request path for deployed REST API stages — stage/route resolution for a CLI-created deployment, and the COGNITO_USER_POOLS authorizer evaluation that must short-circuit to 401 {"message":"Unauthorized"} before the AWS_PROXY integration is dispatched. The response must be emitted synchronously on every leg (absent header, syntactically invalid bearer token, valid pool ID token), never left unanswered.

Related but not covering this: open issue 20261023T170254Z-apigateway-cognito-idp-lambda-five-causes-cognito-authorizer.md also reports Cognito-authorizer traffic hanging to the 60 s cap, and 20261029T182116Z-apigateway-cognito-idp-lambda-execute-api-invocations-answer.md reports execute-api answering 500 or never answering. Neither carries this check's replay key, so this check is filed on its own; if a fixer closes those, this one still has to be re-replayed.

Traces: apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative (verdict: mismatch; detail: "Check timed out after 60.0s").

## Evidence
Expected (AWS): GET https://<api-id>.execute-api.<region>.amazonaws.com/prod/secure with no Authorization header -> HTTP 401, body {"message":"Unauthorized"}, integration not invoked (no side-channel SQS message). Same URL with Authorization: not-a-real-cognito-token -> HTTP 401, body {"message":"Unauthorized"}, integration not invoked. Same URL with a valid Cognito ID token -> HTTP 200 and exactly one side-channel SQS message body "invoked".

Actual (emulator): no HTTP status is ever returned for the first leg. The check's GET helper retries for up to 45 s and the check is terminated by the harness: "Error: Check timed out after 60.0s". No assertion in the check ever executes, so neither the 401 nor the 200 leg is reached. All preceding apigateway/lambda/cognito-idp control-plane calls returned normally.

## Steps to Reproduce
1. Create an SQS queue and a python3.12 Lambda whose handler sends one message to it and returns {"statusCode": 200}.
2. Create a Cognito user pool + app client and obtain a valid ID token for a confirmed user.
3. With the AWS CLI: apigateway create-rest-api; create-resource (path part "secure"); create-authorizer --type COGNITO_USER_POOLS --provider-arns <pool-arn> --identity-source method.request.header.Authorization --authorizer-result-ttl-in-seconds 0; put-method --authorization-type COGNITO_USER_POOLS --authorizer-id <id>; put-integration --type AWS_PROXY --integration-http-method POST --uri arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn-arn>/invocations; lambda add-permission for apigateway.amazonaws.com; create-deployment --stage-name prod.
4. GET <stage-url>/secure with no Authorization header.
Observed: the request never completes (no status code); the check is killed at the 60 s cap. Expected: HTTP 401 with body {"message":"Unauthorized"} and no Lambda invocation, then 401 again for a bogus Authorization header, then 200 plus one SQS message for a valid ID token.
