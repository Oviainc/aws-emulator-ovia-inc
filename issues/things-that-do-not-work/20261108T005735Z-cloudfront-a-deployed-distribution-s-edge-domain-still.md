Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.cf-cache-key-querystring-allowlist-separates-cache-entries cloudfront.cf-response-headers-policy-adds-and-overrides-headers-at-edge cloudfront.cf-unsigned-request-to-trusted-key-group-distribution-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.cf-cache-key-querystring-allowlist-separates-cache-entries|_poll_until#24|Timeout|raise AssertionError(f"timed out waiting for {description} after {timeout_s}s (last: {last!r})")
Issue-Key: cloudfront.cf-response-headers-policy-adds-and-overrides-headers-at-edge|_poll_until#22|Timeout|raise AssertionError(f"timed out waiting for {description} after {timeout_s}s (last: {last!r})")
Issue-Key: cloudfront.cf-unsigned-request-to-trusted-key-group-distribution-rejected|_poll_until#27|Timeout|raise AssertionError(f"timed out waiting for {description} after {timeout_s}s (last: {last!r})")
Issue-Commit: f42419e01630977583bc09726b6f41b0d8951344

# cloudfront: a Deployed distribution's edge domain still answers nothing, so cache-key query-string separation, response-headers-policy injection and trusted-key-group signed-URL enforcement are all unobservable (3 checks burn their whole budget)

> _Note: this issue was identified against the code base at commit `f42419e01630977583bc09726b6f41b0d8951344`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDistribution

## Description
Cause 1 of 1: the CloudFront data plane never answers at a distribution's DomainName, so three edge behaviors cannot be reached at all. Failing checks: cloudfront.cf-cache-key-querystring-allowlist-separates-cache-entries, cloudfront.cf-response-headers-policy-adds-and-overrides-headers-at-edge, cloudfront.cf-unsigned-request-to-trusted-key-group-distribution-rejected.

AWS: recorded behavior only for the data plane (the Smithy model describes the control plane, not the edge). Once GetDistribution reports Status=Deployed, an HTTP(S) GET of <DomainName>/<key> is served by the edge from the configured origin and answers 200 with the origin's bytes, plus the per-behavior effects the config asks for. Smithy grounds what those effects are: CachePolicyQueryStringsConfig.QueryStringBehavior (@required enum none|whitelist|allExcept|all) with QueryStrings=QueryStringNames determines which query strings enter the cache key, so over one keep-alive session varying only an EXCLUDED parameter must reuse the entry (X-Cache: Hit from cloudfront) while varying an allowlisted one must create a new entry (X-Cache: Miss from cloudfront); ResponseHeadersPolicyCustomHeader carries @required Header/Value/Override, so the policy's header reaches the viewer and Override=true replaces the origin's value of the same header while Override=false leaves the origin's value intact; ResponseHeadersPolicyStrictTransportSecurity has @required AccessControlMaxAgeSec, emitted as Strict-Transport-Security: max-age=<AccessControlMaxAgeSec>; TrustedKeyGroups (@required Enabled/Quantity + Items) makes a behavior signature-gated, so a correctly signed URL answers 200 with the object's exact bytes and an unsigned request to the same URL is refused by the edge with HTTP 403 and a CloudFront XML error body whose <Code> element is MissingKey. In all of these the status, the X-Cache token, the header names/values and the error <Code> are the requirement; the domain name, object key, bucket name, expiry and policy ids are values taken from the request and from state.

Emulator: no response is ever obtained from the distribution's DomainName. Each of the three checks waits for Status=Deployed, then polls <DomainName>/<key> for a 200 on a 180s budget and never gets one, so the check itself ends with "Error: Check timed out after 180.0s" (the suite's verdict is 'mismatch'). No status, no body, no headers are produced at the edge: no X-Cache token, no policy-injected or overridden header, no HSTS header, and no 403/MissingKey for the unsigned request. All three checks mark this with their own DATAPLANE_UNIMPLEMENTED marker, i.e. the distribution is Deployed and the domain is the emulator's own, so the silence is the target's and not the runner's.

Why: every behavior a CDN exists for stays invisible while the control plane reports success. A client cannot fetch an object through a distribution it just created; cannot tell a cache hit from a miss, so it cannot verify that a cache policy's query-string allowlist actually separates entries (and may silently serve one tenant's cached response for another's query string); cannot verify a response-headers policy, so security headers it believes it deployed (HSTS, custom headers) are never actually applied; and cannot verify that a trusted-key-group behavior refuses unsigned access — the most dangerous case, because the emulator gives no evidence either way that private content is protected.

Fix: the CloudFront edge data plane — routing a request for a distribution DomainName to a per-distribution edge handler, the origin fetch, cache-key/cache-behavior evaluation (so X-Cache Hit/Miss is produced per the selected cache policy), response assembly through the attached response-headers policy, and the trusted-key-group signature check that produces the 403 MissingKey error body — together with the distribution state those read. Open issues 20261102T102137Z-cloudfront-a-deployed-distribution-s-edge-domain-answers.md, 20261107T192835Z-cloudfront-a-deployed-distribution-s-edge-domain-answers-787448.md, 20261027T001538Z-cloudfront-the-edge-data-plane-serves-nothing-so.md and 20261029T175823Z-cloudfront-10-more-divergences-the-edge-data-plane.md describe the same absence for OTHER check sets and carry none of the three replay keys below; the replay report marked none of these three checks as owned or stub-blocked.

Traces: cloudfront.cf-cache-key-querystring-allowlist-separates-cache-entries, cloudfront.cf-response-headers-policy-adds-and-overrides-headers-at-edge, cloudfront.cf-unsigned-request-to-trusted-key-group-distribution-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 (326/584 passed, 55.8%); service cloudfront 272/418 (65%), 146 failed, of which 143 were marked OWNED by an open issue or ALREADY FILED (stub-blocked) and skipped.

All three surviving failures report the identical detail:
  [mismatch] cloudfront.cf-cache-key-querystring-allowlist-separates-cache-entries — Error: Check timed out after 180.0s
  [mismatch] cloudfront.cf-response-headers-policy-adds-and-overrides-headers-at-edge — Error: Check timed out after 180.0s
  [mismatch] cloudfront.cf-unsigned-request-to-trusted-key-group-distribution-rejected — Error: Check timed out after 180.0s

In each check the budget is consumed by the same loop, which polls the distribution's own domain for any 200 and gives up only on exhaustion:
  def _fetch(session, url, *, timeout_s=180.0, interval_s=5.0): ... _poll_until(probe, lambda r: r is not None and r.status_code == 200, ...) except AssertionError as exc: raise AssertionError(f"{DATAPLANE_MARKER}: {exc}")
with DATAPLANE_MARKER = "DATAPLANE_UNIMPLEMENTED" and the URL built as f"{ctx.endpoint_url(domain)}/{key}" AFTER _wait_distribution_deployed(cf, dist_id) has already observed Status=Deployed. So the control plane answered and only the edge is silent.

Expected vs actual per check:
- cf-cache-key-querystring-allowlist-separates-cache-entries: expected four GETs over ONE requests.Session() to <domain>/<key>?v=1&junk=a, ?v=1&junk=b, ?v=2&junk=a, ?v=2&junk=a to answer 200 with X-Cache tokens showing Hit for the excluded-parameter change and Miss then Hit for the allowlisted v change. Actual: the first GET never answers 200; no X-Cache header is ever seen.
- cf-response-headers-policy-adds-and-overrides-headers-at-edge: expected 200 with x-fidelity: edge (policy-added), x-amz-meta-conflict: edge (Override=true beats the origin's 'from-origin'), x-amz-meta-passthrough: from-origin (Override=false keeps the origin's value), strict-transport-security containing max-age=31536000, and the object bytes intact. Actual: no response at all.
- cf-unsigned-request-to-trusted-key-group-distribution-rejected: expected the correctly signed URL to answer 200 with the object's exact bytes, and the identical unsigned URL to answer 403 with a CloudFront XML body whose <Code> is MissingKey. Actual: the signed positive control never answers 200 (DATAPLANE_UNIMPLEMENTED), so the unsigned 403/MissingKey leg is never reached.

Model references (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json): CachePolicyQueryStringsConfig.QueryStringBehavior @required enum [none, whitelist, allExcept, all] + QueryStrings->QueryStringNames; ResponseHeadersPolicyCustomHeader.Header/Value/Override all @required; ResponseHeadersPolicyStrictTransportSecurity.Override/AccessControlMaxAgeSec @required; TrustedKeyGroups.Enabled/Quantity @required with Items->TrustedKeyGroupIdList.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay suite simparity-behavioral-aws-v1 (service cloudfront) and observe the three checks in trace_ids each end with "Check timed out after 180.0s".
3. Manually: create an S3 bucket, put an object (e.g. index.html) in it, CreateDistribution over that origin, and poll GetDistribution until Status=Deployed.
4. GET http(s)://<DomainName>/<key>. Expect a 200 carrying the origin's bytes plus an X-Cache header. Observed: the request never completes with a 200 within 180s, so nothing about the edge is observable.
5. Cache-key leg: attach a cache policy with QueryStringsConfig {QueryStringBehavior: whitelist, QueryStrings: [v]} and, over ONE keep-alive session, GET ?v=1&junk=a, ?v=1&junk=b, ?v=2&junk=a, ?v=2&junk=a. Expect X-Cache Miss, Hit, Miss, Hit respectively.
6. Response-headers leg: attach a response-headers policy with CustomHeadersConfig {x-fidelity=edge Override=true; x-amz-meta-conflict=edge Override=true; x-amz-meta-passthrough=edge Override=false} and SecurityHeadersConfig StrictTransportSecurity AccessControlMaxAgeSec=31536000, with the origin object carrying x-amz-meta-conflict/x-amz-meta-passthrough = from-origin. GET the object and expect x-fidelity: edge, x-amz-meta-conflict: edge, x-amz-meta-passthrough: from-origin and strict-transport-security: max-age=31536000.
7. Signed-URL leg: CreatePublicKey + CreateKeyGroup, set the default cache behavior's TrustedKeyGroups {Enabled: true, Quantity: 1, Items: [<kg id>]}. GET the URL signed with that private key and expect 200 with the exact object bytes; GET the same URL unsigned and expect HTTP 403 with a CloudFront XML error body whose <Code> element is MissingKey.
8. Re-replay exactly the three checks in trace_ids to confirm the fix.
