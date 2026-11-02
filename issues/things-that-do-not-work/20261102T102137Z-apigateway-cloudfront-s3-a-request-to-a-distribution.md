Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-missing-oac-negative apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-oac-stack-positive apigateway__cloudfront__s3.cloudfront-s3-signed-url-restricted-access-positive apigateway__cloudfront__s3.cloudfront-s3-website-endpoint-origin-positive
Issue-Key: apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-missing-oac-negative|run#12|AssertionError|assert status == 403, f"expected 403 with no bucket policy granted, got {status}: {content!r}"
Issue-Key: apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-oac-stack-positive|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-signed-url-restricted-access-positive|_poll_http_get#31|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-website-endpoint-origin-positive|_poll_http_get#18|Timeout|time.sleep(interval_s)
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# apigateway__cloudfront__s3: a request to a distribution domain is answered 404 UnknownService by the emulator's service router, and three more CloudFront/S3 edge checks never get a response

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDistribution

## Description
Two net-new root causes. Open issues 20261023T210711Z-apigateway-cloudfront-s3-every-cloudfront-s3-integration-check.md and 20261029T175823Z-apigateway-cloudfront-s3-29-more-cloudfront-api-gateway.md describe the same family for other checks but do NOT carry these four replay keys.

Cause 1 of 2: a viewer request to a distribution domain is answered by the emulator's AWS-API router instead of the edge.
AWS: a GET to a deployed distribution's DomainName whose S3 origin has an OAC but no matching bucket-policy statement is answered 403 (the origin refuses the signed read and CloudFront relays the refusal); adding the statement makes the same GET answer 200 with the object.
Emulator: the request is answered 404 with the body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} — the front door tried to resolve the request as an AWS API call, so the edge was never consulted. The status (404), the __type and that message are not CloudFront responses at all; a viewer request carries no SigV4 scope or X-Amz-Target and must be dispatched on the distribution domain.
Why: a client fetching content through a distribution gets an emulator-internal routing error instead of the origin's 403/200, so neither the negative (missing OAC grant) nor the positive (grant added) path of an OAC-protected origin can be distinguished — and the 404 masks a security-relevant 403.
Fix: front-door request dispatch — route non-API HTTP requests whose Host is a distribution domain to the cloudfront edge handler instead of the UnknownService fallback.
Traces: apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-missing-oac-negative.

Cause 2 of 2: three more CloudFront/S3 edge paths never produce any response.
AWS: a CFN-deployed distribution with an OAC over an S3 origin serves the object at its domain; a distribution with a trusted key group answers 403 for an unsigned URL and 200 for a correctly signed one; a distribution over an S3 WEBSITE endpoint origin serves the website's index document. Each check polls the distribution domain for that response.
Emulator: no response is ever obtained — all three end at the 60 s per-check budget ("Error: Check timed out after 60.0s").
Why: the deployed estate reports success at the control plane while nothing is served, so signed-URL restriction and website-endpoint origin behavior are unobservable.
Fix: the CloudFront edge data plane (distribution-domain routing, S3 and S3-website origin fetch, signed-URL/trusted-key-group evaluation) in the cloudfront service.
Traces: apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-oac-stack-positive, apigateway__cloudfront__s3.cloudfront-s3-signed-url-restricted-access-positive, apigateway__cloudfront__s3.cloudfront-s3-website-endpoint-origin-positive.

## Evidence
Cause 1: "AssertionError: expected 403 with no bucket policy granted, got 404: '{\"__type\":\"UnknownService\",\"message\":\"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)\"}'".
Cause 2: apigateway__cloudfront__s3.cfn-deploy-cloudfront-s3-oac-stack-positive, apigateway__cloudfront__s3.cloudfront-s3-signed-url-restricted-access-positive and apigateway__cloudfront__s3.cloudfront-s3-website-endpoint-origin-positive each fail with "Error: Check timed out after 60.0s" while polling the distribution domain for a response.

## Steps to Reproduce
1. Cause 1: deploy (via CloudFormation or the API) a bucket with an object, an OAC and a distribution over that S3 origin, WITHOUT a bucket-policy statement granting the distribution; GET https://<distribution domain>/<key> -> expect 403; observed 404 with the UnknownService body.
2. Add the granting bucket-policy statement and repeat -> expect 200 with the object bytes.
3. Cause 2: with the grant in place (or with a trusted-key-group / S3-website-endpoint origin), GET the distribution domain -> expect 200 / 403-then-200 for signed URLs / the website index document; observed no response before the 60 s budget ends.
