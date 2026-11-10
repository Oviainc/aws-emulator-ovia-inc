Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event apigatewayv2__cognito-idp__lambda.jwt-authorizer-expired-token-rejected-never-reaches-lambda
Issue-Key: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered|_v2_invoke_url#18|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{path.lstrip('/')}"
Issue-Key: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience|_v2_invoke_url#18|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{path.lstrip('/')}"
Issue-Key: apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event|_v2_invoke_url#17|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{path.lstrip('/')}"
Issue-Key: apigatewayv2__cognito-idp__lambda.jwt-authorizer-expired-token-rejected-never-reaches-lambda|_v2_invoke_url#16|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{path.lstrip('/')}"
Issue-Commit: 68689e45c55a2a3c2aeeec91dff9a251e0f23b15

# apigatewayv2__cognito-idp__lambda: an HTTP API's invoke address is an AWS execute-api host the emulator does not serve, so no JWT-authorizer behaviour is reachable

> _Note: this issue was identified against the code base at commit `68689e45c55a2a3c2aeeec91dff9a251e0f23b15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
execute-api:Invoke

## Description
Cause 1 of 1: the address in play for a deployed HTTP API stage is a real-AWS execute-api host, not an address the emulator answers on.
AWS: recorded behavior only (the execute-api data plane is not Smithy-modelled). GetApi/GetStage publish an ApiEndpoint for a deployed HTTP API and that endpoint serves requests; whatever host the emulator publishes it must itself serve.
Emulator: the endpoint in play is https://<api-id>.execute-api.<region>.amazonaws.com, which is not one of the emulator's own hosts, so the request is refused before it leaves the harness: 'endpoint_url was given https://<api-id>.execute-api.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener'. The concrete host/port differ per run; the requirement is only that the announced address be one the emulator is reached on.
Why: every HTTP API JWT-authorizer contract is unobservable — a valid token cannot be served, a foreign-issuer/wrong-audience token cannot be refused 401, a scope mismatch cannot be refused, and jwt.claims can never reach the Lambda event. A customer pointing a client at the published endpoint gets a connection to an address the emulator does not own.
Fix: the apigatewayv2 front door / execute-api listener and the ApiEndpoint it publishes for a deployed stage (own host or per-resource listener), not the amazonaws.com form. Related open issue that does NOT cover these checks: 20261109T120740Z-iot-the-published-data-plane-address-is-a.md (same defect class, filed under iot).
Traces: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered, apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience, apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event, apigatewayv2__cognito-idp__lambda.jwt-authorizer-expired-token-rejected-never-reaches-lambda.

## Evidence
All four checks fail identically before any HTTP request is attempted:
simparity.published.PublishedUrlRejected: endpoint_url was given https://mp67t4dbd4.execute-api.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:45923/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener.

## Steps to Reproduce
1) Create a Cognito user pool + client, an HTTP API with a JWT authorizer bound to that pool, an AWS_PROXY route to a Lambda and a deployed stage.
2) Read the API's endpoint from the emulator and issue a request to it with a Cognito access token.
3) The endpoint is an <api-id>.execute-api.<region>.amazonaws.com host the emulator does not serve, so the call never reaches the authorizer or the integration.
