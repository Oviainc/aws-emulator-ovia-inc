Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-handler-raises-rejected-500 apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-without-invoke-permission-rejected-500 apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-malformed-proxy-response-rejected-500-not-502
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-handler-raises-rejected-500|run#10|AssertionError|assert text == f"{marker}:{prefix}/ok", (
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-without-invoke-permission-rejected-500|run#15|AssertionError|assert text == f"{marker}:/granted", (
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-malformed-proxy-response-rejected-500-not-502|run#11|AssertionError|assert text == f"{marker}:{path}", (
Issue-Commit: 49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4

# apigatewayv2__lambda: the HTTP API proxy payload prefixes the request path with the $default stage name, so every Lambda handler sees '/$default/<path>' in rawPath and path

> _Note: this issue was identified against the code base at commit `49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeApi

## Description
Cause 1 of 1: the Lambda proxy event the HTTP API delivers carries the serving stage name as a leading path segment.
AWS: an HTTP API invoked on its default endpoint https://{api-id}.execute-api.{region}.amazonaws.com/{path} is served by the $default stage, and $default contributes NO path segment — payload format 2.0 reports `rawPath` as the path the client sent ('/v1/ok', '/granted') and payload format 1.0 reports the same value in the top-level `path`; `requestContext.http.path` matches. Only a NAMED stage is addressed as /{stageName}/{path}, and only then does the stage name appear. Smithy: the data-plane payload is not a modeled shape — recorded behavior only; Api.ApiEndpoint documents the endpoint as '{api-id}.execute-api.{region}.amazonaws.com' with the stage appended to the URI, i.e. the stage lives in the URL and is never injected into the request path.
Emulator: every proxy event's path is prefixed with '/$default', e.g. a GET of '/v1/ok' arrives as '/$default/v1/ok' and '/granted' as '/$default/granted' (the handlers echo the path they received back to the client, so the 200 body is 'marker:/$default/v1/ok' instead of 'marker:/v1/ok').
Why: a handler that routes on rawPath/path — every framework adapter and every hand-written path comparison — does not recognise the path, so it mis-routes or 404s; in these checks the healthy control legs cannot even be established, which makes the real subjects (a raising handler rejected with 500, a route without invoke permission rejected with 500, and a malformed proxy response rejected 500-not-502) ungradeable. Any client contract keyed on the path the function observed is wrong by one segment.
Fix: the execute-api HTTP data-plane handler that builds the Lambda proxy event (v1.0 and v2.0 envelope construction): resolve the stage from the request URL, do not prepend it to the path it propagates, and keep a stage prefix only for a request made against a named stage.
Traces: apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-handler-raises-rejected-500, apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-lambda-without-invoke-permission-rejected-500, apigatewayv2__lambda.xsvc-apigatewayv2-3-http-api-malformed-proxy-response-rejected-500-not-502. The same root cause is reported for the apigatewayv2 service heading in this sweep, and open issue 20261107T085452Z-apigatewayv2-cloudformation-2-net-new-causes-the-http.md names it for the apigatewayv2__cloudformation heading; neither owns these three checks.

## Evidence
Replay details (AssertionError text), all three [mismatch]:
  xsvc-apigatewayv2-3-http-api-lambda-handler-raises-rejected-500: "the healthy twin /v1/ok must return '<marker>:/v1/ok', got 200 '<marker>:/$default/v1/ok'"
  xsvc-apigatewayv2-3-http-api-lambda-without-invoke-permission-rejected-500: "the granted route must reach the function and return '<marker>:/granted', got 200 '<marker>:/$default/granted'"
  xsvc-apigatewayv2-3-http-api-malformed-proxy-response-rejected-500-not-502: "format 1.0: the well-formed twin must return '<marker>:/v1/ok', got 200 '<marker>:/$default/v1/ok'"
(The marker is a per-run value; the requirement is that the path the function echoes equals the path the client requested, with no stage segment for $default. Both payload formats are affected — the third check fails on the format 1.0 `path` member.)

## Steps to Reproduce
Replay the behavioral suite and re-run the three checks in trace_ids. Minimal repro: CreateApi(ProtocolType=HTTP) with an AWS_PROXY integration to a Lambda whose handler echoes event.rawPath (payload 2.0) or event.path (payload 1.0), route 'GET /v1/ok', auto-deployed $default stage; then GET https://{api-id}.execute-api.{region}.amazonaws.com/v1/ok. The echoed path must be '/v1/ok'; the emulator echoes '/$default/v1/ok'. Repeat with PayloadFormatVersion 1.0 to see the same prefix in the top-level `path`, and against a named stage (where /{stage}/v1/ok legitimately appears in rawPath).
