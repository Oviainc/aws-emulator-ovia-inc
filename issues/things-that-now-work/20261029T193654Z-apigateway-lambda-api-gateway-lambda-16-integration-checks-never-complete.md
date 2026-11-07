Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda.apigw-binary-media-types-round-trip-raw-bytes apigateway__lambda.apigw-lambda-proxy-unhandled-exception-bad-gateway apigateway__lambda.apigw-mock-integration-serves-cors-preflight apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-request-body-bytes apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-response-body-bytes apigateway__lambda.deep-apigateway-2-apigw-nonproxy-convert-to-binary-rejects-malformed-base64-with-500 apigateway__lambda.deep-apigateway-2-apigw-nonproxy-repeated-override-keeps-the-first-value-instead-of-failing apigateway__lambda.deep-apigateway-2-apigw-nonproxy-request-override-header-and-querystring-reach-backend apigateway__lambda.deep-apigateway-2-apigw-nonproxy-response-override-status-turns-backend-200-into-400 apigateway__lambda.deep-apigateway-2-apigw-nonproxy-undeclared-backend-header-dropped-declared-header-mapped apigateway__lambda.deep-apigateway-2-apigw-nonproxy-when-no-templates-rejects-unmapped-content-type-with-415 apigateway__lambda.deep-apigw-integration-rest-aws-proxy-event-carries-the-v1-structure apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-404-passes-through apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-500-passes-through apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-round-trips-path-query-headers apigateway__lambda.xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden
Issue-Key: apigateway__lambda.apigw-binary-media-types-round-trip-raw-bytes|_poll_http#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.apigw-lambda-proxy-unhandled-exception-bad-gateway|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.apigw-mock-integration-serves-cors-preflight|_poll_http#24|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-request-body-bytes|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-response-body-bytes|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-convert-to-binary-rejects-malformed-base64-with-500|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-repeated-override-keeps-the-first-value-instead-of-failing|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-request-override-header-and-querystring-reach-backend|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-response-override-status-turns-backend-200-into-400|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-undeclared-backend-header-dropped-declared-header-mapped|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-when-no-templates-rejects-unmapped-content-type-with-415|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigw-integration-rest-aws-proxy-event-carries-the-v1-structure|_poll#17|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-404-passes-through|_await#3|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-500-passes-through|_await#3|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-round-trips-path-query-headers|_await#5|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden|_await#5|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Commit: c8d80ea15f1454d07f388eb2b932b3a5430349f4

# API Gateway + Lambda: 16 integration checks never complete — the stage/HTTP_PROXY invoke path never returns a response

> _Note: this issue was identified against the code base at commit `c8d80ea15f1454d07f388eb2b932b3a5430349f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Cause 1 of 1: an invocation that has to reach a Lambda backend through API Gateway never produces an HTTP response.
AWS: a request to a deployed stage whose method has an AWS_PROXY, non-proxy AWS, MOCK or HTTP_PROXY (Lambda function URL) integration always returns an HTTP response — 200 with the mapped body for a working integration, 502 for a malformed proxy response or an unhandled handler exception, 504 when the integration times out, 415 for an unmapped content type, 500 for malformed base64 under CONVERT_TO_BINARY, and the function URL's own status (404/500/403) when proxied through.
Emulator: all 16 of these checks run to the harness deadline and are killed with 'Error: Check timed out after 60.0s'. No status line, no headers and no body are ever produced, so none of the downstream behaviour (binary media types, content handling, request/response overrides, header mapping, the v1 proxy event structure, CORS preflight from a MOCK integration, function-URL passthrough and its unsigned-IAM refusal) can be observed at all.
Why: every client that calls an API Gateway method backed by Lambda through the emulator blocks until its own timeout and cannot distinguish a hang from a slow backend; none of the proxy/non-proxy mapping semantics are reachable.
Fix: the execute-api invoke pipeline's request dispatch and response completion for Lambda-backed integrations (AWS_PROXY, AWS non-proxy and HTTP_PROXY to a function URL), including the path that must still answer when the backend cannot be run.
Traces: all 16 check ids listed in trace_ids.

Note: the open issues 20261023T170254Z-apigateway-lambda-three-integration-behaviour-checks-never-complete.md, 20261023T210711Z-apigateway-lambda-four-integration-authorizer-behaviour-checks-hang.md and 20261024T043741Z-apigateway-lambda-the-stage-variable-integration-check-hangs.md describe the same hang for other checks of this surface; they do not carry any of the 16 replay keys below.

## Evidence
Replay output for every one of the 16 checks, verbatim: 'Error: Check timed out after 60.0s'.
No HTTP status, header set or body was returned by the emulator in any of them, so the expected values could not even be compared. For contrast, the same emulator's TestInvokeMethod path reports the underlying cause explicitly in its execution log on sibling checks: 'Execution failed due to configuration error: the AWS_PROXY integration names a backend this emulator cannot call: no deployment package runs in process and no outbound connection is made, so no backend answer exists to map' — on the execute-api path no answer is produced at all.

## Steps to Reproduce
1. Create a REST API with a resource and a method; attach in turn an AWS_PROXY integration to a Lambda function, a non-proxy AWS integration with contentHandling/request-and-response overrides and declared header mappings, a MOCK OPTIONS integration for CORS preflight, and an HTTP_PROXY integration whose uri is a Lambda function URL.
2. Create a deployment and a stage.
3. Issue an HTTPS request to the stage URL for each method. Each call never returns; the suite kills the check at 60s.
4. Replay the 16 check ids in trace_ids.
