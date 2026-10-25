Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience apigatewayv2__cognito-idp__lambda.jwt-authorizer-expired-token-rejected-never-reaches-lambda
Issue-Key: apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event|_poll_http#18|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience|_poll_http#18|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__cognito-idp__lambda.jwt-authorizer-expired-token-rejected-never-reaches-lambda|_poll_http#17|Timeout|time.sleep(interval_s)
Issue-Commit: 21bca675a34dab10ee37fe0445fa59a838d6e2c4

# apigatewayv2__cognito-idp__lambda: all three JWT-authorizer checks still hang — a deployed HTTP API's invoke URL never serves a request (1 cause)

> _Note: this issue was identified against the code base at commit `21bca675a34dab10ee37fe0445fa59a838d6e2c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane, JWT-authorized HTTP API route)

## Description
REOPENS 20261024T120951Z-apigatewayv2-cognito-idp-lambda-all-three-jwt-authorizer.md: its fix left the call refused. That resolved issue was grounded in exactly these three check ids, with replay keys bound to `_poll_http#18|Timeout|time.sleep(interval_s)` — the poll of the deployed HTTP API's invoke URL. All three checks fail again in this replay with the same symptom, so the execute-api data plane for a JWT-authorized HTTP API route still does not serve.

Cause 1 of 1: an HTTP API whose route AuthorizationType is JWT and whose authorizer names a Cognito user pool as issuer never answers on its invoke URL, so no token is ever evaluated and no integration is ever dispatched.

AWS: recorded behavior only — this is the execute-api data plane, which has no Smithy operation. apigatewayv2's Smithy model carries the control-plane contract these checks build on (CreateAuthorizer with AuthorizerType=JWT and JWTConfiguration{Issuer, Audience}; CreateRoute with AuthorizationType=JWT and AuthorizerId; CreateStage with AutoDeploy). On real AWS a deployed stage then (a) admits a token whose issuer and client_id match the authorizer's Issuer/Audience, answering with the integration's response and delivering the token's claims to the Lambda event under requestContext.authorizer.jwt.claims plus .scopes, and (b) refuses a token from a foreign issuer, a token whose client_id is outside Audience, and an expired token with HTTP 401 and never invokes the integration (CDK's apigatewayv2-authorizers documentation states 401 for failed JWT validation and 403 only for insufficient scopes).

Emulator: every control-plane call in the estate succeeds — CreateApi(ProtocolType=HTTP), CreateIntegration(AWS_PROXY), CreateAuthorizer(JWT), CreateRoute(AuthorizationType=JWT), CreateStage, plus the Cognito pools/clients and the Lambda reaching State=Active — and then nothing is ever servable on the announced invoke URL. Each check's bounded poll of that URL (accept=200, timeout 150–180 s) never sees an accepted status, so the check runs past the suite's 60 s cap and is reported only as `Check timed out after 60.0s`. The previously recorded frame for all three was inside that poll.

Why: the single most security-sensitive behaviour of an HTTP API is unobservable. A client cannot tell an emulator that never serves from one that authorizes every token it is handed, so a JWT authorizer's issuer check, audience check and exp check can all be silently absent; and the positive leg (claims reaching the backend verbatim) cannot be relied on either, so no application that reads requestContext.authorizer.jwt.claims can be developed against this emulator.

Fix: the execute-api data-plane listener for HTTP APIs and the JWT-authorizer evaluation stage that must run in front of integration dispatch. Related OPEN issues name adjacent pieces of the same absence but do not carry these three checks' replay keys: 20261025T030504Z-apigatewayv2-an-http-api-s-invoke-url-never.md (HTTP invoke URL never serves, route-key precedence / ImportApi checks), 20261024T120951Z-apigatewayv2-the-execute-api-data-plane-never-serves.md, and 20261023T151412Z-apigatewayv2-cognito-idp-lambda-all-four-jwt-authorizer.md (which attributes a JWT-authorizer hang to a CreateAuthorizer NotImplemented stub that no longer reproduces here).

Traces: apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event, apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience, apigatewayv2__cognito-idp__lambda.jwt-authorizer-expired-token-rejected-never-reaches-lambda.

## Evidence
All three checks are reported as `[mismatch] ... Error: Check timed out after 60.0s` with no frame, and the service line reads `apigatewayv2__cognito-idp__lambda 0/3 (0%) 3 failed`. None of the three carried an 'ALREADY FILED' or 'OWNED by' marker in this replay, and the only issue that ever carried their replay keys (`_poll_http#18|Timeout|time.sleep(interval_s)` / `_poll_http#17|...`) is the RESOLVED 20261024T120951Z-apigatewayv2-cognito-idp-lambda-all-three-jwt-authorizer.md.

Expected (AWS): GET <invoke-url>/guarded with a valid pool+client access token -> HTTP 200 carrying the Lambda's body, and the backend's log line appears; GET with a token from a different user pool (valid signature, foreign issuer), with a token from a second app client in the same pool (client_id outside Audience), or with a token past its AccessTokenValidity -> HTTP 401 with no new backend log line; widening the authorizer's JwtConfiguration.Audience to include the second client makes that previously refused token succeed unchanged.

Actual (emulator): the invoke URL is never answered with any accepted status. The checks block in their inlined `_poll_http(url, accept=lambda s: s == 200, timeout_s=180.0/150.0)` helper, which would otherwise raise "<description> (<url>) never reached an accepted status within 180.0s. Last: <status> <body>", and the harness kills them at 60 s instead. Consequently neither the admitted leg (claims/scopes under requestContext.authorizer.jwt) nor any of the three refusal legs is ever reached.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. cognito-idp: CreateUserPool + CreateUserPoolClient (ExplicitAuthFlows ALLOW_ADMIN_USER_PASSWORD_AUTH), AdminCreateUser + AdminSetUserPassword, AdminInitiateAuth to mint a real access token. Create a second app client in the same pool, and a second (foreign) pool+client+user, for the negative legs.
3. lambda: CreateFunction (python3.12) whose handler echoes requestContext.authorizer.jwt and prints a marker; wait for State='Active'.
4. apigatewayv2: CreateApi(ProtocolType='HTTP'); CreateIntegration(IntegrationType='AWS_PROXY', IntegrationUri=<fn arn>, PayloadFormatVersion='2.0'); CreateAuthorizer(AuthorizerType='JWT', IdentitySource=['$request.header.Authorization'], JwtConfiguration={'Issuer': '<cognito endpoint>/<poolId>', 'Audience': ['<clientId>']}); CreateRoute(RouteKey='GET /guarded', AuthorizationType='JWT', AuthorizerId=..., Target='integrations/<id>'); CreateStage(StageName='$default', AutoDeploy=True).
5. GET the announced invoke URL at /guarded with `Authorization: Bearer <valid access token>` — AWS answers 200 with the Lambda body; the emulator never answers an accepted status, so a bounded poll of up to 180 s never settles.
6. Repeat with the foreign-pool token, the second-client token and an expired token — AWS answers 401 and the backend is never invoked; the emulator answers nothing at all.
7. Replay the three check ids in trace_ids.
