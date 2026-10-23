Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-allow-passes-context-to-message-routes apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401
Issue-Key: apigatewayv2__lambda|CreateAuthorizer|NotImplemented
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# apigatewayv2__lambda: WebSocket REQUEST-authorizer checks abort because apigatewayv2 CreateAuthorizer answers NotImplemented (1 cause)

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
One net-new root cause, reached through the apigatewayv2__lambda cross-service token. The checks that grade WebSocket $connect authorization all fail at the same setup call.

Cause 1 of 1: apigatewayv2 CreateAuthorizer is an unimplemented scaffold stub, so no REQUEST authorizer can be attached to a WebSocket API's $connect route.
AWS: POST /v2/apis/{ApiId}/authorizers returns 201 with the Authorizer representation (Smithy, apigatewayv2: CreateAuthorizer -> CreateAuthorizerResponse {AuthorizerId, AuthorizerType, AuthorizerUri, IdentitySource, AuthorizerResultTtlInSeconds, Name, ...}; required input members Name, AuthorizerType, IdentitySource; http {POST, /v2/apis/{ApiId}/authorizers, 201}; modeled errors BadRequestException 400, ConflictException 409, NotFoundException 404, TooManyRequestsException 429 — NotImplemented is not modeled). With a REQUEST authorizer on $connect, AWS then: completes the handshake and passes the authorizer's context to the message routes on an Allow policy; refuses the handshake with HTTP 403 on a Deny policy; and refuses it with HTTP 401 when a declared identity source is absent from the request.
Emulator: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAuthorizer operation: operation CreateAuthorizer is scaffolded but not implemented in this emulator.
Why: WebSocket (and HTTP API) authorization cannot be configured at all, so a client cannot protect a $connect route, and the 403-on-deny / 401-on-missing-identity-source handshake contract — the part clients depend on to distinguish 'rejected' from 'unauthenticated' — is absent. Callers also receive an error code that is not in the model.
Fix: implement the CreateAuthorizer handler in the apigatewayv2 service crate's ops module (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_authorizer.rs), persist the authorizer in that crate's state so Get/Update/DeleteAuthorizer and route AuthorizerId references resolve, and wire the REQUEST authorizer invocation into the $connect handshake path.
Traces: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-allow-passes-context-to-message-routes, .xsvc-apigatewayv2-2-ws-request-authorizer-deny-policy-forbidden-403-handshake, .xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401. (The suite reports 6 failing apigatewayv2__lambda checks; the report's detail blocks were truncated after these three, so only these three are keyed here.)

## Evidence
Suite report: apigatewayv2__lambda 0/6 (0%) 6 failed. The three blocks whose detail the report retained all end with

  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAuthorizer operation: operation CreateAuthorizer is scaffolded but not implemented in this emulator

Expected: HTTP 201 + CreateAuthorizerResponse{AuthorizerId, AuthorizerType:'REQUEST', IdentitySource, ...}. Actual: unmodeled NotImplemented, so the $connect handshake is never exercised against an authorizer and the 403 (Deny) / 401 (missing identity source) outcomes are unverified.

## Steps to Reproduce
1. Create a WEBSOCKET api plus a Lambda authorizer function (succeeds).
2. aws apigatewayv2 create-authorizer --api-id <id> --name auth --authorizer-type REQUEST --authorizer-uri <lambda-invoke-uri> --identity-source '$request.querystring.token' -> ClientError NotImplemented ('operation CreateAuthorizer is scaffolded but not implemented in this emulator'); AWS returns 201 + AuthorizerId.
3. Consequently a $connect route cannot set AuthorizationType=CUSTOM/AuthorizerId, and the handshake cannot be made to answer 403 on a Deny policy or 401 when the identity source is missing.
