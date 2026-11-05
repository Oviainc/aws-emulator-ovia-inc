Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-missing-token-not-delivered apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-missing-token-not-delivered|_stays_absent#20|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative|_http_get_status#54|Timeout|time.sleep(2)
Issue-Commit: 2730cd672dd608c80462ed6cac2962eb48b48789

# apigateway__cognito-idp__lambda: a deployed REST API stage fronted by a COGNITO_USER_POOLS authorizer never returns a decidable status, so both Cognito-authorizer admission checks exhaust their budget

> _Note: this issue was identified against the code base at commit `2730cd672dd608c80462ed6cac2962eb48b48789`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api deployed REST API stage)

## Description
Cause 1 of 1: a GET against a deployed REST API stage whose method uses a COGNITO_USER_POOLS authorizer never resolves to any HTTP status, so the checks that grade the authorizer's admission decision never reach a verdict.

AWS: recorded behavior only for the execute-api data plane (it is not an operation in the apigateway Smithy model; the control plane used to build the fixture is modelled — CreateAuthorizer with type=COGNITO_USER_POOLS and providerARNs naming the user pool, PutMethod with authorizationType=COGNITO_USER_POOLS, CreateDeployment). AWS's documented behavior on the stage URL is: a request carrying no Authorization header on a COGNITO_USER_POOLS method is refused by API Gateway itself with HTTP 401 and the body {"message":"Unauthorized"} — fixed text, no variable part; a request carrying a string that is not a verifiable pool token is likewise refused 401; and the integration behind the method is NOT invoked for either. The identical request carrying a live IdToken minted by that pool is served 200 with the integration's own response body. The verdict is always one of 401/403/2xx, delivered promptly.

Emulator: neither check ever observes any of those statuses. Both poll the stage URL for a status in (200, 401, 403) and the harness kills them at the 60 s budget with "Error: Check timed out after 60.0s" — the deployed stage URL answers nothing the poller accepts, so neither the refusal nor the control admission is ever produced. No status code, no error code and no body are available to compare, because the request never completes.

Why: a client relying on AWS's behavior cannot test or operate a Cognito-protected REST API on this emulator at all. The over-permissiveness this scenario exists to catch — an unauthenticated or bogus-token request reaching the backend Lambda — is undetectable, because the authorizer's decision is never expressed; and the positive leg (a valid IdToken reaching the integration) never succeeds either, so a caller cannot distinguish "denied" from "not served".

Fix: the execute-api data-plane request path for deployed REST API stages in the apigateway service — the stage/deployment route resolution and the authorizer evaluation step that must answer 401 with API Gateway's Unauthorized envelope before any integration dispatch, plus the integration dispatch itself for the admitted case.

Traces: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-missing-token-not-delivered; apigateway__cognito-idp__lambda.cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative. Several sibling checks in this same service are already carried by open issues (e.g. 20261029T182116Z-apigateway-cognito-idp-lambda-execute-api-invocations-answer.md and 20261023T170254Z-apigateway-cognito-idp-lambda-five-causes-cognito-authorizer.md); neither of those carries the two replay keys above, so they are filed here.

## Evidence
Expected (AWS): GET <stage-url>/secure with no Authorization header -> HTTP 401, body {"message":"Unauthorized"} (the word 'unauthorized' present, case-insensitive), backend never invoked. GET with Authorization: 'Bearer not-a-real-jwt.at.all' -> HTTP 401. GET with Authorization: <live IdToken from the pool> -> HTTP 200, body carrying the backend's marker, and a BACKEND-INVOKED line in the function's log group.

Actual (emulator): the poll for any status in (200, 401, 403) never succeeds; the check is terminated by the harness.
  apigw-cognito-authorizer-missing-token-not-delivered -> 'Error: Check timed out after 60.0s'
  cli-apigw-deploy-cognito-authorizer-test-unauthorized-negative -> 'Error: Check timed out after 60.0s'
No HTTP status, error code or body was returned to compare against the expectations above.

## Steps to Reproduce
1. Create a Cognito user pool, an app client with ALLOW_ADMIN_USER_PASSWORD_AUTH, and one confirmed user; obtain that user's IdToken via AdminInitiateAuth.
2. Create a REST API with a /secure resource and a GET method whose authorizationType is COGNITO_USER_POOLS, bound to an authorizer created with type=COGNITO_USER_POOLS and providerARNs=[<pool ARN>].
3. Wire the method to an AWS_PROXY integration on a Lambda function that logs a unique BACKEND-INVOKED marker, and grant apigateway.amazonaws.com lambda:InvokeFunction on arn:aws:execute-api:<region>:<account>:<api-id>/*/*.
4. CreateDeployment into stage 'test'.
5. GET https://<api-id>.execute-api.<region>.<host>/test/secure with no Authorization header. Expect 401 with the Unauthorized envelope; observe that the request never yields an accepted status.
6. Repeat with Authorization: 'Bearer not-a-real-jwt.at.all' (expect 401) and with Authorization: <IdToken> (expect 200 plus the backend marker).
