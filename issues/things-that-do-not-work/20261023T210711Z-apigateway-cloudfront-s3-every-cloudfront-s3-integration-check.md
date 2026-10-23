Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.apigateway-s3-binary-media-type-passthrough apigateway__cloudfront__s3.apigateway-s3-getobject-proxy-integration-positive apigateway__cloudfront__s3.apigateway-s3-missing-role-permission-negative apigateway__cloudfront__s3.apigateway-s3-putobject-proxy-integration-positive apigateway__cloudfront__s3.apigw-role-trust-policy-validation apigateway__cloudfront__s3.apigw-s3-direct-integration-getobject-positive apigateway__cloudfront__s3.apigw-s3-direct-integration-putobject-positive apigateway__cloudfront__s3.apigw-s3-integration-missing-role-permission-negative apigateway__cloudfront__s3.apigw-s3-integration-wrong-bucket-arn-negative apigateway__cloudfront__s3.cli-apigateway-deploy-stage-cloudfront-origin-workflow apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-positive apigateway__cloudfront__s3.cloudfront-s3-website-endpoint-origin-positive apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative apigateway__cloudfront__s3.s3-object-created-eventbridge-cloudfront-invalidation-positive apigateway__cloudfront__s3.apigateway-s3-role-trust-policy-missing-negative apigateway__cloudfront__s3.cli-deploy-apigw-cloudfront-distribution-workflow apigateway__cloudfront__s3.cloudfront-apigateway-cache-behavior-forward-positive apigateway__cloudfront__s3.cloudfront-apigateway-custom-header-passthrough apigateway__cloudfront__s3.cloudfront-apigateway-custom-origin-positive apigateway__cloudfront__s3.cloudfront-apigateway-origin-missing-resource-negative apigateway__cloudfront__s3.cloudfront-apigateway-viewer-protocol-policy-positive apigateway__cloudfront__s3.cloudfront-apigateway-wrong-origin-path-negative
Issue-Key: apigateway__cloudfront__s3.apigateway-s3-binary-media-type-passthrough|_poll_until_status#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.apigateway-s3-getobject-proxy-integration-positive|_poll_until_status#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.apigateway-s3-missing-role-permission-negative|run#10|Timeout|time.sleep(3)
Issue-Key: apigateway__cloudfront__s3.apigateway-s3-putobject-proxy-integration-positive|_poll_until_status#6|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.apigw-role-trust-policy-validation|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.apigw-s3-direct-integration-getobject-positive|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.apigw-s3-direct-integration-putobject-positive|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.apigw-s3-integration-missing-role-permission-negative|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.apigw-s3-integration-wrong-bucket-arn-negative|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cli-apigateway-deploy-stage-cloudfront-origin-workflow|_poll_until_http_ok#6|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative|_poll_get#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive|_poll_http_get#17|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-positive|_poll_http#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-website-endpoint-origin-positive|_poll_http_get#17|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative|run#13|Timeout|time.sleep(5)
Issue-Key: apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative|run#14|Timeout|time.sleep(5)
Issue-Key: apigateway__cloudfront__s3.s3-object-created-eventbridge-cloudfront-invalidation-positive|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.apigateway-s3-role-trust-policy-missing-negative|run#17|AssertionError|assert statuses[-1] == 500, f"expected 500 with an untrusted role, got statuses={statuses}"
Issue-Key: apigateway__cloudfront__s3.cli-deploy-apigw-cloudfront-distribution-workflow|run#15|AssertionError|assert status == 200, f"API Gateway itself did not respond 200: {status} {body!r}"
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigateway-cache-behavior-forward-positive|run#6|AssertionError|assert status1 == 200, f"expected 200, got {status1}: {body1!r}"
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigateway-custom-header-passthrough|run#7|AssertionError|assert status == 200, f"expected 200, got {status}: {body!r}"
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigateway-custom-origin-positive|run#3|AssertionError|assert direct_status == 200
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigateway-origin-missing-resource-negative|run#7|AssertionError|assert status == 200, f"positive control failed: expected 200, got {status}: {body!r}"
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigateway-viewer-protocol-policy-positive|run#6|AssertionError|assert redirect_status in (301, 302), f"expected a redirect, got {redirect_status}"
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigateway-wrong-origin-path-negative|run#6|AssertionError|assert status == 403, f"expected 403 for a mismatched origin path, got {status}: {body!r}"
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__cloudfront__s3: every CloudFront/S3 integration check fails on the API Gateway data plane — 17 hang to the 60 s cap and 8 get 404 UnknownService from the deployed stage (2 causes)

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Two root causes, both in the API Gateway data plane as reached from the apigateway__cloudfront__s3 scenarios. Open issues 20261023T140640Z-apigateway-cloudfront-s3-deploy-paths-stack-outputs-carry.md and 20261023T170254Z-apigateway-cloudfront-s3-four-cloudfront-s3-eventbridge-checks.md cover other causes in this service and do not carry the 25 replay keys below.

Cause 1 of 2: a request to a deployed REST API stage (directly, or through a CloudFront distribution whose origin is that stage, or at an S3 proxy integration) never returns.
AWS: a deployed stage with an AWS-service proxy integration to S3 serves GetObject/PutObject through the stage URL and returns the object bytes (200), the configured binary passthrough, or the modeled refusal when the execution role lacks the s3 permission / names the wrong bucket ARN (403/500). A CloudFront distribution fronting that stage returns the origin's response, a redirect for a viewer-protocol-policy of redirect-to-https, or 403 for a mismatched origin path.
Emulator: no response arrives. 17 of these checks exhaust the harness budget and are reported as "Check timed out after 60.0s".
Why: the whole API-Gateway-to-S3 and CloudFront-to-API-Gateway edge is unobservable, and a hang stalls the caller instead of failing it.
Fix: the execute-api data-plane request path (stage/route resolution and integration dispatch) — it must always produce a response.
Traces: the 17 timing-out ids in trace_ids, apigateway__cloudfront__s3.apigateway-s3-binary-media-type-passthrough ... apigateway__cloudfront__s3.s3-object-created-eventbridge-cloudfront-invalidation-positive.

Cause 2 of 2: when the deployed stage does answer, it answers 404 UnknownService.
AWS: the stage invoke URL (and the CloudFront distribution in front of it) returns the integration's response — 200 for the positive legs, a redirect for redirect-to-https, 403 for a mismatched origin path or a role with no trust/permission.
Emulator: HTTP 404 with body `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}`. The request reaches the emulator's generic AWS-API front door rather than the API Gateway data plane, so the error code `UnknownService` — which API Gateway never emits — is returned for every leg, positive and negative alike.
Why: both the positive and the negative legs collapse onto the same 404, so a client cannot tell a working origin from a denied one, and CloudFront behaviours (cache-behavior forwarding, custom-header passthrough, viewer-protocol redirect, origin-path matching) cannot be observed at all.
Fix: the emulator's front-door request routing — bind the execute-api stage endpoint to the API Gateway data plane.
Traces: apigateway__cloudfront__s3.apigateway-s3-role-trust-policy-missing-negative, apigateway__cloudfront__s3.cli-deploy-apigw-cloudfront-distribution-workflow, apigateway__cloudfront__s3.cloudfront-apigateway-cache-behavior-forward-positive, apigateway__cloudfront__s3.cloudfront-apigateway-custom-header-passthrough, apigateway__cloudfront__s3.cloudfront-apigateway-custom-origin-positive, apigateway__cloudfront__s3.cloudfront-apigateway-origin-missing-resource-negative, apigateway__cloudfront__s3.cloudfront-apigateway-viewer-protocol-policy-positive, apigateway__cloudfront__s3.cloudfront-apigateway-wrong-origin-path-negative.

## Evidence
Cause 1: detail for all 17 is exactly "Error: Check timed out after 60.0s".
Cause 2: "AssertionError: expected 200, got 404: b'{\"__type\":\"UnknownService\",\"message\":\"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)\"}'" (cloudfront-apigateway-cache-behavior-forward-positive, cloudfront-apigateway-custom-header-passthrough); "AssertionError: API Gateway itself did not respond 200: 404 '{\"__type\":\"UnknownService\", ...}'" (cli-deploy-apigw-cloudfront-distribution-workflow); "AssertionError: expected a redirect, got 404" (cloudfront-apigateway-viewer-protocol-policy-positive); "AssertionError: expected 403 for a mismatched origin path, got 404: b'{\"__type\":\"UnknownService\", ...}'" (cloudfront-apigateway-wrong-origin-path-negative); "AssertionError: expected 500 with an untrusted role, got statuses=[404, 404, 404, 404, 404, 404]" (apigateway-s3-role-trust-policy-missing-negative).

## Steps to Reproduce
1. Create an S3 bucket and a REST API with an AWS-service proxy integration to s3:GetObject/PutObject, deploy it to a stage.
2. Request the object through the stage invoke URL — observe either no response within 60 s (cause 1) or HTTP 404 with the UnknownService body (cause 2).
3. Create a CloudFront distribution whose custom origin is that stage and request through the distribution domain — same two outcomes.
4. Or replay the check ids listed in trace_ids.
