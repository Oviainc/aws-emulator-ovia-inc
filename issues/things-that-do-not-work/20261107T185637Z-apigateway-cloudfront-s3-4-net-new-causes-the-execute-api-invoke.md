Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive apigateway__cloudfront__s3.deploy-apigateway-role-trust-policy-positive apigateway__cloudfront__s3.deploy-apigateway-s3-integration-stack-positive apigateway__cloudfront__s3.cli-cloudfront-missing-origin-access-negative apigateway__cloudfront__s3.cloudfront-invalidation-s3-object-update-positive apigateway__cloudfront__s3.cloudfront-s3-missing-oac-policy-negative apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive apigateway__cloudfront__s3.apigw-role-trust-policy-validation apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative apigateway__cloudfront__s3.s3-object-created-eventbridge-cloudfront-invalidation-positive
Issue-Key: apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive|_poll_until#5|AssertionError|raise AssertionError(
Issue-Key: apigateway__cloudfront__s3.deploy-apigateway-role-trust-policy-positive|_poll_until#10|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigateway__cloudfront__s3.deploy-apigateway-s3-integration-stack-positive|_poll_until#9|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigateway__cloudfront__s3.cli-cloudfront-missing-origin-access-negative|_poll_until_stable_status#17|AssertionError|raise AssertionError(f"GET {url} never stabilized at status {expected_status}; last={last}")
Issue-Key: apigateway__cloudfront__s3.cloudfront-invalidation-s3-object-update-positive|_poll_http_conn#18|AssertionError|raise AssertionError(f"polling {path} never satisfied predicate; last={last}")
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-missing-oac-policy-negative|_poll_http_get#19|AssertionError|raise AssertionError(f"polling {url} never satisfied the predicate within {timeout_s}s; last={status}: {body!r}")
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive|_poll_http_get#17|AssertionError|raise AssertionError(f"GET {url} never satisfied predicate within {timeout_s}s; last status={status}, body={body!r}")
Issue-Key: apigateway__cloudfront__s3.apigw-role-trust-policy-validation|_poll_until#14|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative|_poll_until#22|Timeout|raise AssertionError(
Issue-Key: apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative|_poll_until#21|Timeout|raise AssertionError(
Issue-Key: apigateway__cloudfront__s3.s3-object-created-eventbridge-cloudfront-invalidation-positive|_poll_until#12|Timeout|raise AssertionError(
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# 4 net-new causes: the execute-api invoke endpoint and the CloudFront edge are not served, S3-under-an-assumed-service-role 500s, and S3->EventBridge never converges

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Four distinct root causes, all grounded in failing checks of this suite that the report does NOT mark ALREADY FILED or OWNED. Related open issues 20261102T102137Z-apigateway-cloudfront-s3-a-request-to-a-distribution.md, 20261029T175823Z-apigateway-cloudfront-s3-29-more-cloudfront-api-gateway.md and 20261104T220426Z-apigateway-cloudfront-s3-5-net-new-root-causes.md own OTHER checks of this service; they do not carry the replay keys below.

Cause 1 of 4: the API Gateway stage invoke endpoint is not routed at all.
AWS: a deployed REST API answers on its stage invoke URL (https://{restApiId}.execute-api.{region}.amazonaws.com/{stage}/{resource}) and returns the integration's response — 200 with the integration body for a wired integration, or the API's own 403/404 when the resource or authorization does not permit it. recorded behavior only for the exact body; the recordings show a real integration response.
Emulator: every request to the invoke URL (and to the API Gateway S3 proxy integration) is answered 404 with content-type application/x-amz-json-1.1 and body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} and header x-amzn-errortype: UnknownService. The fixed text here is the whole message; nothing in it is request-derived. The front door is dispatching on SigV4 credential scope / X-Amz-Target / Host and has no execute-api host route, so the request never reaches an API Gateway handler.
Why: a client that deploys an API and calls its invoke URL — the only way to exercise an API — gets a JSON-1.1 UnknownService 404 that no AWS SDK or browser models, instead of the integration's answer. Every API-Gateway-fronted flow is unusable.
Fix: the emulator's request router / host-based endpoint resolution, plus the API Gateway execute-api data-plane handler for a deployed stage.
Traces: apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive ("direct API Gateway invoke URL responding not met within 60.0s"), apigateway__cloudfront__s3.deploy-apigateway-role-trust-policy-positive, apigateway__cloudfront__s3.deploy-apigateway-s3-integration-stack-positive ("PUT object via API Gateway S3 proxy not met within 90s. Last result: 404").

Cause 2 of 4: the CloudFront distribution domain is not routed either.
AWS: a GET of https://{distributionDomainName}/{key} is served by the edge: 200 with the origin object's bytes when the origin grants access, 403 AccessDenied when the S3 origin's bucket policy/OAC does not, 404 NoSuchKey when the key is absent.
Emulator: the same UnknownService 404 JSON body as Cause 1 is returned for every distribution-domain request, so neither the positive (200) nor the negative (403) outcome can ever be produced.
Why: a client cannot read anything through a distribution, and a negative test that should see 403 sees a 404 with a non-S3, non-CloudFront error document — so an authorization defect is indistinguishable from a routing one.
Fix: the emulator's host-based request router plus the CloudFront edge data-plane handler that fetches from the configured origin.
Traces: apigateway__cloudfront__s3.cli-cloudfront-missing-origin-access-negative, apigateway__cloudfront__s3.cloudfront-invalidation-s3-object-update-positive, apigateway__cloudfront__s3.cloudfront-s3-missing-oac-policy-negative, apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive.

Cause 3 of 4: S3 ListBucket under a session assumed for a service principal answers HTTP 500.
AWS: ListBucket (GET /?list-type=2) with credentials from AssumeRole on a role trusted by apigateway.amazonaws.com returns 200 with the listing, or 403 AccessDenied if the role's policy does not allow s3:ListBucket. A 5xx is never the answer to a well-formed, authorized request.
Emulator: returns 500 with body {"message":"Internal server error"} — a JSON body on an XML-protocol S3 operation, with no Code/Message/RequestId/HostId Error document. The whole string is fixed text; nothing is request-derived.
Why: the SDK surfaces a retryable server fault instead of a result or an AccessDenied, so a caller retries and then fails for the wrong reason; the error is also unparseable as an S3 Error.
Fix: the S3 ListObjectsV2/ListBucket handler's authorization path for assumed-role (service-principal) sessions, and the S3 error serializer so refusals are XML Error documents.
Traces: apigateway__cloudfront__s3.apigw-role-trust-policy-validation ("ListBucket via the apigateway.amazonaws.com-assumed role not met within 90.0s. Last result: (500, '{\"message\":\"Internal server error\"}')").

Cause 4 of 4: S3 ObjectCreated events are never published to EventBridge, so every rule/target assertion polls to the run budget.
AWS: with EventBridgeConfiguration set in PutBucketNotificationConfiguration, S3 emits an "Object Created" event with source aws.s3 onto the default bus; a matching rule then invokes its target (and a target whose resource policy does not admit the rule is not invoked).
Emulator: no event is ever observed; the three checks run to the 180 s check ceiling with "Error: Check timed out after 180.0s" and never reach their assertion, so both the positive delivery and the two negative (no-delivery) controls are unmeasurable.
Why: a client wiring S3 -> EventBridge -> target sees nothing happen, and a negative control proves nothing because absence cannot be told from the pipeline being dead.
Fix: the S3 notification dispatcher's EventBridge destination (publish of the Object Created/Object Deleted events onto the default bus) and the EventBridge rule-matching/target-invocation path for aws.s3 events.
Traces: apigateway__cloudfront__s3.s3-object-created-eventbridge-cloudfront-invalidation-positive, apigateway__cloudfront__s3.s3-eventbridge-missing-rule-target-permission-negative, apigateway__cloudfront__s3.s3-eventbridge-wrong-bucket-arn-pattern-negative.

## Evidence
Cause 1: expected the integration's response on the stage invoke URL; got 404 '{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}' (x-amzn-errortype: UnknownService).
Cause 2: expected 200 (object bytes) or 403 AccessDenied from the distribution domain; got the same UnknownService 404 JSON.
Cause 3: expected 200 listing or 403 AccessDenied XML; got 500 '{"message":"Internal server error"}'.
Cause 4: expected an aws.s3 'Object Created' event on the default bus and a target invocation; got no event at all — all three checks ended 'Error: Check timed out after 180.0s'.

## Steps to Reproduce
1) Replay the suite's apigateway__cloudfront__s3 group.
2) Cause 1: deploy a REST API with an S3 or mock integration, create a deployment/stage, then GET/PUT the stage invoke URL.
3) Cause 2: create a distribution over an S3 origin (OAC or OAI), put an object, then GET https://<distribution-domain>/<key>.
4) Cause 3: create a role trusted by apigateway.amazonaws.com with s3:ListBucket, AssumeRole, and call ListObjectsV2 with the vended credentials.
5) Cause 4: PutBucketNotificationConfiguration with EventBridgeConfiguration, create an EventBridge rule on source aws.s3, PutObject, and watch for the target invocation.
