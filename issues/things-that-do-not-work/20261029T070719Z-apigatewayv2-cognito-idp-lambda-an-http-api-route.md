Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience
Issue-Key: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience|_poll_http#18|TimeoutError|raise TimeoutError(f"{description} ({url}): the run budget ended first; not graded")
Issue-Commit: cc767c9c860d2029dd9667f4e637f1644e38e746

# apigatewayv2__cognito-idp__lambda: an HTTP API route behind a JWT authorizer never answers, so foreign-issuer and wrong-audience tokens are never rejected

> _Note: this issue was identified against the code base at commit `cc767c9c860d2029dd9667f4e637f1644e38e746`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
One net-new root cause.

Cause 1 of 1: a JWT-authorizer-protected HTTP API route produces no response at all, for either the accepted or the rejected token.

AWS:      Smithy: CreateAuthorizer with AuthorizerType=JWT takes JwtConfiguration{Issuer, Audience} and IdentitySource. API Gateway validates the presented bearer token against exactly that issuer and audience: a token minted by the CONFIGURED Cognito user pool and app client is accepted and the request reaches the integration, which answers 200 with the authorizer's JWT claims present under requestContext.authorizer.jwt.claims; a token from a DIFFERENT user pool (foreign issuer) or for a different app client (audience/client_id not in Audience) is refused by the gateway with HTTP 401 and the integration is never invoked. recorded behavior only for the statuses: the 200 on the configured-pool token and the 401 on the foreign-issuer / wrong-audience token are the recorded data-plane responses. The issuer URL, pool id, client id and api id are values from the request and state; the statuses and the claim-passthrough are the requirement.
Emulator: the guarded route never answers. The probe for the positive leg exhausts the run budget with no status: TimeoutError: "GET /guarded with a token from the configured pool and client (https://<host>/_aws/execute-api/<apiId>/$default/guarded): the run budget ended first; not graded". Because the accepted leg never settles, the foreign-issuer and wrong-audience rejection legs are never reached.
Why:      A client cannot authenticate at all through a JWT authorizer — it gets a hang instead of 200 or 401 — and, worse, the emulator never demonstrates that a token from an unrelated user pool or app client IS refused. Anything validating a JWT authorization boundary against this emulator gets no signal, so an authorization regression is indistinguishable from the endpoint being down.
Fix:      The execute-api HTTP data-plane JWT-authorizer pipeline: resolve the route's JWT authorizer, validate the token's issuer and audience against JwtConfiguration, refuse with 401 on mismatch, and on success dispatch to the integration with the decoded claims in requestContext.authorizer.jwt.claims.
Traces:   apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience. The open issue 20261025T084658Z-apigatewayv2-cognito-idp-lambda-all-three-jwt-authorizer--r2.md covers other JWT-authorizer checks but does not carry this check's replay key, so it does not cover this cause; close them together if one fix serves both.

## Evidence
Expected: GET /guarded with a bearer token minted by the configured Cognito user pool and app client returns 200 from the integration, with the JWT claims visible to the handler under requestContext.authorizer.jwt.claims; the same GET with a token from a foreign user pool, or with an audience/client_id outside JwtConfiguration.Audience, returns 401 and never invokes the integration. Actual: no status is ever returned for the positive leg — `TimeoutError: GET /guarded with a token from the configured pool and client (https://127.0.0.1:<port>/_aws/execute-api/<apiId>/$default/guarded): the run budget ended first; not graded` (check_apigatewayv2__cognito_idp__lambda_http_api_jwt_authorizer_rejects_foreign_issuer_and_audience.py line 106, via _poll_http at line 464). Host, port and api id are per-run values.

## Steps to Reproduce
Create two Cognito user pools, each with an app client. Create an HTTP API with a Lambda proxy integration on GET /guarded and a JWT authorizer whose JwtConfiguration.Issuer/Audience name pool A and its client. Deploy the $default stage. GET /guarded with (a) a token from pool A's configured client — expect 200 and claims in the event, (b) a token from pool B — expect 401, (c) a token from a different client of pool A — expect 401. Replay check apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience.
