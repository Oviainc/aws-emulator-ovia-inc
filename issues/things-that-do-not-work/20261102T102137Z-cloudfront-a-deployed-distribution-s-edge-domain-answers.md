Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.cf-compression-serves-gzip-only-when-accept-encoding-present cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected cloudfront.deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-request-rewrite cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-response-header cloudfront.deploy-cloudfront-distribution-serves-origin-content cloudfront.e2e-invalidation-clears-cached-content cloudfront.server-side-origin-access-control-signs-requests cloudfront.server-side-security-headers-policy-enforced-on-response
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.cf-compression-serves-gzip-only-when-accept-encoding-present|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected|_poll_until#30|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-request-rewrite|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-response-header|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deploy-cloudfront-distribution-serves-origin-content|_fetch_with_retries#10|Timeout|time.sleep(2)
Issue-Key: cloudfront.e2e-invalidation-clears-cached-content|_fetch_with_retries#9|Timeout|time.sleep(2)
Issue-Key: cloudfront.server-side-origin-access-control-signs-requests|_fetch#21|Timeout|time.sleep(2)
Issue-Key: cloudfront.server-side-security-headers-policy-enforced-on-response|_fetch#11|Timeout|time.sleep(2)
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudfront: a Deployed distribution's edge domain answers nothing, so compression, edge functions, response-headers policy, OAC origin signing and invalidation refresh are all unreachable (9 checks hang to the cap)

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDistribution

## Description
Cause 1 of 1: the CloudFront data plane never answers at a distribution's DomainName.
AWS: once GetDistribution reports Status=Deployed, an HTTP/HTTPS request to the distribution's DomainName is served by the edge — it fetches from the configured origin, applies the cache behavior (compression when Accept-Encoding: gzip is present and Compress is on), runs any associated CloudFront function (viewer-request URI rewrite, viewer-response header injection, a thrown error surfacing as 503), applies the response-headers policy, SigV4-signs the origin request when an OAC is attached, and re-fetches after a CreateInvalidation completes. Every one of these checks polls that domain for a response.
Emulator: no response is ever obtained from the distribution domain; each check polls until the 60 s per-check budget ends ("Error: Check timed out after 60.0s"). No status, no body, no headers — the edge is simply not served.
Why: every behavior a CDN exists for is unobservable. A client cannot fetch content through a distribution it just created, cannot see cache hits/misses, cannot verify a function association or a security-headers policy, and cannot confirm that an invalidation refreshed anything. Control-plane calls succeed while the service does nothing, which is the most misleading possible failure mode for an emulator.
Fix: the CloudFront edge data plane — request routing from a distribution DomainName to a per-distribution edge handler, origin fetch, cache-behavior/cache-key evaluation and response assembly — alongside the cloudfront distribution state. Open issues naming the same absence for other check sets (20261027T001538Z-cloudfront-the-edge-data-plane-serves-nothing-so.md, 20261029T175823Z-cloudfront-10-more-divergences-the-edge-data-plane.md) do NOT carry the nine replay keys below.
Traces: the nine failing check ids listed in trace_ids.

## Evidence
All nine checks fail with "Error: Check timed out after 60.0s" while polling the distribution's DomainName for any response:
- cloudfront.cf-compression-serves-gzip-only-when-accept-encoding-present (expects a gzip-encoded body only when Accept-Encoding: gzip is sent)
- cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-request-rewrite (expects the rewritten URI's object)
- cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-response-header (expects the function-added response header)
- cloudfront.deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected (expects 503 from a throwing function)
- cloudfront.deep-cloudfront-1-cf-edge-fn-lambda-edge-unqualified-arn-rejected (expects the post-rejection edge probe to answer)
- cloudfront.deploy-cloudfront-distribution-serves-origin-content (expects 200 with the origin's bytes)
- cloudfront.e2e-invalidation-clears-cached-content (expects the refreshed object after the invalidation completes)
- cloudfront.server-side-origin-access-control-signs-requests (expects 200 through the distribution while a direct unsigned S3 GET is 403)
- cloudfront.server-side-security-headers-policy-enforced-on-response (expects X-Content-Type-Options: nosniff, X-Frame-Options: DENY, Strict-Transport-Security: max-age=31536000)

## Steps to Reproduce
1. CreateDistribution over any reachable origin and poll GetDistribution until Status=Deployed.
2. Issue a plain GET to the returned DomainName (http and https) for an object that exists at the origin.
3. Expect an HTTP response from the edge (200 with the origin bytes, plus the cache-behavior/function/policy effects). Observed: no response at all — the request never completes, and the check ends at the 60 s budget.
