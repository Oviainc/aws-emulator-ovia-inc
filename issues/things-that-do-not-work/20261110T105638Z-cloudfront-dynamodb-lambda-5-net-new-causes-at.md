Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-brotli-enabled-cache-policy-serves-br-and-keys-on-encoding cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-query-string-allexcept-and-all-shape-the-cache-key cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-request-policy-forwards-what-the-cache-key-excludes cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-unlisted-header-cookie-query-blocked-from-origin cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-viewer-spoofed-edge-headers-rejected-at-the-edge
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-brotli-enabled-cache-policy-serves-br-and-keys-on-encoding|leg#1|AssertionError|assert got == want, (
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-query-string-allexcept-and-all-shape-the-cache-key|leg#1|AssertionError|assert got == want, (
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-request-policy-forwards-what-the-cache-key-excludes|_scenario#1|AssertionError|assert first["cookies"] == ["sim_fwd=c-v1"], (
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-unlisted-header-cookie-query-blocked-from-origin|_scenario#1|AssertionError|assert mixed["rawQueryString"] == "simfwd=1&simb=2", (
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-viewer-spoofed-edge-headers-rejected-at-the-edge|_scenario#1|AssertionError|assert isinstance(country, str) and re.fullmatch(r"[A-Z]{2}", country) and country != "ZZ", (
Issue-Commit: c52a7255ce2600c21f38f62abb43a8c83b37855a

# cloudfront__dynamodb__lambda: 5 net-new causes at the served edge — Brotli compression, allExcept/all cache-key shaping, origin-request-policy forwarding, stripping of unlisted query strings, and generated CloudFront-Viewer-* headers are all missing

> _Note: this issue was identified against the code base at commit `c52a7255ce2600c21f38f62abb43a8c83b37855a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CloudFront edge data-plane request (GET https://<distribution DomainName>/<path>)

## Description
FIVE net-new root causes; a fixer must close all five before submitting. The distribution edge now ANSWERS (each failure reports a concrete X-Cache value and a real origin-invocation count from the Lambda origin), so the open "the edge data plane serves nothing / never answers" issues no longer describe these failures and do not carry these replay keys: 20261105T111951Z, 20261105T114920Z, 20261105T133059Z, 20261105T205807Z, 20261105T224454Z, 20261106T071701Z and 20261109T172142Z (all cloudfront__dynamodb__lambda). Cause 5 is the same family as the open 20261107T043755Z-cloudfront-dynamodb-lambda-the-edge-never-generates-cloudfront.md, which owns a DIFFERENT check and does not carry this one's key.

Cause 1 of 5: the edge never applies Brotli compression and does not key the cache on the content encoding.
AWS: Smithy: CacheBehavior.Compress — "Whether you want CloudFront to automatically compress certain files for this cache behavior"; ParametersInCacheKeyAndForwardedToOrigin.EnableAcceptEncodingBrotli adds the normalized Accept-Encoding to the cache key. Recorded behavior: with Compress=true and EnableAcceptEncodingBrotli, a viewer sending Accept-Encoding: br is answered with Content-Encoding: br, and the br and identity variants occupy separate cache entries over a single origin fetch each.
Emulator: the response to an Accept-Encoding: br viewer carries no content-encoding at all (the origin was invoked once, so the fetch happened — the compression step did not).
Why: a client measuring transfer sizes or asserting Content-Encoding sees behavior impossible in production, and the encoding dimension of the cache key cannot be exercised.
Fix: the edge response pipeline — honour the behavior's Compress flag together with the cache policy's EnableAcceptEncodingGzip/Brotli, negotiating against the viewer's Accept-Encoding and including the normalized encoding in the cache key.
Traces: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-brotli-enabled-cache-policy-serves-br-and-keys-on-encoding.

Cause 2 of 5: the cache key ignores QueryStringBehavior=allExcept (and all), so excluded parameters split cache entries.
AWS: Smithy: CachePolicyQueryStringsConfig.QueryStringBehavior — "allExcept — All query strings in viewer requests are included in the cache key and in requests that CloudFront sends to the origin, except those that are listed in the QueryStringNames type, which are not included"; "all — All query strings ... are included". Recorded behavior: under allExcept listing 'simnokey', two requests differing only in simnokey share one cache entry: the second is answered X-Cache: Hit from cloudfront and the origin is invoked once in total.
Emulator: the second request is answered 'Miss from cloudfront' and the origin is invoked twice — the excluded name is still part of the key.
Why: origin load and hit ratio are wrong for any URL carrying excluded parameters, and the allExcept/all semantics cannot be validated offline.
Fix: the edge cache-key computation — implement all four QueryStringBehavior modes (none/whitelist/allExcept/all) from the behavior's cache policy rather than keying on the raw query string.
Traces: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-query-string-allexcept-and-all-shape-the-cache-key.

Cause 3 of 5: an origin request policy's allowlisted cookies/headers are not forwarded to the origin.
AWS: Smithy: CacheBehavior.OriginRequestPolicyId binds an origin request policy; its purpose, as the CacheBehavior.ForwardedValues doc states, is "If you want to send values to the origin but not include them in the cache key, use an origin request policy". Recorded behavior: a cookie (or header, or query string) listed in the origin request policy's CookiesConfig/HeadersConfig/QueryStringsConfig arrives at the origin even though the cache policy excludes it from the key.
Emulator: the origin saw no cookies at all for a request carrying the allowlisted cookie — the value never left the edge.
Why: the whole point of an origin request policy is unobservable: an origin that needs a session cookie or a viewer header for correctness receives nothing, so the application behind the distribution behaves differently on the emulator than in production.
Fix: the edge origin-request construction — merge the behavior's origin request policy (CookiesConfig/HeadersConfig/QueryStringsConfig) into the outbound origin fetch, independently of the cache key.
Traces: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-request-policy-forwards-what-the-cache-key-excludes.

Cause 4 of 5: query strings that neither policy lists are forwarded to the origin instead of being stripped.
AWS: recorded behavior only for the forwarded set (Smithy defines the two policies that compose it): the origin receives exactly the query strings the cache policy and the origin request policy between them allow, in the viewer's order; a parameter listed by neither is removed before the origin fetch. The same holds for unlisted headers and cookies.
Emulator: the origin received 'simfwd=1&simunl=leakq&simb=2' where the allowed set is 'simfwd=1&simb=2' — the unlisted parameter leaked through.
Why: this is a data-leak and a correctness divergence: values a distribution is configured to withhold reach the origin, so an origin that branches on an unexpected parameter (or logs it) behaves differently and may expose viewer-supplied data the configuration was meant to drop.
Fix: the edge origin-request construction — filter the viewer's query strings, headers and cookies down to the union of the cache policy's and origin request policy's allowed sets, preserving the viewer's order.
Traces: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-unlisted-header-cookie-query-blocked-from-origin.

Cause 5 of 5: the edge generates no CloudFront-Viewer-Country, so the spoofed-edge-header contract cannot be exercised.
AWS: recorded behavior only: when a behavior's origin request policy includes the CloudFront-managed viewer headers, the edge GENERATES CloudFront-Viewer-Country as an ISO 3166-1 alpha-2 code for every viewer request (even one that never sent it), and a viewer-supplied value for a CloudFront-generated header is overwritten by the edge's own rather than passed through.
Emulator: the origin saw None for CloudFront-Viewer-Country on the check's positive control, so the spoof-rejection assertion is never reached. The same root cause is already filed for a sibling check as 20261107T043755Z-cloudfront-dynamodb-lambda-the-edge-never-generates-cloudfront.md; that issue does not carry this check's key, so it is recorded here.
Why: geo-based routing and personalization at the origin receive nothing, and an attacker-supplied CloudFront-Viewer-* header is indistinguishable from an edge-generated one — a trust boundary the real edge enforces.
Fix: the edge request pipeline — generate the CloudFront-* viewer headers (Country, Address, Forwarded-Proto, Is-*-Viewer, ...) and overwrite any viewer-supplied value for them before the origin fetch.
Traces: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-viewer-spoofed-edge-headers-rejected-at-the-edge.

## Evidence
Cause 1: "AssertionError: /br/obj Accept-Encoding 'br': expected (X-Cache, Content-Encoding, origin invocations) ('Miss from cloudfront', 'br', 1), got ('Miss from cloudfront', None, 1)" — the origin was fetched, the response was not Brotli-encoded.
Cause 2: "AssertionError: /ae/x?simnokey=2&a=1: expected (X-Cache, origin invocations) ('Hit from cloudfront', 1), got ('Miss from cloudfront', 2)".
Cause 3: "AssertionError: cookie sim_fwd is allowlisted by the origin request policy; the origin's cookies were None".
Cause 4: "AssertionError: the origin must receive only the listed query strings, in the viewer's order: expected 'simfwd=1&simb=2', got 'simfwd=1&simunl=leakq&simb=2'".
Cause 5: "AssertionError: control: the edge must report a real two-letter country, got None" — expected an ISO 3166-1 alpha-2 value in CloudFront-Viewer-Country at the origin.
(Parameter and cookie names above are the checks' own fixtures; the requirement is the filtering/forwarding/generation rule, not these literals.)

## Steps to Reproduce
Common setup: create a distribution over a Lambda function-URL origin that echoes the request it received (headers, cookies, raw query string) and counts its invocations; wait for Status Deployed; drive viewer requests at the distribution's DomainName.
1. Cause 1: behavior with Compress=true and a cache policy with EnableAcceptEncodingBrotli; GET with Accept-Encoding: br -> no Content-Encoding on the response.
2. Cause 2: cache policy with QueryStringBehavior=allExcept listing 'simnokey'; GET /ae/x?simnokey=1&a=1 then /ae/x?simnokey=2&a=1 -> second is a Miss and the origin is invoked twice.
3. Cause 3: cache policy excluding cookie 'sim_fwd' plus an origin request policy whitelisting it; GET with Cookie: sim_fwd=... -> origin reports no cookies.
4. Cause 4: policies allowing only 'simfwd' and 'simb'; GET /x?simfwd=1&simunl=leakq&simb=2 -> origin receives the unlisted 'simunl' too.
5. Cause 5: origin request policy including the CloudFront-managed viewer headers; GET without sending CloudFront-Viewer-Country -> origin sees None instead of a two-letter code.
Or replay the five check ids in trace_ids.
