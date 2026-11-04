Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers|_http#7|EndpointRefused|target, route = ctx.endpoint_route(url)
Issue-Commit: 92dc3139d3cb231d5080684849dc2f19903a19e5

# lambda function URL: a percent-escaped request path is refused at egress, so the payload-format-2.0 event (rawPath/rawQueryString/lowercased headers) is never observable

> _Note: this issue was identified against the code base at commit `92dc3139d3cb231d5080684849dc2f19903a19e5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunctionUrlConfig

## Description
One net-new root cause (Cause 1 of 1).

AWS: Smithy (lambda, com-amazonaws-lambda-awsgirapiservice) models CreateFunctionUrlConfig POST /2021-10-31/functions/{FunctionName}/url -> 201 with a REQUIRED output member FunctionUrl, target FunctionUrl, @length min 40 max 100. Real AWS returns an addressable HTTPS endpoint for that function (shape `https://<url-id>.lambda-url.<region>.on.aws/`), and a GET against it is served by the function URL data plane: with payload format 2.0 the handler event carries rawPath with the request path's percent-escapes preserved byte-for-byte (a request for `/some/pa%20th/x` arrives as rawPath `/some/pa%20th/x`, NOT decoded to `/some/pa th/x`), rawQueryString verbatim, and every request header key lowercased. A `%HH` sequence with two hex digits is a legal path escape per RFC 3986, so such a request is a normal 200 for an existing route — AWS does not refuse it.

Emulator: the check never gets a response at all. The function URL it is handed is a bare loopback origin, and the request to `<origin>/some/pa%20th/x` is refused before it leaves the harness:
  simparity.target.EndpointRefused: endpoint 'https://127.0.0.1:34035/some/pa%20th/x' has a '%' that is not a path escape
(raised from context.py `_require_emulator_url` -> target.py `validate_endpoint`). So no GET reaches the emulator, and nothing about the v2.0 event shape can be observed. Only the host/port in that message are environment-specific; the refusal of a legally percent-escaped path, and the fact that the published function URL is a bare `scheme://host:port` origin rather than a per-function addressable endpoint of the modeled 40..100 character shape, are the behaviour at issue.

Why: a client that creates a function URL and then issues any request whose path contains a percent escape (spaces, `/`, unicode, or any reserved character in a path segment — ordinary for URL-driven APIs) cannot reach the function at all here, while on AWS it is served and the handler sees the raw, still-escaped path. Every assertion about function-URL request fidelity (rawPath preservation, rawQueryString, header lowercasing) is unverifiable against this emulator, so code that depends on raw-path routing passes locally only because it is never exercised.

Hypothesis (neither the model nor the recording states the cause): CreateFunctionUrlConfig publishes the emulator's own `scheme://host:port` origin as FunctionUrl instead of a per-function endpoint of the modeled length/shape, and that announced origin is not reachable for a percent-escaped path, so the egress check refuses the address rather than the emulator answering it.

Fix: the function URL publication in the lambda crate's CreateFunctionUrlConfig/GetFunctionUrlConfig handler (the FunctionUrl it mints must satisfy the modeled @length 40..100 and be the address the data plane is actually reached on), plus the function URL data-plane router, which must accept a percent-escaped request path and hand the handler a payload-format-2.0 event whose rawPath keeps the escapes and whose header keys are lowercased.

Related but NOT covering this: open issues 20261023T114649Z-lambda-function-urls-unrouted-configuration-blocks-dropped-from.md and 20261029T211043Z-lambda-data-plane-function-url-endpoints-are-unrouted.md describe function-URL endpoints being unrouted; the replay marks neither as owning this check, and this failure is an egress refusal of a percent-escaped path on the published address, not an unrouted 404.

Traces: behavioral check lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers (verdict: error), the only failing check in this sweep that no filed issue owns.

## Evidence
Suite report, '### lambda' section:

#### [error] lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers
Error: File "context.py", line 331, in _require_emulator_url
  validate_endpoint(url.split("?", 1)[0].split("#", 1)[0], schemes=schemes, path=True)
File "target.py", line 149, in validate_endpoint
  raise EndpointRefused(f"endpoint {endpoint!r} has a '%' that is not a path escape")
simparity.target.EndpointRefused: endpoint 'https://127.0.0.1:34035/some/pa%20th/x' has a '%' that is not a path escape

Expected (AWS): CreateFunctionUrlConfig returns FunctionUrl (required, @length 40..100) naming an addressable HTTPS endpoint for the function; GET <FunctionUrl>some/pa%20th/x?<query> returns the handler's 200 response and the payload-format-2.0 event carries rawPath '/some/pa%20th/x' (escapes preserved), rawQueryString verbatim and lowercased header keys.
Actual (emulator): the published URL is a bare scheme://host:port origin and the percent-escaped request path is refused at egress, so no request reaches the function and no event is produced. Host, port and the generated url-id differ per run and are not part of the requirement.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point the harness at it.
2. Create a python3.12 function whose handler echoes the event as JSON: return {"statusCode": 200, "headers": {"content-type": "application/json"}, "body": json.dumps(event)}.
3. Call lambda CreateFunctionUrlConfig (POST /2021-10-31/functions/<name>/url) with AuthType=NONE and keep the returned FunctionUrl.
4. Issue GET <FunctionUrl>some/pa%20th/x?a=1&a=2&b=%20 with a mixed-case request header.
5. Observe: the request is refused before it is sent — EndpointRefused: endpoint '<FunctionUrl>some/pa%20th/x' has a '%' that is not a path escape — instead of a 200 whose JSON body shows rawPath '/some/pa%20th/x', rawQueryString 'a=1&a=2&b=%20' and lowercased header keys.
6. Equivalent single step: replay behavioral check lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers.
