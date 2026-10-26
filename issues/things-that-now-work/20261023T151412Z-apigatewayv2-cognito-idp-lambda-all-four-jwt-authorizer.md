Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience apigatewayv2__cognito-idp__lambda.jwt-authorizer-expired-token-rejected-never-reaches-lambda
Issue-Key: apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__cognito-idp__lambda.jwt-authorizer-expired-token-rejected-never-reaches-lambda|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# apigatewayv2__cognito-idp__lambda: all four JWT-authorizer checks hang to the 60s cap because apigatewayv2 CreateAuthorizer answers NotImplemented (1 cause)

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
Cause 1 of 1: apigatewayv2 CreateAuthorizer is an unimplemented scaffold stub, so a JWT authorizer fronting a Cognito user pool can never be created; each of the four JWT checks wraps its CreateAuthorizer call in a retry poll (timeout_s=120.0, interval_s=5.0, 'JWT authorizer creation (the issuer''s OIDC discovery document settling)'), so the stub's refusal is retried rather than raised and the check is killed by the harness at its 60s cap with no further diagnosis.
AWS: Smithy model (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model) CreateAuthorizer -> POST /v2/apis/{ApiId}/authorizers, success code 201, returning AuthorizerId and the authorizer document including JwtConfiguration (Issuer, Audience); modelled errors are BadRequestException (400), ConflictException (409), NotFoundException (404) and TooManyRequestsException (429) — 'NotImplemented' is not among them. A valid AuthorizerType='JWT' CreateAuthorizer must return 201 promptly, after which (recorded behavior) a valid token's claims and scopes reach the integration verbatim under requestContext.authorizer.jwt.claims/.scopes, and a token whose client_id is outside the authorizer's Audience, a token whose scopes do not match the route's AuthorizationScopes, a token from a foreign issuer, and an expired token are each refused with HTTP 401 before the integration is invoked.
Emulator: CreateAuthorizer answers error code 'NotImplemented' ('operation CreateAuthorizer is scaffolded but not implemented in this emulator'), which the checks' poll absorbs, so the observable symptom is a hang: 'Check timed out after 60.0s'. Hypothesis for the exact path: the poll never succeeds because every attempt is the same stub refusal — the same refusal is raised directly in apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid and apigatewayv2__dynamodb__lambda.deep-apigatewayv2-1-wsroute-connect-request-authorizer-deny-refuses-before-integration, which call CreateAuthorizer without a poll.
Why: a client using a Cognito-backed JWT authorizer gets an unmodelled error instead of an authorizer, so no JWT-protected route can exist: neither the admit path (claims/scopes forwarded to the integration) nor any of the four 401 refusal paths can be emulated, and callers that retry a transient-looking failure hang instead of failing fast.
Fix: the CreateAuthorizer handler and authorizer state (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_authorizer.rs), then the data-plane JWT validation (issuer/audience/expiry/scope) and the requestContext.authorizer.jwt event projection.
Note: the open issue 20261023T125319Z-apigatewayv2-createauthorizer-stub.md names the same handler but carries none of the replay keys below.
Traces: apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event, apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered, apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience, apigatewayv2__cognito-idp__lambda.jwt-authorizer-expired-token-rejected-never-reaches-lambda.

## Evidence
Replay of suite simparity-behavioral-aws-v1: apigatewayv2__cognito-idp__lambda 0/4 (0%). All four checks report 'Error: Check timed out after 60.0s' — the only 60s timeouts in the suite besides the two apigatewayv2__cloudformation deploys, so this is not a blanket harness cap effect.

The shared blocking call is visible in the replayed source of jwt-authorizer-claims-reach-the-lambda-event:
  authorizer_id = _poll_until(lambda: _try_create_jwt_authorizer(ctx, api_id, issuer, [pool['client_id']]), lambda r: bool(r), timeout_s=120.0, interval_s=5.0, description='JWT authorizer creation (the issuer''s OIDC discovery document settling)')
with the subsequent assertions never reached (requestContext.authorizer.jwt.claims[sub/token_use/client_id] equal to the decoded token, .scopes equal to the token's space-delimited scope claim, and a 401 'Unauthorized' envelope for an out-of-Audience client_id).

Expected: CreateAuthorizer(AuthorizerType='JWT') returns HTTP 201 with an AuthorizerId. Actual: the same operation answers error code 'NotImplemented' when called without a retry wrapper (see apigatewayv2__cloudwatch__lambda and apigatewayv2__dynamodb__lambda failures in this same replay), so the poll cannot converge.

None of the four checks carried an 'ALREADY FILED' or 'OWNED by' marker.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a Cognito user pool and app client, confirm a user, and obtain an access token.
3. CreateApi(ProtocolType='HTTP'), CreateIntegration(IntegrationType='AWS_PROXY', IntegrationUri=<lambda arn>, PayloadFormatVersion='2.0').
4. Call CreateAuthorizer(ApiId=<api>, Name=..., AuthorizerType='JWT', IdentitySource=['$request.header.Authorization'], JwtConfiguration={'Issuer': '<cognito-idp endpoint>/<pool id>', 'Audience': ['<client id>']}). Expect HTTP 201 with AuthorizerId; observe error code NotImplemented (and, inside a retry loop, an indefinite hang).
5. Re-replay the four check ids in trace_ids; each should then proceed to its invoke-time claims/scopes and 401 assertions.
