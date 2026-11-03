Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-404-passes-through apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-500-passes-through apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-round-trips-path-query-headers
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-404-passes-through|_await#4|Timeout|raise AssertionError(f"timed out after {timeout_s}s waiting for {what}; last={last!r}")
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-500-passes-through|_await#4|Timeout|raise AssertionError(f"timed out after {timeout_s}s waiting for {what}; last={last!r}")
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-round-trips-path-query-headers|_await#6|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# apigateway__lambda: HTTP_PROXY integrations pointed at a Lambda function URL never complete — three checks hang to the 60 s harness cap

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegration

## Description
Cause 1 of 1: a REST API method with an HTTP_PROXY integration whose URI is a Lambda function URL never answers. All three checks that exercise this shape (pass-through of a 404, pass-through of a 500, and round-tripping of path/query/headers) make no progress and are killed at the harness's 60 s cap.
AWS: an HTTP_PROXY integration forwards the request to the configured `uri` and returns the upstream response to the caller verbatim — status (including 404 and 500), headers and body — and the request's path, query string and headers reach the upstream function URL unmodified. Recorded behavior only: both the stage URL and the Lambda function URL are data planes with no modelled operation, and the recordings show the request never resolving.
Emulator: all three checks report `Error: Check timed out after 60.0s`. The emulator returns neither the upstream response nor an error code; the request hangs. (For the function-url leg specifically, the emulator elsewhere answers 404 `{"__type":"UnknownService"}` for function-url hosts, which is consistent with the integration target never being routed.)
Why: a client that fronts a Lambda function URL with API Gateway sees requests that never return instead of the upstream status it depends on — a 404 and a 500 both become an indefinite hang, so neither error handling nor retry logic can work.
Fix: the apigateway HTTP_PROXY integration forwarder in the stage/execute-api data plane, and the Lambda function-URL host routing it targets. Related open issue `20261029T193654Z-apigateway-lambda-api-gateway-lambda-16-integration-checks-never-complete.md` covers other integration checks for this service pair but does not carry these three replay keys.
Traces: apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-404-passes-through, apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-500-passes-through, apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-round-trips-path-query-headers

## Evidence
Expected: the HTTP_PROXY integration returns the function URL's own status/body — 404 stays 404, 500 stays 500 — and the path, query string and headers arrive at the function unchanged. Actual: no response at all; replay reports `Error: Check timed out after 60.0s` for each of the three checks, with no HTTP status, error code or message from the emulator.

## Steps to Reproduce
1. Replay the behavioral suite and re-run the three checks listed in trace_ids.
2. Create a Lambda with a function URL (AuthType NONE) whose handler returns 200/404/500 depending on the path.
3. Create a REST API method with `type=HTTP_PROXY` and `uri=<function-url>`, deploy to a stage, and GET the stage URL.
4. Observe the request never returns and the check is killed at 60 s.
