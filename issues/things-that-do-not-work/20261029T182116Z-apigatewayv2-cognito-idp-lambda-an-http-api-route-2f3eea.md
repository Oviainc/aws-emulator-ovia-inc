Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered
Issue-Key: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered|_poll_http#17|Timeout|time.sleep(interval_s)
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# apigatewayv2__cognito-idp__lambda: an HTTP API route with a JWT authorizer never answers, so a scope-mismatch request is neither delivered nor refused

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRoute

## Description
Cause 1 of 1 — the HTTP API data plane does not answer.
AWS: a $default-stage route protected by a JWT authorizer whose AuthorizationScopes are not satisfied by the presented access token answers 403 with {"message":"Forbidden"} and the integration is never invoked; a satisfying token gets the integration's 200.
Emulator: the request to the stage URL never returns within the check's budget — the check ends with 'Check timed out after 60.0s', so neither the refusal nor the delivery can be observed.
Why: scope enforcement on HTTP APIs is unobservable, and a client hangs instead of receiving a modeled 403.
Fix: the apigatewayv2 execute-api request path (JWT authorizer evaluation + integration dispatch).
Traces: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered. The open issues 20261029T070719Z-apigatewayv2-cognito-idp-lambda-an-http-api-route.md and 20261025T084658Z-apigatewayv2-cognito-idp-lambda-all-three-jwt-authorizer--r2.md are related but do not carry this replay key.

## Evidence
apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered -> 'Error: Check timed out after 60.0s'. The sibling check in the same service combination shows the endpoint shape being polled: 'GET /guarded with a token from the configured pool and client (https://127.0.0.1:33245/_aws/execute-api/<api-id>/$default/guarded)'. Expected: 403 {"message":"Forbidden"} for a token missing the route's AuthorizationScopes; actual: no response at all.

## Steps to Reproduce
1. CreateApi(ProtocolType='HTTP') + CreateAuthorizer(AuthorizerType='JWT', IdentitySource=['$request.header.Authorization'], JwtConfiguration={Issuer:<pool issuer>, Audience:[<clientId>]}).
2. CreateRoute with AuthorizationType='JWT' and AuthorizationScopes naming a scope the client does not grant, CreateIntegration to a Lambda, CreateStage('$default').
3. GET the route with an access token lacking that scope -> expect 403 promptly.
