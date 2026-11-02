Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative
Issue-Key: apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative|_http_get_status#50|AssertionError|raise AssertionError(f"GET {url} never resolved within {timeout_s}s: {last_exc}")
Issue-Commit: af1ec33f271a463b262aa787f11efc8d37f33d50

# apigateway__cognito-idp__lambda: a COGNITO_USER_POOLS-protected method answers 500 Internal server error for an unverifiable bearer token instead of 401 Unauthorized (1 cause)

> _Note: this issue was identified against the code base at commit `af1ec33f271a463b262aa787f11efc8d37f33d50`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteApiInvoke

## Description
Cause 1 of 1: the execute-api data plane crashes while evaluating a COGNITO_USER_POOLS authorizer when the identity source header carries a value that is not a verifiable user-pool token.

This is NOT covered by the open issue 20261029T182116Z-apigateway-cognito-idp-lambda-execute-api-invocations-answer.md (its cause 1 says every such invocation answers 500 and carries this check's old replay key as a Timeout at _http_get_status). That shape has changed and the replay key no longer matches: the missing-token leg of this same check now correctly answers 401, so the stage route and the UNAUTHORIZED gateway response do work; only the branch that has to VALIDATE a presented token still faults. The related issue 20261023T170254Z-apigateway-cognito-idp-lambda-five-causes-cognito-authorizer.md also looks similar but does not carry this failure either.

AWS: a GET on a deployed REST API stage whose method has authorizationType=COGNITO_USER_POOLS and an authorizer built on a user pool's provider ARN is refused by API Gateway BEFORE the integration runs whenever the configured identity source (here method.request.header.Authorization) cannot be verified as an ID/access token of one of the authorizer's providerARNs — a missing header, a syntactically invalid string, a token with a bad signature or a token from another pool all take the same path. API Gateway serves the UNAUTHORIZED gateway response: HTTP 401 with the JSON body {"message":"Unauthorized"}. Fixed text of that message is the single word Unauthorized (it is $context.error.messageString of the UNAUTHORIZED gateway response and is only different if the API customizes that gateway response); it has no variable parts — no api id, token, pool id or request id is interpolated into it. The integration is never invoked, so no side effect of the backend is observable. API Gateway never answers 5xx for an unverifiable token: a 500 {"message":"Internal server error"} on that path is a server fault, not a refusal class AWS produces here. Model note: this is the execute-api data plane, which the apigateway Smithy model does not describe (the control-plane model only carries the Authorizer/Method shapes used to configure it: Authorizer.type=COGNITO_USER_POOLS, Authorizer.providerARNs, Authorizer.identitySource, Method.authorizationType/authorizerId, and GatewayResponse type UNAUTHORIZED with statusCode 401) — the refusal above is recorded behavior plus AWS's documented UNAUTHORIZED gateway response.

Emulator: with the authorizer configured exactly as above (COGNITO_USER_POOLS, --provider-arns the pool ARN, --identity-source method.request.header.Authorization, --authorizer-result-ttl-in-seconds 0) and the method wired to an AWS_PROXY Lambda integration on a deployed stage:
  - GET the stage resource with NO Authorization header -> 401 (correct),
  - GET the same URL with Authorization: not-a-real-cognito-token -> HTTP 500 Internal Server Error, repeatedly, for the full 45 s the check retries (the retry loop only short-circuits on 200/401/403), so the request never reaches any terminal non-5xx answer.
The failure is therefore specific to the token-verification branch of the authorizer, not to stage routing.

Why: a client cannot distinguish an authorization refusal from a server fault. Every negative test of a Cognito-protected API — invalid/garbage token, tampered signature, token from the wrong pool — sees 500 instead of 401, so SDK/CLI error handling, retry policy (5xx is retried, 401 is not) and any assertion on the {"message":"Unauthorized"} body are all wrong. It also hides whether the authorizer actually gated the integration, since a 500 says nothing about whether the backend ran.

Fix: the execute-api data-plane request path in the apigateway crate — the COGNITO_USER_POOLS authorizer evaluation step. A token that fails verification (unparseable JWT, bad signature, issuer/pool not among the authorizer's providerARNs, wrong token_use, expired) must be mapped to the API's UNAUTHORIZED gateway response (401, body {"message":"Unauthorized"}) instead of propagating an internal error, and the integration dispatch must be skipped on that path.

Traces: apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative (the only surviving failing check for this cause; the other checks of this group are owned by the issues named above).

## Evidence
Verbatim from the replay of apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative:

  File "check_apigateway__cognito_idp__lambda_cli_apigw_deploy_cognito_authorizer_test_unauthorized_negative.py", line 247, in run
    bad_status = _http_get_status(invoke_url, headers={"Authorization": "not-a-real-cognito-token"})
  AssertionError: GET https://<host>/_aws/execute-api/<api-id>/prod/secure never resolved within 45.0s: HTTP Error 500: Internal Server Error

Expected (AWS): 401, and the check then asserts the Lambda integration did not fire (empty side-channel SQS queue); the assertion it would have run is `assert bad_status == 401, f"expected 401 for a syntactically-invalid bearer token, got {bad_status}"`.
Actual (emulator): HTTP 500 Internal Server Error on every attempt for 45 s.

The preceding leg of the same check — the identical GET with NO Authorization header — returned 401 and passed, which is what localizes the defect to token verification rather than to stage routing or the UNAUTHORIZED gateway response itself.

Requirement being violated: status 401 (not 500) and body {"message":"Unauthorized"} — fixed text, no interpolated values. Host, port, api id, stage and resource name in the URL above are per-run values and are not part of the requirement.

## Steps to Reproduce
1. Create a Cognito user pool with a confirmed user and an app client (no secret).
2. Create a REST API, a resource /secure, and an authorizer: create-authorizer --type COGNITO_USER_POOLS --provider-arns arn:aws:cognito-idp:<region>:<account>:userpool/<poolId> --identity-source method.request.header.Authorization --authorizer-result-ttl-in-seconds 0.
3. put-method GET on /secure with --authorization-type COGNITO_USER_POOLS --authorizer-id <id>; put-integration --type AWS_PROXY --integration-http-method POST --uri arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fnArn>/invocations; add the lambda:InvokeFunction permission for apigateway.amazonaws.com.
4. create-deployment --stage-name prod.
5. GET the stage URL .../prod/secure with NO Authorization header -> 401 (works today).
6. GET the same URL with header `Authorization: not-a-real-cognito-token`.
   Expected: 401 with body {"message":"Unauthorized"} and the Lambda integration not invoked.
   Observed: HTTP 500 Internal Server Error (body {"message":"Internal server error"}), repeatedly.
