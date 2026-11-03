Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Commit: 39390cfa3000e408bc0d1db242197466d1e66569

# cloudfront__lambda: a Deployed distribution's edge domain answers nothing, so GeoRestriction blacklist/whitelist enforcement at the edge is never observable

> _Note: this issue was identified against the code base at commit `39390cfa3000e408bc0d1db242197466d1e66569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDistribution

## Description
Cause 1 of 1: the distribution's edge domain produces no response at all, so CloudFront's geo-restriction enforcement (blacklist -> CloudFront-generated 403, whitelist -> served again from the origin) can never be exercised. Failing check: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked.

AWS: once GetDistribution reports Status=Deployed, a GET to the distribution's DomainName is answered by the edge, which fetches from the configured origin (here an OAC-signed Lambda function URL) and returns 200 with the origin's body; with an OriginRequestPolicy forwarding CloudFront-Viewer-Country, the origin sees that header carrying a two-letter ISO 3166-1 alpha-2 code, and the request carries an x-amz-cf-id header proving CloudFront was the caller. Smithy (com.amazonaws.cloudfront): DistributionConfig.Restrictions -> Restrictions.GeoRestriction (required) with GeoRestriction.RestrictionType (required, enum GeoRestrictionType = blacklist|whitelist|none), Quantity (required) and Items (LocationList of two-letter uppercase country codes); the model documents blacklist as 'the countries in which you don't want CloudFront to distribute your content' and whitelist as 'the countries in which you want CloudFront to distribute your content'. Recorded behavior for the data plane: with RestrictionType=blacklist containing the viewer's country, every request to the edge domain is refused by CloudFront itself with HTTP status 403 and the response header X-Cache: "Error from cloudfront" (that header value is fixed text; the status 403 and the CloudFront-generated error body are the requirement), and the body contains none of the origin's bytes — no echo JSON, no x-amz-cf-id, and none of the per-request values the origin would echo. Switching the same country to RestrictionType=whitelist makes the edge serve 200 from the origin again with a fresh (uncached, CachingDisabled) origin response.

Emulator: no response is ever obtained from the distribution domain. The check ends at the per-check budget with "Check timed out after 60.0s" while polling for the baseline 200 through the edge, before the blacklist beat is even reached; the control plane meanwhile accepts CreateDistribution/UpdateDistribution and reports a healthy distribution, and the stored Restrictions round-trip.

Why: a client that relies on CloudFront for geo blocking gets no enforcement and no content: the distribution reports itself Deployed and stores the geo restriction, but the edge serves neither the allowed 200 nor the CloudFront-generated 403, so both the happy path and the refusal a security control depends on are silently absent. A caller cannot distinguish 'blocked by geo restriction' from 'nothing is listening'.

Fix: the CloudFront edge data plane in the cloudfront service — distribution-domain routing to an edge request handler, the OAC-signed Lambda function-URL origin fetch, CloudFront-Viewer-Country generation plus origin-request-policy forwarding, and GeoRestriction evaluation producing CloudFront's own 403 (X-Cache: Error from cloudfront) without contacting the origin. Open issues 20261102T102137Z-cloudfront-lambda-a-deployed-distribution-over-a-lambda.md, 20261027T001538Z-cloudfront-lambda-the-edge-enforces-nothing-because-it.md and 20261029T080904Z-cloudfront-lambda-a-deployed-distribution-s-edge-domain.md describe the same silent edge for AllowedMethods and signed-URL checks, but none of them carries this check's replay key or its geo-restriction cause.

Traces: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked.

## Evidence
Expected: (1) GET https://<distribution domain>/?nonce=<n> -> 200, body is the origin's JSON echo carrying this run's marker, the request's nonce in the echoed query string, the CloudFront-Viewer-Country two-letter code, and x-amz-cf-id among the header names; (2) after UpdateDistribution sets Restrictions.GeoRestriction = {RestrictionType: blacklist, Quantity: 1, Items: [<viewer country>]}, the same GET -> HTTP 403 with response header X-Cache: "Error from cloudfront" and a CloudFront-generated body containing no origin bytes (no marker, no nonce, no x-amz-cf-id, no "header_names"); (3) after switching the same country to whitelist, the GET -> 200 again from the origin with the fresh nonce.
Actual: no HTTP response is ever obtained from the distribution's edge domain. Suite output for cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked: "Error: Check timed out after 60.0s" — the check never gets past its pre-restriction baseline probe (the check marks such failures DATAPLANE_UNIMPLEMENTED), so neither the geo 403 nor the whitelist 200 is ever produced.

## Steps to Reproduce
1. Create a Lambda function with a function URL (AuthType AWS_IAM) that echoes the CloudFront-Viewer-Country header, the raw query string and the header names it saw.
2. Create an OriginAccessControl (OriginAccessControlOriginType=lambda, SigningBehavior=always, SigningProtocol=sigv4) and a distribution whose single origin is that function URL host, with CachePolicy CachingDisabled, an OriginRequestPolicy that forwards all query strings and CloudFront-Viewer-Country, ViewerProtocolPolicy https-only, and Restrictions.GeoRestriction = {RestrictionType: none, Quantity: 0}.
3. Poll GetDistribution until Status=Deployed.
4. GET https://<Distribution.DomainName>/?nonce=<unique> -> expect 200 with the origin's echo, including a two-letter CloudFront-Viewer-Country and x-amz-cf-id in the forwarded header names. Observed: the request never yields a response; the check ends at its 60 s budget.
5. UpdateDistribution setting Restrictions.GeoRestriction = {RestrictionType: blacklist, Quantity: 1, Items: [<country from step 4>]}, then repeat the GET -> expect HTTP 403 with X-Cache: "Error from cloudfront" and no origin bytes in the body.
6. UpdateDistribution setting the same country as whitelist, repeat the GET -> expect 200 from the origin with the fresh nonce.
