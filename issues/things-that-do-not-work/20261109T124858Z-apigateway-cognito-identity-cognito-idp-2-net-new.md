Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-expired-token-negative apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-expired-token-negative|run#20|AssertionError|assert rejection_message == "The incoming token has expired", (
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative|run#14|PublishedUrlRejected|url = f"{ctx.stage_url(api_endpoint, '$default')}/test"
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive|run#16|PublishedUrlRejected|url = f"{ctx.stage_url(api_endpoint, '$default')}/test"
Issue-Commit: 68689e45c55a2a3c2aeeec91dff9a251e0f23b15

# apigateway__cognito-identity__cognito-idp: 2 net-new causes — a Cognito-authorizer 401 carries the generic 'Unauthorized' message, and the REST API's invoke host is an AWS execute-api host the emulator does not serve

> _Note: this issue was identified against the code base at commit `68689e45c55a2a3c2aeeec91dff9a251e0f23b15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
execute-api:Invoke

## Description
Two net-new root causes in the API Gateway REST data plane fronted by a Cognito user-pool authorizer.

Cause 1 of 2: the expired-token refusal message of a COGNITO_USER_POOLS authorizer is the generic 'Unauthorized'.
AWS: recorded behavior only (the execute-api data plane is not described by a Smithy model). A request carrying an expired Cognito ID/access token to a method gated by a COGNITO_USER_POOLS authorizer is answered HTTP 401 with the JSON body {"message":"The incoming token has expired"}. The fixed text of that message is the whole sentence 'The incoming token has expired' — it has no variable part; the generic 'Unauthorized' sentence is what AWS uses for a MISSING/unparseable identity source, not for a token that parsed and is past its exp.
Emulator: answers the refusal with message 'Unauthorized', so the expired-token class is indistinguishable from the missing-token class.
Why: a client (or an SDK/SPA retry policy) that keys off the message to decide 'refresh the token and retry' versus 'sign in again' takes the wrong branch, and log-based diagnosis of token expiry is impossible.
Fix: the execute-api data plane's authorizer-evaluation path that renders the 401 body for a COGNITO_USER_POOLS authorizer — the token-expiry branch must emit AWS's sentence instead of falling through to the generic one.
Traces: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-expired-token-negative.

Cause 2 of 2: the REST API's invoke address is a real-AWS execute-api host that the emulator does not serve.
AWS: recorded behavior only. A deployed REST API stage is reachable at the API's own invoke URL; the emulator must announce/serve an address it is actually reached on.
Emulator: the invoke address in play is https://<api-id>.execute-api.<region>.amazonaws.com — a host that is not one of the emulator's own hosts, so the harness refuses the egress before any request is made ('endpoint_url was given https://<api-id>.execute-api.us-west-2.amazonaws.com ... its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener'). The host/port values differ per run; the requirement is that the announced address be one the emulator serves.
Why: no customer of the emulator can invoke a deployed REST API at all, so every authorizer/claims behaviour behind the stage is unreachable.
Fix: the apigateway front door / execute-api data-plane listener and the address it publishes for a deployed stage (per-resource listener or own-host URL) rather than the amazonaws.com form. Related open issue, which does NOT cover these checks: 20261103T000000Z-apigateway-cloudformation-a-template-deployed-rest-api-stage-url-answers-404-unknownservice.md.
Traces: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative, apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive.

## Evidence
Cause 1 — expected the specific expired-token message, got 'Unauthorized' (assertion: rejection_message == "The incoming token has expired").
Cause 2 — simparity.published.PublishedUrlRejected: endpoint_url was given https://mp67t4dbd4.execute-api.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:33575/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener.

## Steps to Reproduce
1) Create a Cognito user pool + app client and a REST API whose method is gated by a COGNITO_USER_POOLS authorizer; deploy a stage.
2) Cause 1: sign in, let the ID token pass its exp (or mint one with an exp in the past), call the method with it and read the 401 body's message — AWS says 'The incoming token has expired', the emulator says 'Unauthorized'.
3) Cause 2: ask the emulator for the deployed stage's invoke address and try to call it — the address is an <api-id>.execute-api.<region>.amazonaws.com host the emulator does not serve.
