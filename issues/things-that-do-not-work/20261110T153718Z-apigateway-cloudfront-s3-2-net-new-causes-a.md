Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive
Issue-Key: apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative|_http_get#18|Timeout|with urllib.request.urlopen(req, timeout=timeout, context=_https_context(verify)) as resp:
Issue-Key: apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive|_poll_http_get#18|Timeout|time.sleep(interval_s)
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# apigateway__cloudfront__s3: 2 net-new causes — a CloudFront custom origin over an API Gateway REST API never delivers the API's response, and a legacy-OAI S3 origin never serves its object

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject (CloudFront edge origin fetch) / CreateDistribution

## Description
Two net-new root causes in the apigateway__cloudfront__s3 suite. Neither is covered by an open issue: the open issues for this heading (20261110T092303Z-apigateway-cloudfront-s3-5-net-new-causes-at.md, 20261104T220426Z-apigateway-cloudfront-s3-5-net-new-root-causes.md, 20261109T152222Z-apigateway-cloudfront-s3-a-distribution-s-domainname-is.md, 20261029T175823Z-apigateway-cloudfront-s3-29-more-cloudfront-api-gateway.md, 20261109T181319Z-apigateway-cloudfront-s3-createdistribution-s-invalidviewercertificate-refusal-does.md) carry other checks' replay keys; these two checks carry none.

Cause 1 of 2: a CloudFront distribution with two cache behaviors over the same API Gateway REST API — one via an https-only custom origin, one via an http-only custom origin — produces neither the API's response on the https leg nor an origin-connect failure on the http leg.
AWS: recorded behavior only (OriginProtocolPolicy's effect is not modelled as a wire rule). A custom origin whose OriginProtocolPolicy is https-only connects to the API Gateway execute-api domain over TLS and the edge answers HTTP 200 with the integration's JSON body. A custom origin whose policy is http-only connects over plaintext HTTP, which API Gateway never serves, so the origin request never yields an HTTP response and CloudFront relays HTTP 502 (Bad Gateway) or 504 (Gateway Timeout) to the viewer — never the API's body.
Emulator: the check is reported "Error: Check timed out after 60.0s" with no assertion reached. Its positive control polls the https-only path for 240 s waiting for a 200 whose JSON carries origin_label "protocol-mismatch", and the negative polls the http-only path for 90 s waiting for 502/504; neither predicate is ever satisfied, so the harness kills the check and no status or body is ever compared.
Why: a client cannot put CloudFront in front of an API Gateway REST API at all, and the one behavioural difference OriginProtocolPolicy exists to express — a downgraded origin connection failing closed rather than silently succeeding — cannot be observed, so a misconfigured origin is indistinguishable from a working one.
Fix: the edge's custom-origin request path — it must honour OriginProtocolPolicy when connecting to the origin, relay the origin's response for https-only, and surface a failed/refused plaintext connection to the viewer as 502/504.
Traces: apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative.

Cause 2 of 2: a distribution whose S3 origin uses a legacy origin access identity, with the OAI's canonical user granted read in the bucket policy, never serves the object.
AWS: recorded behavior only. CreateDistributionWithTags over an S3OriginConfig whose OriginAccessIdentity is "origin-access-identity/cloudfront/<id>" reaches Status Deployed, and once the bucket policy grants s3:GetObject to the OAI's canonical ARN the edge answers HTTP 200 with the object's exact bytes for a viewer GET of that key.
Emulator: the check is reported "Error: Check timed out after 60.0s" with no assertion reached — it waits up to 1500 s for the distribution to report Deployed and then polls the edge for 120 s for a 200, and neither is reached inside the run budget.
Why: the whole legacy OAI access path — still the configuration of record for a large installed base of CloudFront+S3 estates — serves nothing, and a client migrating from OAI to OAC has no working baseline to compare against.
Fix: the distribution lifecycle (Status must reach Deployed) and the edge's S3-origin fetch under an origin access identity, including the S3 bucket-policy evaluation for the OAI canonical user.
Traces: apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive.

## Evidence
Cause 1 — apigateway__cloudfront__s3.cloudfront-apigw-origin-protocol-mismatch-negative, verdict [mismatch]: "Error: Check timed out after 60.0s", no assertion reached. The check's own predicates were: https leg — status == 200 and json.loads(body)["origin_label"] == "protocol-mismatch", polled 240 s at 10 s; http leg — status in (502, 504), polled 90 s at 5 s. Neither was ever satisfied.

Cause 2 — apigateway__cloudfront__s3.cloudfront-s3-oai-legacy-access-positive, verdict [mismatch]: "Error: Check timed out after 60.0s", no assertion reached. The check creates a private bucket, puts index.txt, creates an OAI, creates the distribution with S3OriginConfig.OriginAccessIdentity = "origin-access-identity/cloudfront/<oai id>" and DefaultCacheBehavior CachePolicyId 4135ea2d-6df8-44a3-9df3-4b5a84be39ad (CachingDisabled), installs the OAI bucket policy, then _wait_distribution_deployed(timeout_s=1500.0) followed by _poll_http_get(edge_url, lambda s, b: s == 200, timeout_s=120.0).

Expected in both: an edge response within the deployment/propagation window — 200 with the API's JSON body (and 502/504 on the downgraded origin) for cause 1, 200 with index.txt's exact bytes for cause 2. Actual: no edge response ever matches, so nothing was compared. Distribution ids, OAI ids, domains, bucket names and cache-buster query values are per-run values.

## Steps to Reproduce
Cause 1: create a Lambda-proxy REST API and deploy a stage; CreateDistribution with two custom origins on the same execute-api domain and OriginPath /<stage>, one OriginProtocolPolicy https-only (default behavior) and one http-only (behavior path pattern "broken/*"); wait for Deployed; GET /probe at the distribution domain (expect 200 with the integration's JSON) and GET /broken/probe (expect 502 or 504).
Cause 2: create a private bucket and put index.txt; CreateCloudFrontOriginAccessIdentity; CreateDistributionWithTags over <bucket>.s3.<region>.amazonaws.com with S3OriginConfig.OriginAccessIdentity set to that OAI; PutBucketPolicy granting s3:GetObject to the OAI's canonical ARN; wait for Deployed; GET /index.txt at the distribution domain and compare the bytes.
Re-replay the two checks in trace_ids to confirm the fix.
