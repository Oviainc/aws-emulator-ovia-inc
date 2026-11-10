Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda.rest-api-cors-signed-get-granted-unsigned-get-refused
Issue-Key: apigateway__lambda.rest-api-cors-signed-get-granted-unsigned-get-refused|_poll#14|AssertionError|raise AssertionError(f"{description} not answered {want} within {timeout_s}s; last {last!r}")
Issue-Commit: d9137e240d6d86ce2d0eba5736851daf32b7e9eb

# apigateway execute-api: a SigV4-signed cross-origin GET on an AWS_IAM method is refused 403 AccessDenied ("Requests a browser sends from another site are refused")

> _Note: this issue was identified against the code base at commit `d9137e240d6d86ce2d0eba5736851daf32b7e9eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
One net-new root cause.

Cause 1 of 1: the REST execute-api data plane refuses an authorized request solely because it carries a cross-site `Origin` header. Failing check: apigateway__lambda.rest-api-cors-signed-get-granted-unsigned-get-refused.

AWS: API Gateway does not enforce CORS on the actual (non-preflight) request. For a REST method whose authorizationType is AWS_IAM, the only authorization input is the SigV4 signature in the Authorization header; the `Origin` request header is never an authorization input and is forwarded to the integration like any other header (the AWS_PROXY Lambda event sees it under `headers.origin`). AWS answers such a signed GET with the integration's own response — here the proxy's 200 plus whatever Access-Control-Allow-Origin / Access-Control-Expose-Headers the handler sets — and leaves enforcement of those headers to the browser. Only the OPTIONS preflight is answered by the gateway itself (the MOCK OPTIONS method's integration-response mapping). The execute-api data plane is not described by the apigateway Smithy model, so this is recorded behavior plus AWS's documented CORS rule (https://docs.aws.amazon.com/apigateway/latest/developerguide/how-to-cors.html); there is no modeled error named `AccessDenied` for a cross-origin signed request, and AWS's refusal vocabulary for this data plane is `Missing Authentication Token` (403) for an unsigned call on an AWS_IAM method and `{"message":"Forbidden"}` / an IAM-denial message when the signature identity is denied.

Emulator: every signed GET to the deployed stage resource is answered HTTP 403 with header `x-amzn-errortype: AccessDenied` and body `{"__type":"AccessDenied","message":"Requests a browser sends from another site are refused"}`. The check polled the positive control for ~43s and got that same 403 on every attempt, so the integration is never reached. Both the status code, the error code `AccessDenied` and the whole message string are fixed text the emulator invents; nothing in it is derived from the request or from state, and no part of it corresponds to an AWS error for this case.

Why: a browser-based client of an IAM-authorized REST API is refused outright. No CORS configuration on the API can make the call succeed, because the refusal happens before authorization and before the integration; the handler never sees the Origin it is supposed to echo, and the client receives an error code (`AccessDenied`) and message AWS never emits, so client-side error handling keyed on AWS's codes cannot classify it.

Fix: the apigateway execute-api REST data-plane request dispatch/authorizer. Remove the Origin-based refusal from the request path: `Origin` must be passed through to the integration (and into the AWS_PROXY event headers) and must have no effect on authorization; the gateway answers only the OPTIONS preflight itself, and a signed request on an AWS_IAM method is authorized from the SigV4 signature alone. The related open issue 20261104T220426Z-apigateway-a-cors-preflight-to-a-resource-with.md covers the OPTIONS/MOCK preflight answer, not this refusal of the signed GET, and 20261107T191316Z-apigateway-lambda-3-net-new-causes-non-proxy-d346f3.md covers non-proxy integration 500s; neither carries this check's replay key.

Traces: apigateway__lambda.rest-api-cors-signed-get-granted-unsigned-get-refused.

## Evidence
Expected (AWS): signed GET https://<stage endpoint>/items with `Origin: https://app.example.com` -> 200, body {"marker":"sim2real-browser-cors","seen":{"origin":"https://app.example.com"}}, response headers access-control-allow-origin: https://app.example.com and access-control-expose-headers: x-app-marker (set by the Lambda proxy handler). An unsigned GET on the same AWS_IAM method -> 403 {"message":"Missing Authentication Token"} with no CORS header.

Actual (emulator): every signed GET -> 403, headers {content-type: application/json, x-amzn-errortype: AccessDenied}, body {"__type":"AccessDenied","message":"Requests a browser sends from another site are refused"}. Replay output:
  AssertionError: signed GET https://127.0.0.1:<port>/_aws/execute-api/<api-id>/live/items from https://app.example.com not answered 200 within 42.75s; last (403, {... 'x-amzn-errortype': 'AccessDenied' ...}, '{"__type":"AccessDenied","message":"Requests a browser sends from another site are refused"}')
Fixed text in the emulator's message: the whole string "Requests a browser sends from another site are refused" and the code "AccessDenied" — neither is AWS wording for this case. Variable parts of AWS's own refusals here are none for "Missing Authentication Token"; the host/port/api-id above are per-environment values, not requirements.

## Steps to Reproduce
1. CreateRestApi (REGIONAL), create resource /items.
2. Put a MOCK OPTIONS method on /items whose integration response returns Access-Control-Allow-Origin: https://app.example.com, Access-Control-Allow-Methods: GET,OPTIONS, Access-Control-Allow-Headers: authorization,x-amz-date.
3. PutMethod GET on /items with authorizationType=AWS_IAM and an AWS_PROXY integration to a Lambda function that echoes event.headers.origin and sets its own Access-Control-Allow-Origin/Expose-Headers; lambda:AddPermission for apigateway.amazonaws.com on arn:aws:execute-api:<region>:<account>:<api-id>/*/GET/items.
4. CreateDeployment stageName=live.
5. Send a SigV4-signed GET to <stage>/items with header Origin: https://app.example.com.
6. Observe 403 AccessDenied "Requests a browser sends from another site are refused" instead of the handler's 200.
Replay: apigateway__lambda.rest-api-cors-signed-get-granted-unsigned-get-refused.
