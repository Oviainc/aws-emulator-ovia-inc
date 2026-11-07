Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.cfcache-cookie-allowlist-separates-cache-entries
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.cfcache-cookie-allowlist-separates-cache-entries|_poll_until#27|Timeout|raise AssertionError(f"timed out waiting for {description} after {timeout_s}s (last: {last!r})")
Issue-Commit: 9cfb70c8dd865d3301250e334464bd408152814b

# cloudfront: a cookie-allowlist cache policy is never observable at the edge — the distribution's DomainName serves nothing, so the check never reaches a verdict

> _Note: this issue was identified against the code base at commit `9cfb70c8dd865d3301250e334464bd408152814b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
One net-new root cause in the cloudfront crate.

Note on adjacent filed work: open issue 20261029T175823Z-cloudfront-10-more-divergences-the-edge-data-plane.md describes the same surface (its Cause 1 is "the distribution's edge domain answers nothing") and even names this check in its prose, but it does NOT carry this failure's replay key — it recorded the `_poll_until` frame under a 60 s cap, whereas this run fails with a bare `Check timed out after 180.0s` and no frame. The replay suite therefore reports this check as unowned, so it is filed here rather than left to be closed unrun.

Cause 1 of 1: a cache policy that allowlists a single cookie is never observable at the edge, because the deployed distribution's DomainName never serves a response.

AWS: a cache policy whose ParametersInCacheKeyAndForwardedToOrigin.CookiesConfig sets CookieBehavior to `whitelist` with CookieNames = [<the one cookie>] puts exactly the listed cookies into the cache key. Smithy: CachePolicyCookiesConfig.CookieBehavior (required, target CachePolicyCookieBehavior, enum values none | whitelist | allExcept | all), documented as "whitelist – Only the cookies in viewer requests that are listed in the CookieNames type are included in the cache key and in requests that CloudFront sends to the origin." Once GetDistribution reports Distribution.Status == "Deployed", an HTTP GET of the DomainName the create returned is served by the edge. The edge wire protocol itself is not modeled, so for the response details: recorded behavior only — the recording shows (a) a cold GET carrying the allowlisted cookie answers 200 with the origin object's exact bytes; (b) a repeat GET with the same cookie value answers from the edge cache with response header `X-Cache: Hit from cloudfront` and the same bytes; (c) after the origin object is rewritten, a GET differing ONLY in a cookie that is not on the allowlist still answers 200, the OLD bytes, and `X-Cache: Hit from cloudfront` (same cache entry); (d) a GET with a different value for the allowlisted cookie is a different cache key, goes to the origin and serves the NEW bytes, then becomes its own cached entry; and (e) the original cookie value's entry is still intact and still holds the old bytes. The fixed parts of that contract are the status 200, the header name `X-Cache` and its value `Hit from cloudfront`; the bytes, bucket name, cookie name/values, distribution id and domain are all values taken from the request and from the emulator's own state, never literals from one run.

Emulator: the check never reaches a verdict. Replay output is `Error: Check timed out after 180.0s`. Every control-plane call that precedes the first edge fetch is answered (CreateBucket with the region's LocationConstraint, PutBucketTagging, PutObject, CreateCachePolicy, CreateOriginAccessControl, CreateDistribution, TagResource, PutBucketPolicy), and then nothing: either GetDistribution never reports Status == "Deployed" or the returned DomainName never answers 200, so the check's bounded 180 s fetch loop and the suite's own budget both expire before any body or `X-Cache` header is seen. No status, no header and no bytes are ever produced to compare.

Why: a client that provisions a cache policy to separate cache entries by one cookie gets no way to confirm — or use — that separation. The whole point of the policy (an allowlisted cookie partitions the cache, an unlisted cookie does not) is unverifiable and unusable, and a caller cannot distinguish "the edge is enforcing my cache key" from "the edge is dead": both look like a request that never completes rather than a refusal it could branch on.

Fix: the CloudFront edge data-plane handler and the host-based router entry that maps the minted d<id>.cloudfront.net domain to the stored distribution, plus the cache-key composition that must read ParametersInCacheKeyAndForwardedToOrigin.CookiesConfig (CookieBehavior/CookieNames) when forming the key and emit the `X-Cache` hit/miss header; and, if the distribution never leaves its initial status, the distribution state's transition to Status == "Deployed" in the CreateDistribution/GetDistribution state.

Traces: cloudfront.cfcache-cookie-allowlist-separates-cache-entries.

## Evidence
Cause 1 — failing check cloudfront.cfcache-cookie-allowlist-separates-cache-entries, verdict [mismatch].

Emulator (actual): `Error: Check timed out after 180.0s`. No HTTP status, no response body and no `X-Cache` response header were ever obtained from the distribution's DomainName; the check's helpers (`_wait_distribution_deployed`, bounded at 900 s on GetDistribution Status; `_fetch`, bounded at 180 s polling for a 200) never satisfied their predicates, and the suite cut the check off at its 180 s cap. The check carries its own DATAPLANE_UNIMPLEMENTED marker for exactly this condition ("a failure caused by an edge data plane that serves no traffic at all"), which is what the timeout is standing in for.

AWS (expected), for a distribution whose DefaultCacheBehavior uses a cache policy with CookiesConfig = {CookieBehavior: "whitelist", Cookies: {Quantity: 1, Items: [<keyed cookie>]}} over an OAC-signed S3 origin:
  - GET https://<DomainName>/<key> with `Cookie: <keyed>=<value-A>`  -> 200, body = the origin object's current bytes.
  - the same GET repeated                                            -> 200, same bytes, `X-Cache: Hit from cloudfront`.
  - after the origin object is overwritten, GET with `Cookie: <keyed>=<value-A>; <unlisted>=<anything>` -> 200, the PRE-overwrite bytes, `X-Cache: Hit from cloudfront` (the unlisted cookie is not in the cache key).
  - GET with `Cookie: <keyed>=<value-B>`                             -> 200, the POST-overwrite bytes (different cache key, origin fetch), and a repeat of it -> `X-Cache: Hit from cloudfront`.
  - GET with `Cookie: <keyed>=<value-A>` once more                   -> 200, still the PRE-overwrite bytes (separation, not eviction).
Fixed text in the contract: the status `200` and the header `X-Cache: Hit from cloudfront`. Variable parts taken from the request/state: the bucket and object key, the distribution id and DomainName, the cookie name and its two values, and the object bodies.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the behavioral check cloudfront.cfcache-cookie-allowlist-separates-cache-entries.
2. Create an S3 bucket in the runner's region and put an object (e.g. key `cookie.txt`) with some bytes V1.
3. CreateCachePolicy with MinTTL = DefaultTTL = MaxTTL = 3600 and ParametersInCacheKeyAndForwardedToOrigin = {EnableAcceptEncodingGzip: false, EnableAcceptEncodingBrotli: false, HeadersConfig: {HeaderBehavior: "none"}, QueryStringsConfig: {QueryStringBehavior: "none"}, CookiesConfig: {CookieBehavior: "whitelist", Cookies: {Quantity: 1, Items: ["sid"]}}}.
4. CreateOriginAccessControl (OriginAccessControlOriginType "s3", SigningBehavior "always", SigningProtocol "sigv4") and CreateDistribution with one S3 origin carrying that OriginAccessControlId and a DefaultCacheBehavior referencing the cache policy; PutBucketPolicy granting cloudfront.amazonaws.com s3:GetObject with AWS:SourceArn = the distribution ARN.
5. Poll GetDistribution until Distribution.Status == "Deployed". Observe whether that status is ever reported.
6. GET https://<DomainName>/cookie.txt with header `Cookie: sid=alpha`. Observe that no response ever arrives (the poll runs to its bound and the check is cut off at 180 s) instead of 200 with V1.
7. Repeat the GET and inspect the `X-Cache` response header; then overwrite the object with V2 and repeat the GET with `Cookie: sid=alpha; other=zzz` (must still serve V1 as a cache hit) and with `Cookie: sid=beta` (must serve V2 from the origin). None of these can be evaluated, because step 6 never returns.
