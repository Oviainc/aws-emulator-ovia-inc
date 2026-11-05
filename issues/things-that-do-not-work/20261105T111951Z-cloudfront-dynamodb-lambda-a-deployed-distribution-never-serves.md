Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-brotli-enabled-cache-policy-serves-br-and-keys-on-encoding cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-cache-hit-leaves-origin-invocation-count-unchanged cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-edge-generates-viewer-country-and-forwarded-proto cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-path-prefixes-the-path-the-origin-sees cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-query-string-allexcept-and-all-shape-the-cache-key cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-request-policy-forwards-what-the-cache-key-excludes cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-unlisted-header-cookie-query-blocked-from-origin cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-viewer-spoofed-edge-headers-rejected-at-the-edge
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-brotli-enabled-cache-policy-serves-br-and-keys-on-encoding|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-cache-hit-leaves-origin-invocation-count-unchanged|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-edge-generates-viewer-country-and-forwarded-proto|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-path-prefixes-the-path-the-origin-sees|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-query-string-allexcept-and-all-shape-the-cache-key|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-request-policy-forwards-what-the-cache-key-excludes|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-unlisted-header-cookie-query-blocked-from-origin|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-viewer-spoofed-edge-headers-rejected-at-the-edge|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# cloudfront__dynamodb__lambda: a deployed distribution never serves its origin, so all eight edge/cache-behaviour checks poll to their deadline

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1 — a distribution created over a Lambda origin never answers at its edge domain, so none of the cache-key, origin-request, compression, origin-path or edge-header behaviours can be observed.
AWS: CreateDistribution returns a DomainName; once the distribution is Deployed, a GET on that domain is served through the configured cache behaviour — the cache policy decides the cache key (query strings all/allExcept, headers, cookies, Accept-Encoding for br/gzip), the origin request policy decides what reaches the origin, OriginPath prefixes the path the origin sees, the edge adds CloudFront-Viewer-Country and X-Forwarded-Proto and strips client-supplied CloudFront-* headers, and a cache hit leaves the origin invocation count unchanged.
Emulator: recorded behavior only — each check never observes a response that satisfies its predicate and the harness reports "Check timed out after 60.0s".
Why: no CloudFront client can fetch anything through a distribution this emulator created, so caching, header forwarding and edge-spoofing protection are all unverifiable. Open issues 20261102T102137Z-cloudfront-lambda-a-deployed-distribution-over-a-lambda.md and 20261103T062355Z-... record a related symptom for the cloudfront__lambda group; they do not carry these replay keys.
Fix: the CloudFront data plane — routing a request on the distribution's edge domain to the configured origin, and the cache-key/origin-request/header-manipulation stages in front of it.
Traces: all eight check ids in trace_ids.

## Evidence
Error: Check timed out after 60.0s for every deep-cloudfront-3-cf-origin-* check. Expected: a GET on the distribution's DomainName is served (HTTP 200 from the origin or a cached response) within the window, with the cache-key, forwarded-header and origin-path behaviours observable.

## Steps to Reproduce
CreateDistribution with a Lambda function-URL origin and a cache policy/origin request policy; wait for Status=Deployed; GET https://<DomainName>/<path> with Accept-Encoding: br, with query strings, with a spoofed CloudFront-Viewer-Country header, and twice for a cache hit.
