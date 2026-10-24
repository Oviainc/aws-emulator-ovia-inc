Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event apigatewayv2__cognito-idp__lambda.jwt-authorizer-expired-token-rejected-never-reaches-lambda
Issue-Key: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience|_poll_http#18|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event|_poll_http#18|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__cognito-idp__lambda.jwt-authorizer-expired-token-rejected-never-reaches-lambda|_poll_http#17|Timeout|time.sleep(interval_s)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# apigatewayv2__cognito-idp__lambda: all three JWT-authorizer checks hang — a deployed HTTP API stage never serves an invoke (1 cause)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane) with a JWT authorizer

## Description
Cause 1 of 1: an HTTP API with a JWT authorizer backed by a Cognito user pool never answers on its invoke URL, so no JWT is ever evaluated.
AWS: recorded behavior only (data plane). On a deployed HTTP API route whose AuthorizationType is JWT, API Gateway validates the presented token against the authorizer's issuer and audience before the integration runs: a token from a foreign issuer or with a foreign audience is refused 401/403 and the backend Lambda is never invoked, an expired token is refused the same way, and an accepted token's claims are delivered to the Lambda event under requestContext.authorizer.jwt.claims.
Emulator: each check deploys the API and then polls its invoke URL; nothing servable ever comes back, so every check runs past the suite's 60 s cap and is reported as `Check timed out after 60.0s`. Neither the positive leg (claims reach the Lambda) nor the negative legs (foreign issuer/audience, expired token) can be observed.
Why: a client cannot verify that its JWT authorizer actually gates traffic — the most security-sensitive behaviour of an HTTP API is unobservable, and an emulator that never serves looks identical to one that authorises everything.
Fix: the execute-api data-plane listener for HTTP APIs and the JWT-authorizer evaluation stage in front of integration dispatch (the same root data-plane absence reported for service apigatewayv2 in this sweep). Related open issue 20261023T151412Z-apigatewayv2-cognito-idp-lambda-all-four-jwt-authorizer.md attributes a JWT-authorizer hang to a CreateAuthorizer NotImplemented stub; these three checks are not carried by it.
Traces: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience, apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event, apigatewayv2__cognito-idp__lambda.jwt-authorizer-expired-token-rejected-never-reaches-lambda.

## Evidence
Expected: the deployed stage answers 200 with the Lambda's event (carrying requestContext.authorizer.jwt.claims) for a valid pool token, and 401/403 without invoking the Lambda for a foreign-issuer, foreign-audience or expired token. Actual: `Error: Check timed out after 60.0s` for all three checks — the invoke URL never served a request.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a Cognito user pool + client, CreateApi(ProtocolType=HTTP), CreateAuthorizer(AuthorizerType=JWT) with that issuer/audience, CreateIntegration(AWS_PROXY to a Lambda), CreateRoute(AuthorizationType=JWT), CreateStage(AutoDeploy=true).
3. GET the announced invoke URL with a valid, a foreign-issuer and an expired bearer token — none of them is ever answered.
4. Replay the three check ids in trace_ids.
