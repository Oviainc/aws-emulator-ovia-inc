Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.http-api-cors-signed-iam-get-granted-unsigned-get-refused
Issue-Key: apigatewayv2__lambda.http-api-cors-signed-iam-get-granted-unsigned-get-refused|run#10|PublishedUrlRejected|url = f"{ctx.stage_url(api['ApiEndpoint'], '$default')}/items"
Issue-Commit: d9137e240d6d86ce2d0eba5736851daf32b7e9eb

# apigatewayv2 CreateApi/GetApi publish ApiEndpoint as https://<api-id>.execute-api.<region>.amazonaws.com, a host the emulator does not serve

> _Note: this issue was identified against the code base at commit `d9137e240d6d86ce2d0eba5736851daf32b7e9eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
One net-new root cause.

Cause 1 of 1: the HTTP API's published `apiEndpoint` names a real-AWS host instead of the emulator's own reachable address, so every client that follows it leaves the emulator. Failing check: apigatewayv2__lambda.http-api-cors-signed-iam-get-granted-unsigned-get-refused.

AWS: Smithy (apigatewayv2) models `CreateApi` -> CreateApiResponse member `ApiEndpoint` (target `__string`, jsonName `apiEndpoint`, HTTP POST /v2/apis, 201) and `GetApi` -> GetApiResponse with the same member (HTTP GET /v2/apis/{ApiId}, 200). The contract of that member is an endpoint that actually serves the API's stages: on AWS it is `https://{api-id}.execute-api.{region}.amazonaws.com`, and that host resolves and answers the API's routes. For an emulator the requirement is the same contract, not the same literal host: the value must be an address the emulator itself answers on, because `apiEndpoint` is the only address a client is given for the data plane.

Emulator: `apiEndpoint` is published verbatim as `https://<api-id>.execute-api.<region>.amazonaws.com`. That host is not one the emulator serves, so the published-URL guard refuses it before any request is made: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<api-id>.execute-api.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. The api-id, region, host and port in that text are per-run values; the defect is the announced host family (`*.execute-api.<region>.amazonaws.com`) rather than the emulator's own.

Why: a client that reads `apiEndpoint` and dials it is sent off the emulator to real AWS (or to a name that does not resolve), so none of the HTTP API's data-plane behaviour — route dispatch, the IAM authorizer, the CorsConfiguration grant on the actual request and on the preflight — can be exercised at all. Every HTTP API data-plane check built from the published endpoint is blocked by this one value.

Fix: the apigatewayv2 `ApiEndpoint` value in state/serialization for CreateApi, GetApi and GetApis (and anywhere the stage/data-plane URL is derived from it) must publish the emulator's own reachable execute-api address. The similar open issues 20261109T143515Z-apigatewayv2-lambda-logs-apigatewayv2-the-api-endpoint-is-published-as-an.md (service apigatewayv2__lambda__logs) and 20261109T143515Z-apigateway-dynamodb-lambda-logs-http-api-apiendpoint-is-published-as-id-execute.md (service apigateway__dynamodb__lambda__logs) describe the same shape of defect for other services; neither carries this check's replay key, so this check is not covered by them.

Traces: apigatewayv2__lambda.http-api-cors-signed-iam-get-granted-unsigned-get-refused.

## Evidence
Expected (AWS): CreateApi(ProtocolType=HTTP, CorsConfiguration={AllowOrigins:[https://app.example.com], AllowMethods:[GET], AllowHeaders:[authorization,x-amz-date], ExposeHeaders:[x-app-marker]}) returns an `apiEndpoint` that serves the API; a signed GET to <apiEndpoint>/<route> on an AWS_IAM-authorized route reaches the Lambda integration and returns 200 with the configured Access-Control-* headers.

Actual (emulator): `apiEndpoint` = https://mp67t4dbd4.execute-api.us-west-2.amazonaws.com — not an address the emulator answers on. Replay output:
  File "context.py", line 586, in _published_target
  simparity.published.PublishedUrlRejected: endpoint_url was given https://mp67t4dbd4.execute-api.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:35027/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
(The api-id `mp67t4dbd4`, the region and the port 35027 are per-run values; the requirement is that the announced host be the emulator's own.)

## Steps to Reproduce
1. CreateApi with ProtocolType=HTTP and a CorsConfiguration.
2. Read `apiEndpoint` from the CreateApi (or GetApi) response.
3. Observe it is https://<api-id>.execute-api.<region>.amazonaws.com — a host the emulator does not serve — rather than the emulator's own http(s)://<own host>:<port>/... address.
4. Any request to that endpoint never reaches the emulator's HTTP API data plane.
Replay: apigatewayv2__lambda.http-api-cors-signed-iam-get-granted-unsigned-get-refused.
