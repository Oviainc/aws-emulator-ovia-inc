Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.cf-response-headers-policy-adds-and-overrides-headers-at-edge cloudfront.server-side-security-headers-policy-enforced-on-response cloudfront.cache-behavior-first-listed-match-wins-over-more-specific-pattern cloudfront.cf-cache-key-querystring-allowlist-separates-cache-entries cloudfront.cfcache-cookie-allowlist-separates-cache-entries cloudfront.cf-compression-serves-gzip-only-when-accept-encoding-present cloudfront.cf-continuous-deployment-single-header-routes-to-staging-distribution cloudfront.cf-unsigned-request-to-trusted-key-group-distribution-rejected cloudfront.signed-cookies-custom-policy-admits-wildcard-and-expired-policy-refused cloudfront.cfcache-conditional-get-304-only-for-the-matching-etag cloudfront.cfcache-range-beyond-object-rejected-and-partial-served-from-cache cloudfront.custom-error-response-rewrites-status-and-caches-for-error-caching-min-ttl cloudfront.https-only-and-disallowed-method-rejected-at-edge cloudfront.deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-request-rewrite cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-response-header
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.cf-response-headers-policy-adds-and-overrides-headers-at-edge|run#27|AssertionError|assert headers.get("x-fidelity") == "edge", (
Issue-Key: cloudfront.server-side-security-headers-policy-enforced-on-response|run#13|AssertionError|assert headers.get("x-content-type-options") == "nosniff", (
Issue-Key: cloudfront.cache-behavior-first-listed-match-wins-over-more-specific-pattern|run#32|AssertionError|assert later.headers.get(_MARKER_HEADER) == "second", (
Issue-Key: cloudfront.cf-cache-key-querystring-allowlist-separates-cache-entries|run#32|AssertionError|assert x2 == "Hit from cloudfront", (
Issue-Key: cloudfront.cfcache-cookie-allowlist-separates-cache-entries|run#38|AssertionError|assert excluded.content == v1, (
Issue-Key: cloudfront.cf-compression-serves-gzip-only-when-accept-encoding-present|run#25|AssertionError|assert gz_resp.headers.get("content-encoding") == "gzip", (
Issue-Key: cloudfront.cf-continuous-deployment-single-header-routes-to-staging-distribution|run#34|AssertionError|assert staged_resp.content == b"staging-origin", (
Issue-Key: cloudfront.cf-unsigned-request-to-trusted-key-group-distribution-rejected|run#34|AssertionError|assert unsigned.status_code == 403, (
Issue-Key: cloudfront.signed-cookies-custom-policy-admits-wildcard-and-expired-policy-refused|run#42|AssertionError|assert missing.status_code == 403, (
Issue-Key: cloudfront.cfcache-conditional-get-304-only-for-the-matching-etag|run#28|AssertionError|assert etag and etag.startswith('"') and etag.endswith('"'), (
Issue-Key: cloudfront.cfcache-range-beyond-object-rejected-and-partial-served-from-cache|run#30|AssertionError|assert unsatisfiable.status_code == 416, (
Issue-Key: cloudfront.custom-error-response-rewrites-status-and-caches-for-error-caching-min-ttl|run#31|AssertionError|assert errored.status_code == 200, (
Issue-Key: cloudfront.https-only-and-disallowed-method-rejected-at-edge|run#32|AssertionError|assert r.status_code == 301, (
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected|_edge_get#13|AssertionError|raise AssertionError(f"{DATAPLANE_MARKER}: {exc}{tail}") from exc
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-request-rewrite|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-response-header|run#13|AssertionError|assert raw.headers.get("x-amz-meta-sim2real-secret") == secret, (
Issue-Commit: c52a7255ce2600c21f38f62abb43a8c83b37855a

# cloudfront: 10 net-new causes in the now-served edge data plane — response-headers policies, cache-key allowlists, compression, continuous-deployment routing, signed-URL/cookie enforcement, ETag/304, unsatisfiable Range, custom error responses, redirect-to-https and CloudFront Functions are all ignored

> _Note: this issue was identified against the code base at commit `c52a7255ce2600c21f38f62abb43a8c83b37855a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CloudFront edge data-plane request (GET https://<distribution DomainName>/<path>)

## Description
The distribution edge now ANSWERS (every failure below carries the emulator's own 'x-cache: Miss from cloudfront' / 'x-amz-cf-id' response headers), so the previously filed "the edge domain answers nothing" issues no longer describe these failures. What answers is a pass-through proxy that ignores almost everything the distribution config says. TEN net-new root causes; a fixer must close all ten before submitting. Related but NOT covering (none carries these replay keys): 20261102T102137Z-cloudfront-a-deployed-distribution-s-edge-domain-answers.md, 20261108T005735Z-cloudfront-a-deployed-distribution-s-edge-domain-still.md, 20261108T060951Z-cloudfront-a-deployed-distribution-s-edge-domain-serves.md, 20261107T231127Z-cloudfront-a-cookie-allowlist-cache-policy-is-never.md — all of those describe an edge that returns nothing, which is no longer the observed behavior.

Cause 1 of 10: a ResponseHeadersPolicy attached to a cache behavior is never applied to the response.
AWS: Smithy: CacheBehavior.ResponseHeadersPolicyId binds a response headers policy to the behavior; ResponseHeadersPolicyCustomHeadersConfig is documented "CloudFront includes these headers in HTTP responses that it sends for requests that match a cache behavior that's associated with this response headers policy", and SecurityHeadersConfig contributes X-Content-Type-Options, Strict-Transport-Security, X-Frame-Options, Referrer-Policy etc. Recorded behavior: the viewer response carries the policy's custom headers and the configured security headers, in addition to the origin's own.
Emulator: the viewer response carries only x-cache, x-amz-cf-id, content-length and date — no custom header from the policy and no security header. This is also what makes the behavior-precedence check fail: the behavior at index 1 is only distinguishable by the response-headers policy bound to it, and that policy produces nothing.
Why: a client cannot verify CORS, HSTS, CSP or any header-injection contract through the emulator, and a security posture the distribution declares is silently absent at the edge.
Fix: the edge data plane's response pipeline — after resolving the matching behavior, apply its ResponseHeadersPolicy (CustomHeadersConfig, SecurityHeadersConfig, CorsConfig, with the configured override semantics) before answering the viewer.
Traces: cloudfront.cf-response-headers-policy-adds-and-overrides-headers-at-edge, cloudfront.server-side-security-headers-policy-enforced-on-response, cloudfront.cache-behavior-first-listed-match-wins-over-more-specific-pattern.

Cause 2 of 10: the edge cache key ignores the cache policy's query-string and cookie allowlists.
AWS: Smithy: CachePolicyQueryStringsConfig.QueryStringBehavior=whitelist means "Only the query strings in viewer requests that are listed in the QueryStringNames type are included in the cache key"; CachePolicyCookiesConfig behaves the same way for cookies. Recorded behavior: two requests that differ ONLY in a query string (or cookie) the policy does not list hit the same cache entry — the second answers X-Cache: Hit from cloudfront and the origin is not re-read.
Emulator: a request differing only in the EXCLUDED query parameter answers 'Miss from cloudfront', and a request differing only in the EXCLUDED cookie is served freshly rewritten origin bytes — the cache key wrongly includes the unlisted name.
Why: cache hit ratios and origin load are wrong by orders of magnitude for any client whose URLs carry tracking parameters, and no cache-key-shaping contract can be validated offline.
Fix: the edge cache-key computation — derive the key from the behavior's cache policy ParametersInCacheKeyAndForwardedToOrigin (QueryStringsConfig / CookiesConfig / HeadersConfig) instead of the raw request.
Traces: cloudfront.cf-cache-key-querystring-allowlist-separates-cache-entries, cloudfront.cfcache-cookie-allowlist-separates-cache-entries.

Cause 3 of 10: the edge never compresses, even with Compress enabled and a compressible viewer.
AWS: Smithy: CacheBehavior.Compress — "Whether you want CloudFront to automatically compress certain files for this cache behavior"; with Compress=true, EnableAcceptEncodingGzip in the cache policy and a viewer sending Accept-Encoding: gzip, the response carries Content-Encoding: gzip and a smaller body, and the uncompressed variant is served to a viewer that sends no Accept-Encoding.
Emulator: the response to a gzip viewer has content-encoding absent and content-length equal to the raw object size.
Why: a client measuring transfer size or asserting Content-Encoding sees behavior that cannot occur in production, and the encoding dimension of the cache key is untestable.
Fix: the edge response pipeline — honour the behavior's Compress flag and the cache policy's EnableAcceptEncodingGzip/Brotli, negotiating against the viewer's Accept-Encoding.
Traces: cloudfront.cf-compression-serves-gzip-only-when-accept-encoding-present.

Cause 4 of 10: a continuous-deployment policy's SingleHeaderConfig does not route to the staging distribution.
AWS: recorded behavior only for the edge path (Smithy models ContinuousDeploymentPolicyConfig with SingleHeaderPolicyConfig {Header, Value}): a viewer request carrying that header/value is served by the STAGING distribution's configuration and origin; a request without it is served by the primary.
Emulator: the request carrying the configured header is answered with the PRIMARY origin's body (b'primary-origin').
Why: a blue/green rollout cannot be exercised at all — the staging distribution is unreachable, so a caller validating a canary before promotion gets a false positive.
Fix: the edge request-routing step — before resolving a behavior, consult the distribution's ContinuousDeploymentPolicyId and dispatch to the staging distribution when the SingleHeader (or SingleWeight) criterion matches.
Traces: cloudfront.cf-continuous-deployment-single-header-routes-to-staging-distribution.

Cause 5 of 10: TrustedKeyGroups is not enforced at the edge — unsigned requests to private content are served.
AWS: Smithy: CacheBehavior.TrustedKeyGroups is "A list of key groups that CloudFront can use to validate signed URLs or signed cookies" and "When a cache behavior contains trusted key groups, CloudFront requires signed URLs or signed cookies for all requests that match the cache behavior". Recorded behavior: a request with no (or an expired, or a non-matching-policy) signature is refused 403 and the object's bytes never reach the viewer; a correctly signed URL or cookie set is served 200.
Emulator: an unsigned request to a trusted-key-group behavior is answered 200 with the protected object's bytes, and a request with no signed cookies is likewise answered 200 with the protected body.
Why: this is an authorization bypass. Private content a client believes is gated behind signed URLs/cookies is public on the emulator, so an access-control regression in the client's signing path is invisible until production.
Fix: the edge request pipeline — when the matched behavior has TrustedKeyGroups (or TrustedSigners) enabled, verify the CloudFront signed-URL query parameters or CloudFront-Policy/-Signature/-Key-Pair-Id cookies against the key group's public keys and the policy's Resource/DateLessThan/IpAddress conditions, answering 403 otherwise.
Traces: cloudfront.cf-unsigned-request-to-trusted-key-group-distribution-rejected, cloudfront.signed-cookies-custom-policy-admits-wildcard-and-expired-policy-refused.

Cause 6 of 10: edge responses carry no ETag, so conditional GETs can never be 304.
AWS: recorded behavior only: CloudFront passes the origin's validators through, so a cached object answers with a quoted ETag; a follow-up GET with If-None-Match set to that ETag is answered 304 Not Modified with no body, and a non-matching If-None-Match is answered 200.
Emulator: the edge serves no quoted ETag at all (the check reads None), so there is nothing to be conditional on and the 304 path is unreachable.
Why: every revalidating client (browser, CDN-in-front, HTTP cache) re-downloads whole objects, and a client asserting 304 behavior cannot test it.
Fix: the edge response pipeline — propagate the origin's ETag/Last-Modified on both miss and hit, and implement If-None-Match/If-Modified-Since evaluation producing 304.
Traces: cloudfront.cfcache-conditional-get-304-only-for-the-matching-etag.

Cause 7 of 10: an unsatisfiable Range is answered 200 with the whole object instead of 416.
AWS: recorded behavior only: a Range whose first byte position is at or beyond the object length is refused 416 Range Not Satisfiable (with Content-Range: bytes */<length>), and a satisfiable Range is answered 206 Partial Content with the requested slice and a Content-Range.
Emulator: a Range starting at byte 4096 of a 2048-byte object is answered 200 with the entire object body.
Why: the edge is over-permissive — a client that relies on 416 to detect a bad offset (resumable download, chunked reader) silently processes the whole object as if it were the requested slice, corrupting the assembled result.
Fix: the edge Range handling — evaluate the Range header against the cached/origin object length and answer 416 (or 206 with Content-Range) rather than ignoring it.
Traces: cloudfront.cfcache-range-beyond-object-rejected-and-partial-served-from-cache.

Cause 8 of 10: CustomErrorResponses are not applied — the origin's error status and body pass straight through.
AWS: Smithy: CustomErrorResponse.ResponseCode is "The HTTP status code that you want CloudFront to return to the viewer along with the custom error page" and ResponsePagePath is the page fetched for it; ErrorCachingMinTTL is the minimum time the ErrorCode is cached. Recorded behavior: when the origin answers the configured ErrorCode, the viewer receives ResponseCode with the body of ResponsePagePath, and the error is cached for ErrorCachingMinTTL so a repeat does not re-hit the origin.
Emulator: the viewer receives the origin's 404 and the origin's raw error document (an S3 <Error><Code>NoSuchKey</Code>... body), with no rewrite to the configured ResponseCode and no custom page.
Why: a single-page app or branded-error-page setup serves raw origin XML to end users, and any client asserting the rewritten status sees the origin's.
Fix: the edge error-handling step — match the origin's status against the distribution's CustomErrorResponses, fetch ResponsePagePath, answer ResponseCode, and cache the outcome for ErrorCachingMinTTL.
Traces: cloudfront.custom-error-response-rewrites-status-and-caches-for-error-caching-min-ttl.

Cause 9 of 10: ViewerProtocolPolicy is not enforced — plain HTTP to a redirect-to-https behavior is served.
AWS: Smithy: ViewerProtocolPolicy (enum allow-all | https-only | redirect-to-https) is documented exactly — "redirect-to-https: If a viewer submits an HTTP request, CloudFront returns an HTTP status code of 301 (Moved Permanently) to the viewer along with the HTTPS URL" and "https-only: If a viewer sends an HTTP request, CloudFront returns an HTTP status code of 403 (Forbidden)". Also in the same check: a method outside AllowedMethods is refused 405.
Emulator: a plain-HTTP request to a redirect-to-https behavior is answered 200 with the object's body (526 bytes of content) and no Location header.
Why: a client's HTTP-to-HTTPS upgrade path is never exercised, and content a distribution declares HTTPS-only is served in clear text.
Fix: the edge request pipeline — evaluate the matched behavior's ViewerProtocolPolicy against the request scheme and answer 301 (with the https URL in Location) or 403 before any origin fetch, and enforce AllowedMethods with 405.
Traces: cloudfront.https-only-and-disallowed-method-rejected-at-edge.

Cause 10 of 10: CloudFront Functions associated with a behavior are not executed at the edge, and origin response headers are dropped on the way to the viewer.
AWS: Smithy: CacheBehavior.FunctionAssociations — "A list of CloudFront functions that are associated with this cache behavior. CloudFront functions must be published to the LIVE stage to associate them with a cache behavior." Recorded behavior: a viewer-request function's mutations to request.uri and request.headers take effect on the origin fetch; a viewer-response function's header additions reach the viewer; a function that throws causes the edge to answer 503; and on a behavior with NO function association the origin's own response headers are passed through to the viewer unchanged.
Emulator: the throwing branch keeps answering 200 (the check's own marker reports DATAPLANE_UNIMPLEMENTED after 120 s of polling for the 503), the viewer-request rewrite never takes effect (the check exhausts its 180 s budget), and on the function-free control behavior the origin's metadata header is absent from the viewer response, which carries only x-cache/x-amz-cf-id/content-length/date.
Why: edge compute is entirely unobservable — function code cannot be tested, a function fault does not surface as the 503 a client must handle, and even without functions the viewer loses every header the origin set (content-type, cache-control, custom metadata).
Fix: the edge request/response pipeline — execute the behavior's LIVE-stage FunctionAssociations for viewer-request and viewer-response events (mapping a function fault to 503), and pass the origin's response headers through to the viewer instead of synthesising a minimal header set.
Traces: cloudfront.deep-cloudfront-1-cf-edge-fn-throw-yields-503-rejected, cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-request-rewrite, cloudfront.deep-cloudfront-1-cf-edge-fn-viewer-response-header.

## Evidence
Cause 1: "AssertionError: the policy's custom header never reached the viewer: None (all headers: {'x-cache': 'Miss from cloudfront', 'x-amz-cf-id': '...', 'content-length': '32', 'date': '...'})"; "AssertionError: missing/incorrect X-Content-Type-Options header, got headers={'x-cache': 'Miss from cloudfront', 'x-amz-cf-id': '...', 'content-length': '32', 'connection': 'close', 'date': '...'}"; "AssertionError: a behaviour past index 0 must be reachable and its response headers policy applied: expected X-Sim-Behavior='second' for /c/obj.txt, got None".
Cause 2: "AssertionError: a request differing only in the EXCLUDED query parameter missed the cache: X-Cache='Miss from cloudfront' — the cache key wrongly includes 'junk'"; "AssertionError: a request differing only in the EXCLUDED cookie 'other' was served the rewritten origin bytes b'sim2real-cookie-keyed-version-2' — the cache key wrongly includes 'other'". Expected 'Hit from cloudfront' and the first-version bytes.
Cause 3: "AssertionError: the edge did not compress for a gzip viewer: content-encoding=None, headers={'x-cache': 'Miss from cloudfront', ..., 'content-length': '2944', ...}". Expected Content-Encoding: gzip.
Cause 4: "AssertionError: the aws-cf-cd-fidelity header did not route to the staging distribution: b'primary-origin'". Expected the staging origin's body.
Cause 5: "AssertionError: an unsigned request to a trusted-key-group behavior was served: 200 / 'sim2real-signed-url-protected-object'" and "AssertionError: a request with no signed cookies must be refused: expected 403, got 200 / 'sim2real-cookie-protected-object-a'". Expected HTTP 403 and no body in both.
Cause 6: "AssertionError: the edge served no quoted ETag to be conditional on: None". Expected a quoted ETag and a 304 for a matching If-None-Match.
Cause 7: "AssertionError: a Range starting at byte 4096 of a 2048-byte object was answered 200; an edge that ignores an unsatisfiable Range and serves the whole object is over-permissive: b'\\x00\\x01\\x02...'". Expected 416 with Content-Range: bytes */2048.
Cause 8: "AssertionError: the custom error response must rewrite the origin's status: expected 200 (ResponseCode), got 404 / '<Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message>...'". Expected the configured ResponseCode (200) and the ResponsePagePath body.
Cause 9: "AssertionError: redirect-to-https answered plain HTTP with 200, expected 301; headers {'x-cache': 'Miss from cloudfront', ..., 'content-length': '526', ...}". Expected 301 Moved Permanently with the https URL in Location.
Cause 10: "AssertionError: DATAPLANE_UNIMPLEMENTED: timed out waiting for 503 for the throwing branch after 120s (last: <Response [200]>)"; "Error: Check timed out after 180.0s" for the viewer-request rewrite; "AssertionError: the origin metadata header did not reach the viewer on the function-free behaviour: {'x-cache': 'Miss from cloudfront', 'x-amz-cf-id': '...', 'content-length': '38', 'date': '...'}". Expected 503 for the throw, the rewritten URI at the origin, and the origin's header present on the viewer response.
(Hosts, ports, x-amz-cf-id values, content-lengths and dates in these tails are per-run values, not requirements.)

## Steps to Reproduce
Common setup: create a distribution over a reachable origin (us-east-1 control plane), wait for GetDistribution to report Status Deployed, then drive viewer requests at the distribution's DomainName.
1. Cause 1: attach a ResponseHeadersPolicy with a CustomHeadersConfig entry and a SecurityHeadersConfig (ContentTypeOptions) to the behavior; GET an object -> the header is absent from the response.
2. Cause 2: attach a cache policy with QueryStringBehavior=whitelist (listing only 'keep') / CookiesConfig whitelist (listing only 'keep'); GET /obj?keep=1&junk=a then /obj?keep=1&junk=b -> second answers Miss instead of Hit.
3. Cause 3: set Compress=true and a cache policy with EnableAcceptEncodingGzip; GET with Accept-Encoding: gzip -> no Content-Encoding.
4. Cause 4: create a staging distribution and a continuous-deployment policy with SingleHeaderConfig {Header, Value}; GET with that header -> primary origin's body.
5. Cause 5: set TrustedKeyGroups on the behavior; GET with no signature, and GET with an expired CloudFront-Policy cookie set -> 200 with the protected bytes instead of 403.
6. Cause 6: GET an object, then GET again with If-None-Match set to the returned ETag -> no ETag was returned at all.
7. Cause 7: GET with Range: bytes=4096- on a 2048-byte object -> 200 with the whole object instead of 416.
8. Cause 8: configure CustomErrorResponses {ErrorCode:404, ResponseCode:'200', ResponsePagePath:'/error.html', ErrorCachingMinTTL:n}; GET a missing key -> origin's 404 and raw XML.
9. Cause 9: set ViewerProtocolPolicy=redirect-to-https; GET over plain HTTP -> 200 instead of 301.
10. Cause 10: publish a CloudFront Function to LIVE and associate it viewer-request (one that throws, and one that rewrites request.uri), plus a function-free control behavior; drive requests -> 200 instead of 503, no rewrite, and the origin's response header missing on the control.
Or replay the check ids in trace_ids.
