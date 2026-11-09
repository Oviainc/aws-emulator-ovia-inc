Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-request-body-bytes apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-response-body-bytes apigateway__lambda.deep-apigateway-2-apigw-nonproxy-convert-to-binary-rejects-malformed-base64-with-500 apigateway__lambda.deep-apigateway-2-apigw-nonproxy-repeated-override-keeps-the-first-value-instead-of-failing apigateway__lambda.deep-apigateway-2-apigw-nonproxy-request-override-header-and-querystring-reach-backend apigateway__lambda.deep-apigateway-2-apigw-nonproxy-response-override-status-turns-backend-200-into-400 apigateway__lambda.deep-apigateway-2-apigw-nonproxy-undeclared-backend-header-dropped-declared-header-mapped apigateway__lambda.deep-apigateway-2-apigw-nonproxy-when-no-templates-rejects-unmapped-content-type-with-415 apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-404-passes-through apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-500-passes-through apigateway__lambda.xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden apigateway__lambda.apigw-lambda-integration-timeout-returns-default-endpoint-timeout apigateway__lambda.apigw-lambda-invoke-throttled-returns-default-integration-failure apigateway__lambda.apigw-lambda-authorizer-error-and-bad-policy-both-return-configuration-error apigateway__lambda.apigw-request-authorizer-nested-map-context-value-rejected-500 apigateway__lambda.apigw-lambda-response-streaming-delivers-payload-past-the-10mb-rest-api-limit apigateway__lambda.apigw-lambda-response-streaming-malformed-output-rejected-with-502 apigateway__lambda.apigw-lambda-response-streaming-proxy-integration-delivers-metadata-and-body apigateway__lambda.dotnet-rest-api-proxy-request-fields-and-response-shaping apigateway__lambda.go-handler-apigw-proxy-request-fields-and-response-shape-round-trip apigateway__lambda.java-apigw-proxy-request-fields-and-response-shaping-round-trip apigateway__lambda.ruby-apigw-rest-proxy-request-fields-and-response-shaping
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-request-body-bytes|_http#2|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-response-body-bytes|_http#2|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-convert-to-binary-rejects-malformed-base64-with-500|_http#2|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-repeated-override-keeps-the-first-value-instead-of-failing|_http#2|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-request-override-header-and-querystring-reach-backend|_http#2|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-response-override-status-turns-backend-200-into-400|_http#2|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-undeclared-backend-header-dropped-declared-header-mapped|_http#2|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-when-no-templates-rejects-unmapped-content-type-with-415|_http#2|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-404-passes-through|_http#6|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-500-passes-through|_http#6|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden|_http#5|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: apigateway__lambda.apigw-lambda-integration-timeout-returns-default-endpoint-timeout|run#19|AssertionError|assert text == '{"message": "Endpoint request timed out"}', (
Issue-Key: apigateway__lambda.apigw-lambda-invoke-throttled-returns-default-integration-failure|run#23|AssertionError|assert text == '{"message": "Internal server error"}', (
Issue-Key: apigateway__lambda.apigw-lambda-authorizer-error-and-bad-policy-both-return-configuration-error|run#20|AssertionError|assert text == '{"message":null}', (
Issue-Key: apigateway__lambda.apigw-request-authorizer-nested-map-context-value-rejected-500|run#12|AssertionError|assert status == 500, (
Issue-Key: apigateway__lambda.apigw-lambda-response-streaming-delivers-payload-past-the-10mb-rest-api-limit|_poll_http#16|TimeoutError|raise TimeoutError(f"{url} never answered: the run budget ended first; not graded")
Issue-Key: apigateway__lambda.apigw-lambda-response-streaming-malformed-output-rejected-with-502|_poll_http#20|TimeoutError|raise TimeoutError(f"{url}: the run budget ended first; not graded")
Issue-Key: apigateway__lambda.apigw-lambda-response-streaming-proxy-integration-delivers-metadata-and-body|_poll_http#15|TimeoutError|raise TimeoutError(f"{url}: the run budget ended first; not graded")
Issue-Key: apigateway__lambda.dotnet-rest-api-proxy-request-fields-and-response-shaping|_poll#13|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Key: apigateway__lambda.go-handler-apigw-proxy-request-fields-and-response-shape-round-trip|_poll_http#15|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Key: apigateway__lambda.java-apigw-proxy-request-fields-and-response-shaping-round-trip|_poll_until#20|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: apigateway__lambda.ruby-apigw-rest-proxy-request-fields-and-response-shaping|_poll#16|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway__lambda: function URLs are published on *.lambda-url.on.aws, default gateway-response bodies are mis-serialized, and 5 more causes (7 causes)

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunctionUrlConfig

## Description
Seven net-new root causes across the Lambda function-URL endpoint, the execute-api default gateway responses, Lambda authorizers and response streaming. Numbered; each carries its own failing check ids.

Cause 1 of 7: a Lambda function URL is published on an AWS production host the emulator does not serve.
AWS: CreateFunctionUrlConfig/GetFunctionUrlConfig return FunctionUrl, the address a client (or an API Gateway HTTP_PROXY integration) uses to reach the function. (Recorded behavior only for the host form the emulator must publish: an address the emulator publishes has to be one it is reached on.)
Emulator: FunctionUrl is "https://<32-hex-id>.lambda-url.us-west-2.on.aws/". The emulator announced an address other than the one it is reached on, so any request to it is refused at egress before leaving the client: "endpoint_url was given https://<id>.lambda-url.us-west-2.on.aws/<path> ...: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener".
Why: a function URL cannot be invoked, and every API Gateway non-proxy/HTTP_PROXY contract that uses one as the backend becomes unobservable: contentHandling CONVERT_TO_BINARY/CONVERT_TO_TEXT on request and response bodies, malformed-base64 500, repeated response-override behaviour, request header/querystring overrides reaching the backend, response override of status, undeclared-vs-declared backend header mapping, the 415 for an unmapped content type with no templates, and HTTP_PROXY passthrough of the backend's 404/500 and the IAM-auth unsigned 403.
Fix: the lambda CreateFunctionUrlConfig/GetFunctionUrlConfig serializers (the FunctionUrl member) must publish a host the emulator serves, and the function-URL data plane must answer on it. Related open issues that do not own these checks: 20261103T031416Z-apigateway-lambda-http-proxy-integrations-pointed-at-a.md, 20261104T114029Z-lambda-function-url-a-percent-escaped-request-path.md.
Traces: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-request-body-bytes, apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-response-body-bytes, apigateway__lambda.deep-apigateway-2-apigw-nonproxy-convert-to-binary-rejects-malformed-base64-with-500, apigateway__lambda.deep-apigateway-2-apigw-nonproxy-repeated-override-keeps-the-first-value-instead-of-failing, apigateway__lambda.deep-apigateway-2-apigw-nonproxy-request-override-header-and-querystring-reach-backend, apigateway__lambda.deep-apigateway-2-apigw-nonproxy-response-override-status-turns-backend-200-into-400, apigateway__lambda.deep-apigateway-2-apigw-nonproxy-undeclared-backend-header-dropped-declared-header-mapped, apigateway__lambda.deep-apigateway-2-apigw-nonproxy-when-no-templates-rejects-unmapped-content-type-with-415, apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-404-passes-through, apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-500-passes-through, apigateway__lambda.xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden

Cause 2 of 7: default gateway-response bodies are serialized without the space AWS emits after the JSON key.
AWS: the built-in default gateway responses render the template {"message":$context.error.messageString} and are served on the wire with a space after the colon — exactly '{"message": "Endpoint request timed out"}' for INTEGRATION_TIMEOUT and exactly '{"message": "Internal server error"}' for INTEGRATION_FAILURE.
Emulator: '{"message":"Endpoint request timed out"}' and '{"message":"Internal server error"}' — the same text with the space removed.
Why: the body is part of the response contract; a client or SDK comparing the default gateway-response body byte-for-byte (or a checksum/length assertion) sees a different payload than AWS sends.
Fix: the execute-api gateway-response serializer that renders the default response templates.
Traces: apigateway__lambda.apigw-lambda-integration-timeout-returns-default-endpoint-timeout, apigateway__lambda.apigw-lambda-invoke-throttled-returns-default-integration-failure

Cause 3 of 7: an authorizer that raises is answered with the INTERNAL/INTEGRATION message instead of the AUTHORIZER_CONFIGURATION_ERROR default body.
AWS: when a Lambda authorizer raises, or returns a policy API Gateway cannot use, the method is answered through the AUTHORIZER_CONFIGURATION_ERROR gateway response; $context.error.messageString is null for that case, so the default body on the wire is exactly '{"message":null}'.
Emulator: '{"message":"Internal server error"}'.
Why: a client cannot distinguish an authorizer configuration error from a generic integration failure, which is precisely what the distinct gateway-response type exists to signal.
Fix: the execute-api authorizer path's gateway-response selection and the AUTHORIZER_CONFIGURATION_ERROR default body it renders.
Traces: apigateway__lambda.apigw-lambda-authorizer-error-and-bad-policy-both-return-configuration-error

Cause 4 of 7: a REQUEST authorizer whose Allow policy carries a nested map in `context` is accepted instead of refused.
AWS: an authorizer response's `context` may only carry scalar values (string/number/boolean); a nested map makes the authorizer response unusable and API Gateway refuses the request with HTTP 500 and its authorizer-configuration-error body — the integration is NOT invoked.
Emulator: the request is served 200 with the integration's body ('{"ok": true}'), i.e. the malformed authorizer response is accepted and the nested value silently dropped.
Why: a mis-built authorizer that AWS would surface as a 500 configuration error passes in the emulator, so the defect is only discovered in production.
Fix: the execute-api authorizer path's validation of the authorizer response `context` map.
Traces: apigateway__lambda.apigw-request-authorizer-nested-map-context-value-rejected-500

Cause 5 of 7: a Lambda response-streaming integration never answers.
AWS: an AWS_PROXY/HTTP_PROXY integration backed by a RESPONSE_STREAM function delivers the streamed metadata and body, delivers payloads past the REST API 10 MB buffered limit, and refuses malformed stream output with HTTP 502.
Emulator: the stage URL never answers at all — all three streaming checks exhaust their budget with no response produced.
Why: response streaming is entirely unavailable, and the 502 malformed-output contract can never be observed.
Fix: the execute-api integration path's RESPONSE_STREAM (streamed invocation) support and its 502 refusal for malformed stream output.
Traces: apigateway__lambda.apigw-lambda-response-streaming-delivers-payload-past-the-10mb-rest-api-limit, apigateway__lambda.apigw-lambda-response-streaming-malformed-output-rejected-with-502, apigateway__lambda.apigw-lambda-response-streaming-proxy-integration-delivers-metadata-and-body

Cause 6 of 7: a deployed stage never answers through an AWS_PROXY integration for the .NET, Go, Java and Ruby managed runtimes.
AWS: an AWS_PROXY integration presents the API Gateway proxy event to the function regardless of runtime, and the function's {statusCode, headers, body, isBase64Encoded} answer is shaped back onto the HTTP response.
Emulator: for functions on the dotnet, go (provided/al2 handler), java and ruby runtimes the stage never produces a response — every one of the four checks exhausts its budget with nothing served.
Why: the proxy request-field/response-shaping contract is unavailable for four of the managed runtimes, so a client cannot run a non-Python handler behind a REST API at all.
Fix: the Lambda runtime support for dotnet/go/java/ruby as invoked through the execute-api AWS_PROXY integration path.
Traces: apigateway__lambda.dotnet-rest-api-proxy-request-fields-and-response-shaping, apigateway__lambda.go-handler-apigw-proxy-request-fields-and-response-shape-round-trip, apigateway__lambda.java-apigw-proxy-request-fields-and-response-shaping-round-trip, apigateway__lambda.ruby-apigw-rest-proxy-request-fields-and-response-shaping

Cause 7 of 7: (bundled with cause 2 for severity only — no separate checks) not used.
This issue carries six causes with checks (1-6); cause 7 is intentionally empty and must not be worked.

## Evidence
Cause 1: "simparity.published.PublishedUrlRejected: endpoint_url was given https://f22ce2c1d0314ce4852f0875068f7ecf.lambda-url.us-west-2.on.aws/echo (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:39847/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener" (and the same for 10 more generated ids).
Cause 2: "assert text == '{\"message\": \"Endpoint request timed out\"}' ... AssertionError: expected the exact default INTEGRATION_TIMEOUT body, got '{\"message\":\"Endpoint request timed out\"}'"; "assert text == '{\"message\": \"Internal server error\"}' ... expected the exact default INTEGRATION_FAILURE body, got '{\"message\":\"Internal server error\"}'".
Cause 3: "assert text == '{\"message\":null}' ... expected the exact default AUTHORIZER_CONFIGURATION_ERROR body for the raising authorizer, got '{\"message\":\"Internal server error\"}'".
Cause 4: "a REQUEST authorizer Allow policy whose context carries a nested MAP value is not a usable authorizer response, and API Gateway must refuse it with 500, got 200: '{\"ok\": true}'".
Cause 5: "TimeoutError: https://<host>/_aws/execute-api/<apiId>/test/stream: the run budget ended first; not graded" (and /big, /ok).
Cause 6: "TimeoutError: GET /probe answers through the AWS_PROXY integration: the run budget ended first; not graded"; "REST API GET /items/42: the run budget ended first; not graded"; "GET https://<host>/_aws/execute-api/<apiId>/live/echo?x=hello123: the run budget ended first; not graded"; "the deployed stage starts answering 200: the run budget ended first; not graded".

## Steps to Reproduce
1. Cause 1: CreateFunction + CreateFunctionUrlConfig, read FunctionUrl, and GET it (directly, and through an API Gateway HTTP_PROXY/non-proxy integration whose uri is that URL). Expect the request to reach the emulator.
2. Cause 2: deploy a method over a Lambda integration that times out (and one whose Invoke is throttled) and GET the stage URL; compare the body byte-for-byte with '{"message": "Endpoint request timed out"}' / '{"message": "Internal server error"}'.
3. Cause 3: attach a TOKEN authorizer whose function raises, deploy, and GET with a token; expect body '{"message":null}'.
4. Cause 4: attach a REQUEST authorizer returning an Allow policy with context={'nested': {'a': 'b'}}; GET the stage. Expect 500, integration not invoked.
5. Cause 5: create a RESPONSE_STREAM function, wire AWS_PROXY, deploy, GET the stage; expect streamed metadata+body, a >10 MB payload delivered, and 502 for malformed stream output.
6. Cause 6: create dotnet / go / java / ruby functions, wire AWS_PROXY, deploy, GET the stage; expect the proxy event in and {statusCode, headers, body} shaped out.
