Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cli-cloudfront-invalidate-cache-after-s3-update apigateway__cloudfront__s3.cloudfront-apigw-origin-positive apigateway__cloudfront__s3.cloudfront-apigw-origin-missing-resource-policy-negative apigateway__cloudfront__s3.cloudfront-invalidation-s3-object-update-positive apigateway__cloudfront__s3.cloudfront-s3-access-logging-delivery-positive
Issue-Key: apigateway__cloudfront__s3.cli-cloudfront-invalidate-cache-after-s3-update|_poll_until_ok#24|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-origin-positive|_poll_get#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-origin-missing-resource-policy-negative|_poll_get#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-invalidation-s3-object-update-positive|_poll_http_conn#18|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-access-logging-delivery-positive|run#23|AssertionError|assert status == 200 and fetched == body, f"origin fetch failed while generating log traffic: {status}"
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# apigateway__cloudfront__s3: five more CloudFront/API-Gateway edge checks fail on the data plane — four hang to the 60 s cap and one origin fetch answers 404 (2 causes)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Two root causes. Open issue 20261023T210711Z-apigateway-cloudfront-s3-every-cloudfront-s3-integration-check.md describes the same two symptom families but does not carry any of these five check keys, so they are unowned and filed here.

Cause 1 of 2: a request through a CloudFront distribution (or a poll for the distribution/invalidation to converge) never produces a response.
AWS: a distribution whose custom origin is an API Gateway REST API stage, or whose origin is an S3 bucket, reaches Status=Deployed and then serves every GET with an HTTP response: the origin's own bytes on a miss, the cached bytes on a hit, and the fresh bytes after a CreateInvalidation reaches Completed. Recorded behavior only for the per-case bodies; what is structural is that each request and each polled status terminates.
Emulator: the four checks report exactly 'Check timed out after 60.0s' — no HTTP status line and no terminal distribution/invalidation status ever arrives, so the cache-hit/miss, invalidation and resource-policy assertions are never reached.
Why: a client cannot serve or invalidate anything through the edge, and a hang stalls the caller rather than failing it, so nothing distinguishes a cold cache from a dead router.
Fix: the CloudFront edge request path and the distribution/invalidation status state machines, together with the execute-api data-plane dispatch the apigw-origin checks depend on; every request and every polled status must terminate.
Traces: apigateway__cloudfront__s3.cli-cloudfront-invalidate-cache-after-s3-update, apigateway__cloudfront__s3.cloudfront-apigw-origin-positive, apigateway__cloudfront__s3.cloudfront-apigw-origin-missing-resource-policy-negative, apigateway__cloudfront__s3.cloudfront-invalidation-s3-object-update-positive.

Cause 2 of 2: the origin fetch used to generate access-log traffic answers HTTP 404 instead of the object.
AWS: a GET of an object that was just put behind the distribution returns HTTP 200 with that object's exact bytes, and the request is then recorded in the configured access-log bucket.
Emulator: the same GET answers HTTP 404, so no log traffic is ever generated and the delivery assertion cannot run.
Why: the 404 is not a CloudFront/S3 response to a request for an object that exists; a client sees the object as missing immediately after a successful PUT, and access-log delivery is unobservable.
Fix: the edge/origin request path that resolves a distribution request to its S3 origin object (and the front-door host routing that must hand that hostname to it rather than to the generic AWS-API resolver).
Traces: apigateway__cloudfront__s3.cloudfront-s3-access-logging-delivery-positive.

## Evidence
Cause 1: 'Error: Check timed out after 60.0s' for all four checks — no status, no body, no terminal status.
Cause 2: AssertionError: origin fetch failed while generating log traffic: 404 (expected status == 200 and the fetched body == the uploaded body).

## Steps to Reproduce
1. Cause 1: create an S3 bucket (or a REST API with an AWS_PROXY/Lambda integration and a deployed stage), create_distribution with that origin, poll get_distribution for Status=Deployed, then GET the distribution domain over HTTPS; also create_invalidation and poll it. Observe that the poll/GET never returns within 60 s.
2. Cause 2: put_object behind the distribution, configure access logging to a second bucket, then GET the object through the distribution -> observe HTTP 404 instead of 200 with the object bytes.
3. Or replay the five check ids in trace_ids.
