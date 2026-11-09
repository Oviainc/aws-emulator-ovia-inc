Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.origin-request-policy-header-does-not-enter-the-cache-key
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.origin-request-policy-header-does-not-enter-the-cache-key|_poll_until#29|Timeout|raise AssertionError(f"timed out waiting for {description} after {timeout_s}s (last: {last!r})")
Issue-Commit: 3ab6a566a5e8009a4810936ef4b3a5f4bdba04b2

# cloudfront: a Deployed distribution's edge domain never answers 200, so the origin-request-policy-vs-cache-key contract is never exercised (check times out at 180s)

> _Note: this issue was identified against the code base at commit `3ab6a566a5e8009a4810936ef4b3a5f4bdba04b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject (CloudFront edge data plane; CreateDistribution/CreateCachePolicy/CreateOriginRequestPolicy control plane)

## Description
Cause 1 of 1: a distribution that CloudFront reports as Deployed serves nothing on its own DomainName, so no viewer request to the edge ever reaches a 200 and the cache-key semantics of a cache policy versus an origin request policy can never be observed. Failing check: cloudfront.origin-request-policy-header-does-not-enter-the-cache-key.

AWS: a request to a distribution's edge domain for an object that exists at the configured origin is answered 200 with the origin object's body, plus CloudFront's own response headers — `X-Cache: Miss from cloudfront` on the first (cold) request for a cache key and `X-Cache: Hit from cloudfront` on a subsequent request that resolves to the same cache key. Which viewer values form the cache key is governed by the cache policy, not the origin request policy: Smithy `OriginRequestPolicyConfig` documents that an origin request policy "determines the values that CloudFront includes in requests that it sends to the origin" and that "If you want to send values to the origin and also include them in the cache key, use CachePolicy". So a header allowlisted by `CachePolicyConfig.ParametersInCacheKeyAndForwardedToOrigin.HeadersConfig` (HeaderBehavior=whitelist) splits the cache (changing it yields a Miss), while the same header allowlisted ONLY by `OriginRequestPolicyConfig.HeadersConfig` does not (changing it yields a Hit on the existing entry). The edge data plane itself is not modeled — the Smithy model covers only the control plane — so the 200/body/X-Cache behavior above is recorded behavior only.

Emulator: the control-plane calls succeed (cache policies, origin request policy, OAC and the distribution are all created), but the check never gets past its first edge fetch: it polls the distribution's DomainName for a 200 and is killed by the suite at the 180s cap ("Check timed out after 180.0s"), i.e. the emulator's edge endpoint never answered 200 for `/keyed/obj.txt` within the check's whole budget. No `X-Cache` value — Hit or Miss — is ever produced, and the origin object body is never served. Hypothesis (neither the model nor the recording states which of the two is the stall): either GetDistribution never reports `Status == "Deployed"`, or the distribution DomainName is not served/does not route to the S3 origin; the recorded symptom is only that the check makes no progress and no 200 ever arrives.

Why: a client that points a browser, CDN test or integration suite at the distribution's DomainName gets nothing at all, so content delivery, cache-hit behavior and any cache-key tuning (cache policy vs origin request policy) cannot be developed or regression-tested against the emulator. Worse, a configuration that would fragment the cache in production (conflating the origin request policy with the cache policy) is indistinguishable here from a correct one, because no cache state is ever observable.

Fix: the CloudFront edge data plane — the request path that serves a distribution's DomainName (origin fetch through the configured Origin/OAC, cache lookup/store keyed by the behavior's CachePolicy, and the `X-Cache: Hit from cloudfront` / `Miss from cloudfront` response header), together with whatever marks a created distribution `Deployed`. The cache key must be derived from the matched cache behavior's CachePolicy only; the OriginRequestPolicy must affect the forwarded origin request and never the cache key.

Related but NOT covering this check (the suite does not mark it owned by any of them): 20261108T005735Z-cloudfront-a-deployed-distribution-s-edge-domain-still.md, 20261102T102137Z-cloudfront-a-deployed-distribution-s-edge-domain-answers.md, 20261107T192835Z-cloudfront-a-deployed-distribution-s-edge-domain-answers-787448.md, 20261108T060951Z-cloudfront-a-deployed-distribution-s-edge-domain-serves.md.

Traces: cloudfront.origin-request-policy-header-does-not-enter-the-cache-key.

## Evidence
Expected (AWS): GET https://<distribution-domain>/keyed/obj.txt with header X-Sim-Variant: one -> 200 with the origin object body and `X-Cache: Miss from cloudfront`; repeating with X-Sim-Variant: two -> 200 + `Miss from cloudfront` (the cache policy allowlists that header); repeating that again -> 200 + `Hit from cloudfront`. GET /forwarded/obj.txt (cache policy HeaderBehavior=none, origin request policy allowlists X-Sim-Variant) -> 200 + `Miss from cloudfront` cold, then 200 + `Hit from cloudfront` when only X-Sim-Variant changes.

Actual (emulator): no leg is ever reached. The check polls the edge for a 200 (5s interval, 180s budget) and the suite kills it first:
  [mismatch] cloudfront.origin-request-policy-header-does-not-enter-the-cache-key
  Error: Check timed out after 180.0s
No 200, no body and no X-Cache header of either value were ever observed from the distribution's DomainName. The check's own marker for this class of failure is DATAPLANE_UNIMPLEMENTED ("a failure caused by an edge data plane that serves no traffic at all").

## Steps to Reproduce
1. CreateCachePolicy A: MinTTL=DefaultTTL=MaxTTL=3600, ParametersInCacheKeyAndForwardedToOrigin.HeadersConfig = {HeaderBehavior: whitelist, Headers: [X-Sim-Variant]}, cookies/query-strings none.
2. CreateCachePolicy B: same TTLs, HeadersConfig = {HeaderBehavior: none}.
3. CreateOriginRequestPolicy: HeadersConfig = {HeaderBehavior: whitelist, Headers: [X-Sim-Variant]}, cookies/query-strings none.
4. Create an S3 bucket in the client's region, put objects keyed `keyed/obj.txt` and `forwarded/obj.txt`, and CreateOriginAccessControl (s3, SigningBehavior=always, sigv4).
5. CreateDistribution with one S3 origin using that OAC, CacheBehaviors: /keyed/* -> cache policy A, /forwarded/* -> cache policy B + the origin request policy; DefaultCacheBehavior -> cache policy B. Grant cloudfront.amazonaws.com s3:GetObject on the bucket scoped to the distribution ARN.
6. Wait for GetDistribution to report Status=Deployed, then GET https://<DomainName>/keyed/obj.txt with X-Sim-Variant: one over a keep-alive session.
7. Observe: the request never answers 200 — the poll exhausts and the check is killed at the 180s cap, instead of returning the object body with `X-Cache: Miss from cloudfront` and then exercising the Hit/Miss arms above.
