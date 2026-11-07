Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-request-body-bytes apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-response-body-bytes apigateway__lambda.deep-apigateway-2-apigw-nonproxy-convert-to-binary-rejects-malformed-base64-with-500 apigateway__lambda.deep-apigateway-2-apigw-nonproxy-repeated-override-keeps-the-first-value-instead-of-failing apigateway__lambda.deep-apigateway-2-apigw-nonproxy-request-override-header-and-querystring-reach-backend apigateway__lambda.deep-apigateway-2-apigw-nonproxy-response-override-status-turns-backend-200-into-400 apigateway__lambda.deep-apigateway-2-apigw-nonproxy-undeclared-backend-header-dropped-declared-header-mapped apigateway__lambda.deep-apigateway-2-apigw-nonproxy-when-no-templates-rejects-unmapped-content-type-with-415 apigateway__lambda.deep-apigw-integration-stage-variable-selects-lambda-ungranted-function-rejected apigateway__lambda.xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-request-body-bytes|_poll_http#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-response-body-bytes|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-convert-to-binary-rejects-malformed-base64-with-500|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-repeated-override-keeps-the-first-value-instead-of-failing|_poll_http#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-request-override-header-and-querystring-reach-backend|_poll_http#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-response-override-status-turns-backend-200-into-400|_poll_http#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-undeclared-backend-header-dropped-declared-header-mapped|_poll_http#9|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-when-no-templates-rejects-unmapped-content-type-with-415|_poll_http#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigw-integration-stage-variable-selects-lambda-ungranted-function-rejected|run#17|AssertionError|assert r_headers.get("x-amzn-errortype") == "InternalServerErrorException", (
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden|run#11|AssertionError|assert json.loads(body) == {"Message": "Forbidden"}, f"proxied refusal body was {body!r}"
Issue-Commit: 49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4

# apigateway__lambda: 3 net-new causes — non-proxy HTTP integrations never answer (8 checks hang), the ungranted stage-variable 500 carries no x-amzn-errortype, and an AWS_IAM function URL's unsigned refusal body is emulator-authored

> _Note: this issue was identified against the code base at commit `49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Three net-new root causes in the apigateway__lambda surface, grounded in ten failing behavioral checks. A fixer must close ALL THREE before submitting.

=== Cause 1 of 3: a deployed stage with a non-proxy `HTTP` integration never produces a response — the whole non-proxy request/response transformation surface is unobservable ===

AWS: `Integration$type` = `HTTP` (non-proxy) means API Gateway transforms the request and the response rather than passing them through. Smithy (services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model): `Integration$contentHandling` targets `ContentHandlingStrategy` (values CONVERT_TO_BINARY | CONVERT_TO_TEXT) and documents "If this property is not defined, the request payload will be passed through from the method request to integration request without modification"; `Integration$passthroughBehavior` documents WHEN_NO_MATCH / WHEN_NO_TEMPLATES / NEVER and states that with NEVER (or WHEN_NO_TEMPLATES with a template defined) "the method request of an unmapped content-type will be rejected with an HTTP 415 Unsupported Media Type response"; `Integration$requestParameters` documents the `method.request.{location}.{name}` → integration parameter mapping (and static `'quoted'` values), and `IntegrationResponse$responseParameters` the response-header mapping, so an undeclared backend header is dropped while a declared one is mapped. The concrete contracts the eight checks assert: CONVERT_TO_BINARY base64-decodes a text request body before the backend sees it and is a no-op on an already-binary body; CONVERT_TO_TEXT base64-encodes a binary body and is a no-op on already-text; an unset contentHandling forwards the body byte-identical; a CONVERT_TO_BINARY body that is not valid base64 is answered 500; `#set($context.requestOverride.header.* / .querystring.*)` overrides reach the backend, a repeated override keeps the first value instead of failing, `$context.responseOverride.status` turns a backend 200 into a 400; an unmapped Content-Type with no template is answered 415.

Emulator: all eight checks are killed at the harness's 60 s cap (`Check timed out after 60.0s`). The deployed stage answers nothing — the first probe of the invoke URL (a plain POST to the resource whose integration has contentHandling unset) never returns, so not one of the transformations above can even be attempted.

Why: a client using the classic non-proxy integration style (mapping templates, parameter overrides, content handling, passthroughBehavior) gets no response at all: requests hang instead of being transformed, refused with 415, or answered with the overridden status. Every binary-media and VTL-override workflow is unusable, and because the call never returns, a client cannot even fall back to error handling.

Fix: the execute-api data-plane integration dispatcher for `type=HTTP` (non-proxy) — it must complete the hop to the integration endpoint and answer, and then apply contentHandling, requestParameters/requestTemplates, $context.requestOverride/$context.responseOverride and passthroughBehavior as modeled.

Traces (8 checks, same error): apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-request-body-bytes; …-content-handling-converts-response-body-bytes; …-convert-to-binary-rejects-malformed-base64-with-500; …-repeated-override-keeps-the-first-value-instead-of-failing; …-request-override-header-and-querystring-reach-backend; …-response-override-status-turns-backend-200-into-400; …-undeclared-backend-header-dropped-declared-header-mapped; …-when-no-templates-rejects-unmapped-content-type-with-415. Related but NOT owning this cause: open issues 20261103T031416Z-apigateway-lambda-http-proxy-integrations-pointed-at-a.md (HTTP_PROXY, not non-proxy HTTP) and 20261023T210711Z-apigateway-lambda-four-integration-authorizer-behaviour-checks-hang.md; the replay reported these eight checks as unowned.

=== Cause 2 of 3: the 500 served when a stage variable selects a Lambda the gateway may not invoke carries no `x-amzn-errortype` response header ===

AWS: when an AWS_PROXY integration URI resolves through a stage variable to a function whose resource policy carries no `lambda:InvokeFunction` grant for apigateway.amazonaws.com, the deployed stage answers HTTP 500 with body `{"message": "Internal server error"}` AND the response header `x-amzn-errortype: InternalServerErrorException`; the ungranted function is not invoked (no log group is created for it). Recorded behavior only for the header (the execute-api data plane is not in the control-plane Smithy model); the status, body and header above are the recorded refusal. The fixed text here is the error-type token `InternalServerErrorException` and the body sentence `Internal server error`; nothing in them comes from the request.

Emulator: the status (500) and the body are produced, but the response carries NO `x-amzn-errortype` header at all — the check fails with `AssertionError: expected InternalServerErrorException, got None`.

Why: SDKs and gateways key their retry/classification logic on `x-amzn-errortype`; with the header absent, a client cannot tell an API Gateway-generated 500 from a backend-generated one, and botocore/AWS SDK error-type dispatch for execute-api responses degrades to an unmodeled error.

Fix: the execute-api data-plane error serializer — every gateway-generated refusal/error response must set `x-amzn-errortype` to the error's type token (here `InternalServerErrorException`) alongside the status and the JSON body.

Traces: apigateway__lambda.deep-apigw-integration-stage-variable-selects-lambda-ungranted-function-rejected.

=== Cause 3 of 3: an unsigned request to an AWS_IAM Lambda function URL is refused with an emulator-authored body instead of AWS's `{"Message":"Forbidden"}` ===

AWS: a Lambda function URL configured with `AuthType=AWS_IAM` refuses a request that carries no SigV4 signature with HTTP 403 and the body `{"Message":"Forbidden"}` — JSON with the single member `Message` capitalised, value `Forbidden` — and no function-generated response headers (the function does not run). The same refusal is what an API Gateway HTTP_PROXY integration pointed at that function URL forwards to its caller. Recorded behavior only (the function-URL data plane is not in the Smithy model); the status and body above are the recorded refusal, and both are entirely fixed text with no request- or state-derived parts.

Emulator: the status is right (403) but the body is `{"__type":"AccessDenied","message":"Sign the request with the emulator credentials"}` — a different envelope (`__type`/`message` instead of `Message`), a different error code (`AccessDenied` instead of none/Forbidden) and prose (`Sign the request with the emulator credentials`) that is the emulator's own authorship and names the emulator in a response a client parses.

Why: a client that matches the documented `{"Message":"Forbidden"}` refusal (directly or proxied through an API Gateway stage) does not recognise the emulator's body, so unauthenticated-access handling and negative tests both break; and the message leaks emulator-specific wording into a response surface AWS defines.

Fix: the Lambda function-URL auth refusal serializer (the AWS_IAM unsigned/invalid-signature path) — answer 403 with exactly `{"Message":"Forbidden"}` and no function headers, and make the same bytes the body an HTTP_PROXY integration forwards.

Traces: apigateway__lambda.xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden (the 403 status assertion passed; the body assertion failed). Related but NOT owning this cause: open issues 20261023T114649Z-lambda-function-urls-unrouted-configuration-blocks-dropped-from.md and 20261104T114029Z-lambda-function-url-a-percent-escaped-request-path.md; neither carries a key for the AWS_IAM refusal body.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), service heading `### apigateway__lambda`, 10 failing checks not owned by any open issue:

Cause 1 (8 checks, identical error `Check timed out after 60.0s`):
  [mismatch] deep-apigateway-2-apigw-nonproxy-content-handling-converts-request-body-bytes
  [mismatch] deep-apigateway-2-apigw-nonproxy-content-handling-converts-response-body-bytes
  [mismatch] deep-apigateway-2-apigw-nonproxy-convert-to-binary-rejects-malformed-base64-with-500
  [mismatch] deep-apigateway-2-apigw-nonproxy-repeated-override-keeps-the-first-value-instead-of-failing
  [mismatch] deep-apigateway-2-apigw-nonproxy-request-override-header-and-querystring-reach-backend
  [mismatch] deep-apigateway-2-apigw-nonproxy-response-override-status-turns-backend-200-into-400
  [mismatch] deep-apigateway-2-apigw-nonproxy-undeclared-backend-header-dropped-declared-header-mapped
  [mismatch] deep-apigateway-2-apigw-nonproxy-when-no-templates-rejects-unmapped-content-type-with-415
  Each check does PutMethod(authorizationType=NONE) + PutIntegration(type="HTTP", integrationHttpMethod="POST", uri=<echo backend>/echo[, contentHandling=CONVERT_TO_BINARY|CONVERT_TO_TEXT]) + PutMethodResponse/PutIntegrationResponse, CreateDeployment, then POSTs/GETs the stage invoke URL.
  Expected: a response per the transformation under test (e.g. CONVERT_TO_BINARY: the backend hashes the base64-DECODED bytes; unset twin: the base64 text byte-identical; malformed base64: 500; no template + unmapped Content-Type: 415; responseOverride.status: 400). Actual: the first probe of the deployed stage never returns and the check is killed at 60 s — no status, no body.

Cause 2:
  [mismatch] deep-apigw-integration-stage-variable-selects-lambda-ungranted-function-rejected
  AssertionError: expected InternalServerErrorException, got None
  (assert r_headers.get("x-amzn-errortype") == "InternalServerErrorException")
  Expected: 500 + body {"message": "Internal server error"} + header x-amzn-errortype: InternalServerErrorException. Actual: 500 and the body, but the header is absent (None).

Cause 3:
  [mismatch] xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden
  AssertionError: proxied refusal body was '{"__type":"AccessDenied","message":"Sign the request with the emulator credentials"}'
  (assert json.loads(body) == {"Message": "Forbidden"}; the preceding `status == 403` assertion passed)
  Expected: 403 with body {"Message": "Forbidden"} and no x-echo-backend header. Actual: 403 with the emulator-authored AccessDenied envelope above.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite().
2. Cause 1: in the report find `### apigateway__lambda` and replay any of the eight `deep-apigateway-2-apigw-nonproxy-*` checks. By hand: create a REST API with binaryMediaTypes=["application/octet-stream"], a child resource, PutMethod POST authorizationType=NONE, PutIntegration type="HTTP" integrationHttpMethod="POST" uri=<an echo HTTP backend> (optionally contentHandling=CONVERT_TO_BINARY), PutMethodResponse/PutIntegrationResponse for 200, CreateDeployment, then POST the stage invoke URL. AWS answers 200 with the transformed body; the emulator never answers.
3. Cause 2: replay deep-apigw-integration-stage-variable-selects-lambda-ungranted-function-rejected. By hand: AWS_PROXY integration whose uri interpolates a stage variable naming a Lambda with NO apigateway invoke grant; GET that stage's path and inspect the response headers of the 500 — AWS sets x-amzn-errortype: InternalServerErrorException, the emulator sets none.
4. Cause 3: replay xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden. By hand: CreateFunctionUrlConfig(AuthType="AWS_IAM") on a function, then GET the function URL with no SigV4 signature (and again through an API Gateway HTTP_PROXY integration pointed at it). AWS answers 403 {"Message":"Forbidden"}; the emulator answers 403 {"__type":"AccessDenied","message":"Sign the request with the emulator credentials"}.
