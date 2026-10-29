Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.apigateway-s3-role-trust-policy-missing-negative apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-oac-stack-positive apigateway__cloudfront__s3.cfn-stack-update-cloudfront-origin-positive apigateway__cloudfront__s3.cli-apigateway-deploy-stage-cloudfront-origin-workflow apigateway__cloudfront__s3.cli-cloudfront-create-distribution-s3-origin-workflow apigateway__cloudfront__s3.cli-cloudfront-missing-origin-access-negative apigateway__cloudfront__s3.cli-cloudfront-s3-oac-setup-workflow apigateway__cloudfront__s3.cli-s3-cors-cloudfront-preflight-workflow apigateway__cloudfront__s3.cli-s3-sync-cloudfront-invalidate-workflow apigateway__cloudfront__s3.cloudfront-apigw-cache-behavior-path-routing apigateway__cloudfront__s3.cloudfront-apigw-custom-header-forward-positive apigateway__cloudfront__s3.cloudfront-apigw-viewer-request-header-passthrough apigateway__cloudfront__s3.cloudfront-s3-bucket-policy-wrong-distribution-negative apigateway__cloudfront__s3.cloudfront-s3-missing-oac-policy-negative apigateway__cloudfront__s3.cloudfront-s3-oac-origin-access-positive apigateway__cloudfront__s3.cloudfront-s3-oac-origin-positive apigateway__cloudfront__s3.cloudfront-s3-origin-mismatched-bucket-region-negative apigateway__cloudfront__s3.cloudfront-s3-origin-missing-bucket-policy-negative apigateway__cloudfront__s3.cloudfront-s3-origin-path-prefix-resolution apigateway__cloudfront__s3.cloudfront-s3-public-bucket-direct-access-bypass apigateway__cloudfront__s3.cloudfront-s3-public-bucket-direct-access-negative apigateway__cloudfront__s3.cloudfront-s3-wrong-oac-signing-negative apigateway__cloudfront__s3.deploy-apigateway-role-trust-policy-positive apigateway__cloudfront__s3.deploy-apigateway-s3-integration-stack-positive apigateway__cloudfront__s3.deploy-s3-role-trust-policy-positive apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-missing-oac-negative apigateway__cloudfront__s3.cli-deploy-apigw-cloudfront-distribution-workflow apigateway__cloudfront__s3.cloudfront-apigateway-custom-origin-positive
Issue-Key: apigateway__cloudfront__s3.apigateway-s3-role-trust-policy-missing-negative|_poll_until_status#20|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cfn-deploy-cloudfront-apigw-stack-positive|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-oac-stack-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cfn-stack-update-cloudfront-origin-positive|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cli-apigateway-deploy-stage-cloudfront-origin-workflow|_poll_until_http_ok#15|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cli-cloudfront-create-distribution-s3-origin-workflow|_poll_until_http_ok#19|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cli-cloudfront-missing-origin-access-negative|_poll_until_stable_status#17|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cli-cloudfront-s3-oac-setup-workflow|_poll_until_http_ok#26|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cli-s3-cors-cloudfront-preflight-workflow|_poll_options#28|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cli-s3-sync-cloudfront-invalidate-workflow|_poll_sticky#29|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-cache-behavior-path-routing|_poll_get#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-custom-header-forward-positive|_poll_get#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-viewer-request-header-passthrough|_poll_get#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-bucket-policy-wrong-distribution-negative|_poll_http_get#21|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-missing-oac-policy-negative|_poll_http_get#19|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-oac-origin-access-positive|_poll_http_get#18|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-oac-origin-positive|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-origin-mismatched-bucket-region-negative|_poll_http#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-origin-missing-bucket-policy-negative|_poll_http#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-origin-path-prefix-resolution|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-public-bucket-direct-access-bypass|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-public-bucket-direct-access-negative|_poll_http_get#25|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-wrong-oac-signing-negative|_poll_http#15|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.deploy-apigateway-role-trust-policy-positive|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.deploy-apigateway-s3-integration-stack-positive|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.deploy-s3-role-trust-policy-positive|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-missing-oac-negative|run#11|AssertionError|assert status == 403, f"expected 403 with no bucket policy granted, got {status}: {content!r}"
Issue-Key: apigateway__cloudfront__s3.cli-deploy-apigw-cloudfront-distribution-workflow|run#27|AssertionError|assert status == 200, f"CloudFront did not return 200 from API Gateway origin: {status} {body!r}"
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigateway-custom-origin-positive|run#10|AssertionError|assert status == 200, f"expected 200 from CloudFront custom origin, got {status}: {body!r}"
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# apigateway__cloudfront__s3: 29 more CloudFront/API-Gateway edge checks have no data plane — 26 hang to the 60 s cap and 3 are answered 404 UnknownService

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution (CloudFront edge data plane / API Gateway invoke URL)

## Description
This issue bundles 2 net-new root causes in the apigateway__cloudfront__s3 behavioral group. Both are about the DATA plane: the control-plane calls (CreateRestApi/CreateDeployment, CreateDistribution, CreateOriginAccessControl, PutBucketPolicy, CreateRole) all succeed, and then nothing answers at the resulting edge/invoke host. Open issues 20261023T210711Z-apigateway-cloudfront-s3-every-cloudfront-s3-integration-check.md, 20261024T043741Z-apigateway-cloudfront-s3-five-more-cloudfront-api-gateway.md and 20261024T182555Z-apigateway-cloudfront-s3-a-deployed-rest-api-s.md describe the same surface but do not carry the replay keys below, so these checks are still unowned.

Cause 1 of 2: nothing listens on the distribution's edge domain or the REST API stage invoke URL, so every check that polls it burns its whole budget.
AWS: after CreateDistribution returns a DomainName (d<id>.cloudfront.net), and after CreateDeployment publishes a stage, an HTTPS GET/PUT/OPTIONS against that host is served by the edge/front door — it returns the origin's status and body (200 with the S3 object bytes, 301/302 for a viewer-protocol redirect, 403 AccessDenied as a CloudFront/S3 XML error body when the origin or bucket policy denies, 404 NoSuchKey for a missing key). Smithy: CreateDistribution -> DistributionConfig/DomainName and GetDistribution model the edge host but the model says nothing about the edge protocol, so: recorded behavior only — the recordings show an HTTP response arriving within seconds of Status=Deployed.
Emulator: no response ever arrives on that host. Each of the 26 checks below polls until the harness cap and the suite reports `Check timed out after 60.0s`. No request-id, status or body is produced at all.
Why: every end-to-end assertion a client can make about CloudFront or API Gateway is unreachable — OAC/OAI/bucket-policy enforcement, origin-path prefixing, cache-behavior path routing, custom-header forwarding, CORS preflight, invalidation-then-refetch, and all the negative controls that need a 403 from the edge. A client cannot distinguish "denied" from "nothing is running".
Fix: the CloudFront edge data-plane handler and the API Gateway stage invoke handler in the emulator's request router — the host-based front door must map d<id>.cloudfront.net and <api-id>.execute-api.<region>.amazonaws.com to the stored distribution/stage and fetch from the configured origin/integration.
Traces: apigateway__cloudfront__s3.apigateway-s3-role-trust-policy-missing-negative, .cfn-deploy-cloudfront-apigw-stack-positive, .cfn-deploy-cloudfront-s3-oac-stack-positive, .cfn-stack-update-cloudfront-origin-positive, .cli-apigateway-deploy-stage-cloudfront-origin-workflow, .cli-cloudfront-create-distribution-s3-origin-workflow, .cli-cloudfront-missing-origin-access-negative, .cli-cloudfront-s3-oac-setup-workflow, .cli-s3-cors-cloudfront-preflight-workflow, .cli-s3-sync-cloudfront-invalidate-workflow, .cloudfront-apigw-cache-behavior-path-routing, .cloudfront-apigw-custom-header-forward-positive, .cloudfront-apigw-viewer-request-header-passthrough, .cloudfront-s3-bucket-policy-wrong-distribution-negative, .cloudfront-s3-missing-oac-policy-negative, .cloudfront-s3-oac-origin-access-positive, .cloudfront-s3-oac-origin-positive, .cloudfront-s3-origin-mismatched-bucket-region-negative, .cloudfront-s3-origin-missing-bucket-policy-negative, .cloudfront-s3-origin-path-prefix-resolution, .cloudfront-s3-public-bucket-direct-access-bypass, .cloudfront-s3-public-bucket-direct-access-negative, .cloudfront-s3-wrong-oac-signing-negative, .deploy-apigateway-role-trust-policy-positive, .deploy-apigateway-s3-integration-stack-positive, .deploy-s3-role-trust-policy-positive.

Cause 2 of 2: an edge request that DOES reach the emulator is answered by the AWS-API front door as an unroutable API call — 404 with a JSON UnknownService envelope.
AWS: a viewer request to the distribution domain is never answered with an AWS JSON error envelope. It is answered by the edge: 200 with the origin body on the positive path, and 403 with CloudFront's/S3's XML AccessDenied body when the S3 bucket policy grants nothing to the distribution. The status and the error body shape are the requirement; the host, distribution id and object key in them come from the request.
Emulator: HTTP 404 with body `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}`. The fixed text of that message is the emulator's own router text; it contains no value from the request. The request was dispatched to the generic AWS-protocol router, which has no host-to-distribution mapping and no SigV4 credential scope to fall back on, so it reports "no target service" instead of serving the edge.
Why: every negative control inverts. A check expecting 403 for a missing bucket policy gets 404; a check expecting 200 from an API Gateway origin gets 404 with a JSON body a CloudFront client will never parse. A client sees "this host is not an AWS endpoint" where AWS says "your request was denied" or "here is your object".
Fix: the emulator's host-based request router (the fallback that emits UnknownService) must recognise CloudFront edge hosts and execute-api hosts before falling through to the AWS-protocol dispatcher; the edge handler then owns the status and the XML error body.
Traces: apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-missing-oac-negative (expected 403, got 404 UnknownService), .cli-deploy-apigw-cloudfront-distribution-workflow (expected 200 from the API Gateway origin, got 404 UnknownService), .cloudfront-apigateway-custom-origin-positive (expected 200 from the custom origin, got 404 UnknownService).

## Evidence
Cause 2, verbatim from the replay:
  apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-missing-oac-negative
  AssertionError: expected 403 with no bucket policy granted, got 404: '{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}'
  apigateway__cloudfront__s3.cli-deploy-apigw-cloudfront-distribution-workflow
  AssertionError: CloudFront did not return 200 from API Gateway origin: 404 '{"__type":"UnknownService",...}'
  apigateway__cloudfront__s3.cloudfront-apigateway-custom-origin-positive
  AssertionError: expected 200 from CloudFront custom origin, got 404: b'{"__type":"UnknownService",...}'

Expected: 403 + S3/CloudFront XML AccessDenied body (fixed text as S3 words AccessDenied; the bucket/key values come from the request) for the missing-bucket-policy case; 200 + the origin's bytes for the two positive cases.
Actual: 404 + the emulator router's JSON UnknownService envelope.

Cause 1, verbatim: `Error: Check timed out after 60.0s` for all 26 listed checks. The control-plane setup in each (CreateBucket/PutObject/CreateRole/CreateRestApi/CreateDeployment/CreateDistribution) returned 200/201 first, so the failure is confined to the edge fetch.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and run the behavioral suite (service group apigateway__cloudfront__s3).
2. Cause 2: create an S3 bucket with one object, create a distribution with that bucket as origin and NO bucket policy granting the distribution, wait for Status=Deployed, then HTTP GET https://<DomainName>/<key>. Observe 404 and the JSON UnknownService body instead of 403 AccessDenied.
3. Cause 1: do the same with a bucket policy that DOES grant the distribution (or with an API Gateway REST API deployed to a stage as the origin) and poll the edge domain / invoke URL. Observe that no response ever arrives and the check reaches the 60 s cap.
