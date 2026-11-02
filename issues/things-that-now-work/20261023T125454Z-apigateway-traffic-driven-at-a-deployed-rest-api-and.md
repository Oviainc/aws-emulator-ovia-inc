Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.deep-apigateway-3-apigw-private-vpclink-positive apigateway.deploy-apigateway-1-cfn-deployment-without-method-rejected apigateway.deploy-apigateway-2-update-rollback-keeps-serving-good-deployment apigateway.deploy-rest-api-cfn-behaves apigateway__cloudformation__lambda.cfn-custom-resource-signals-failure-stack-rollback-negative apigateway__cloudfront__s3.cloudfront-apigw-cache-behavior-path-routing apigateway__cloudfront__s3.cloudfront-apigw-custom-header-forward-positive apigateway__cloudfront__s3.cloudfront-apigw-origin-missing-resource-policy-negative apigateway__cloudfront__s3.cloudfront-apigw-origin-positive apigateway__cloudfront__s3.cloudfront-apigw-viewer-request-header-passthrough apigateway__cloudfront__s3.deploy-apigateway-role-trust-policy-positive apigateway__cloudfront__s3.deploy-apigateway-s3-integration-stack-positive apigateway__cloudfront__s3.deploy-s3-role-trust-policy-positive apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cfn apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-sam apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-stage-metrics-count-errors-latency-under-apiname-stage apigateway__cloudwatch__lambda.obs-apigateway-1-5xx-alarm-ok-to-alarm-notifies-sqs-via-sns apigateway__cognito-idp__lambda.cfn-apigw-lambda-proxy-integration-resource-binding-positive apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage apigateway__dynamodb__lambda.apigw-lambda-proxy-malformed-response-bad-gateway apigateway__dynamodb__lambda.apigw-proxy-invoke-denied-without-lambda-resource-policy-for-that-path apigateway__dynamodb__lambda.apigw-stage-variables-isolate-ddb-target-per-stage apigateway__dynamodb__lambda__logs.deploy-full-stack-apigateway-lambda-dynamodb-logs apigateway__dynamodb__lambda__logs.iam-role-wrong-principal-service-negative apigateway__lambda.apigw-binary-media-types-round-trip-raw-bytes apigateway__lambda.apigw-lambda-proxy-unhandled-exception-bad-gateway apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-request-body-bytes apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-response-body-bytes apigateway__lambda.deep-apigateway-2-apigw-nonproxy-convert-to-binary-rejects-malformed-base64-with-500 apigateway__lambda.deep-apigateway-2-apigw-nonproxy-repeated-override-keeps-the-first-value-instead-of-failing apigateway__lambda.deep-apigateway-2-apigw-nonproxy-request-override-header-and-querystring-reach-backend apigateway__lambda.deep-apigateway-2-apigw-nonproxy-response-override-status-turns-backend-200-into-400 apigateway__lambda.deep-apigateway-2-apigw-nonproxy-undeclared-backend-header-dropped-declared-header-mapped apigateway__lambda.deep-apigateway-2-apigw-nonproxy-when-no-templates-rejects-unmapped-content-type-with-415 apigateway__lambda.deep-apigw-integration-rest-aws-proxy-event-carries-the-v1-structure apigateway__lambda.deep-apigw-integration-stage-variable-selects-lambda-ungranted-function-rejected apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-404-passes-through apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-500-passes-through apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-round-trips-path-query-headers apigateway__lambda.xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden apigateway__lambda__logs__resourcegroupstaggingapi__sqs.api-key-discovered-tagged-and-removed-from-index-on-delete apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive
Issue-Key: apigateway.deep-apigateway-3-apigw-private-vpclink-positive|_retry_dependency#1|Timeout|time.sleep(15)
Issue-Key: apigateway.deploy-apigateway-1-cfn-deployment-without-method-rejected|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.deploy-apigateway-2-update-rollback-keeps-serving-good-deployment|_poll_http#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.deploy-rest-api-cfn-behaves|_poll_http#7|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudformation__lambda.cfn-custom-resource-signals-failure-stack-rollback-negative|run#16|Timeout|time.sleep(5.0)
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-cache-behavior-path-routing|_poll_get#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-custom-header-forward-positive|_poll_get#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-origin-missing-resource-policy-negative|_poll_get#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-origin-positive|_poll_get#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-viewer-request-header-passthrough|_poll_get#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.deploy-apigateway-role-trust-policy-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.deploy-apigateway-s3-integration-stack-positive|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.deploy-s3-role-trust-policy-positive|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree|_post_order#5|Timeout|time.sleep(5.0)
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cfn|_post_order#5|Timeout|time.sleep(5.0)
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-sam|_post_order#5|Timeout|time.sleep(5.0)
Issue-Key: apigateway__cloudwatch__lambda.metrics-elbv2-apigw-rest-api-stage-metrics-count-errors-latency-under-apiname-stage|_ready_then_align#7|Timeout|time.sleep(0.5)
Issue-Key: apigateway__cloudwatch__lambda.obs-apigateway-1-5xx-alarm-ok-to-alarm-notifies-sqs-via-sns|_drain#16|Timeout|resp = sqs.receive_message(QueueUrl=q_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: apigateway__cognito-idp__lambda.cfn-apigw-lambda-proxy-integration-resource-binding-positive|_poll_invoke_until#6|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage|_await#5|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-proxy-malformed-response-bad-gateway|_poll_http#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda.apigw-proxy-invoke-denied-without-lambda-resource-policy-for-that-path|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda.apigw-stage-variables-isolate-ddb-target-per-stage|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-full-stack-apigateway-lambda-dynamodb-logs|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda__logs.iam-role-wrong-principal-service-negative|_retry_negative_update#14|Timeout|time.sleep(3.0)
Issue-Key: apigateway__lambda.apigw-binary-media-types-round-trip-raw-bytes|_poll_http#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.apigw-lambda-proxy-unhandled-exception-bad-gateway|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-request-body-bytes|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-content-handling-converts-response-body-bytes|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-convert-to-binary-rejects-malformed-base64-with-500|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-repeated-override-keeps-the-first-value-instead-of-failing|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-request-override-header-and-querystring-reach-backend|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-response-override-status-turns-backend-200-into-400|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-undeclared-backend-header-dropped-declared-header-mapped|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigateway-2-apigw-nonproxy-when-no-templates-rejects-unmapped-content-type-with-415|_poll_http#2|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigw-integration-rest-aws-proxy-event-carries-the-v1-structure|_poll#17|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigw-integration-stage-variable-selects-lambda-ungranted-function-rejected|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-404-passes-through|_await#3|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-500-passes-through|_await#3|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-function-url-round-trips-path-query-headers|_await#5|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden|_await#5|Timeout|time.sleep(interval_s if streak == 0 else 1.0)
Issue-Key: apigateway__lambda__logs__resourcegroupstaggingapi__sqs.api-key-discovered-tagged-and-removed-from-index-on-delete|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive|run#8|AssertionError|assert status == 200, f"expected Allow decision to reach the integration, got {status}: {body!r}"
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Traffic driven at a deployed REST API (and at a CloudFront distribution fronting one) never completes: 41 checks hang to the 60 s budget, and one authorized invoke answers 404 UnknownService (2 causes)

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two root causes on the API Gateway data path. Neither is marked ALREADY FILED or OWNED in the replay report.

Cause 1 of 2 — a request sent to a deployed stage's invoke URL (or to a CloudFront distribution whose origin is one) never produces a response: the check reaches its 60 s budget with no HTTP status line at all.
AWS: recorded behavior only — a deployed REST API stage answers every request on its invoke URL with a complete HTTP response (200 for a wired integration, 403 with no base-path mapping, 502 for a malformed Lambda proxy response, 415/504 for the mapping and timeout cases). A CloudFront distribution fronting it likewise reaches Deployed and answers. A response is always produced; the check never waits on AWS.
Emulator: all 41 checks below stop at 'Check timed out after 60.0s' — the request (or the convergence poll that precedes it: a stage that becomes callable, a distribution that becomes Deployed) never completes. A hang is the emulator's: nothing answered on the address it published.
Why: every data-plane behavior of REST APIs is unobservable — integration request/response mapping, content handling, binary media types, proxy event shape, stage variables, authorizer caching, HTTP-proxy pass-through, access/execution logging and stage metrics. A client cannot distinguish 'slow' from 'never', so tooling that polls these endpoints blocks instead of failing.
Fix: the execute-api data-plane listener and the stage/distribution convergence it depends on — it must always produce a response (even an error one) rather than leaving the connection open.
Traces: the 41 timed-out check ids in trace_ids below (apigateway.deep-apigateway-3-apigw-private-vpclink-positive through apigateway__lambda__logs__resource-groups-tagging-api__sqs.api-key-discovered-tagged-and-removed-from-index-on-delete).

Cause 2 of 2 — an invoke that does get a response gets 404 UnknownService from the generic request router instead of the stage's integration result.
AWS: recorded behavior only — a request to https://<api-id>.execute-api.<region>.amazonaws.com/<stage>/<path> is routed to the stage; with an Allow authorizer decision the integration runs and its status is returned.
Emulator: HTTP 404 with body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} — the execute-api hostname is not recognised as a data-plane route, so the control-plane router answers instead.
Why: an API that the control plane reports as deployed is not served; the 404 also masks the authorizer decision the check is asserting.
Fix: host-based routing for execute-api endpoints in the request router, ahead of the service-resolution fallback. Open issue 20261023T114649Z-apigateway-api-gateway-execute-api-stage-endpoints-are-not.md describes this same routing gap and owns sibling checks, but it does not carry this check's replay key.
Traces: apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive.

## Evidence
Cause 1: 'Error: Check timed out after 60.0s' with no traceback frame, for 41 distinct checks spanning apigateway, apigateway__cloudfront__s3, apigateway__lambda, apigateway__dynamodb__lambda, apigateway__cloudwatch__lambda and apigateway__cloudwatch__dynamodb__lambda__logs__sqs.
Cause 2: AssertionError: expected Allow decision to reach the integration, got 404: b'{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'.

## Steps to Reproduce
1. Create a REST API with a MOCK or AWS_PROXY integration, CreateDeployment to a stage, then issue an HTTPS GET to the stage invoke URL — expect a complete HTTP response within seconds, observe the request never completing (re-run any of the 41 listed checks).
2. Put a CloudFront distribution in front of that stage and poll until Deployed, then GET through the distribution domain — same hang.
3. Deploy the apigw execution-role template (apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive) and invoke the stage path — expect the integration's status, observe 404 UnknownService.
