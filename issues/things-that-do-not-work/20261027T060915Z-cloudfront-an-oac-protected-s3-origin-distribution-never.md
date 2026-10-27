Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.e2e-distribution-invalidation-cache-refresh
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.e2e-distribution-invalidation-cache-refresh|_poll_fetch#26|Timeout|time.sleep(interval_s)
Issue-Commit: 09bd9e5af92b317e5d499d0eb02cee520d64dd1d

# cloudfront: an OAC-protected S3-origin distribution never serves anything at its edge domain, so the stale-cache-then-invalidation refresh sequence can never be observed

> _Note: this issue was identified against the code base at commit `09bd9e5af92b317e5d499d0eb02cee520d64dd1d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
This issue carries ONE root cause in the cloudfront service. Close it before submitting.

Cause 1 of 1: a distribution built over a private S3 origin with an origin access control, a long-TTL cache policy and a completed invalidation never produces a single response at its edge DomainName, so none of the caching contract (first fetch from origin, stale hit after the origin object changes, fresh content only after an invalidation completes) can be exercised.

AWS: the control-plane half is modelled — Smithy: CreateDistribution -> POST /2020-05-31/distribution (201); CreateCachePolicy's CachePolicyConfig carries MinTTL/DefaultTTL/MaxTTL and ParametersInCacheKeyAndForwardedToOrigin; CreateOriginAccessControl declares OriginAccessControlOriginType/SigningBehavior/SigningProtocol; CreateInvalidation -> POST /2020-05-31/distribution/{DistributionId}/invalidation (201, payload Invalidation, errors AccessDenied 403 / BatchTooLarge 413 / InconsistentQuantities 400 / InvalidArgument 400 / MissingBody 400 / NoSuchDistribution 404 / TooManyInvalidationsInProgress 400); GetInvalidation -> GET /2020-05-31/distribution/{DistributionId}/invalidation/{Id} (200, payload Invalidation). The edge data plane itself is not described by that control-plane model, so for it this is recorded behavior only. What the recording requires, in order: GetDistribution reports Status "Deployed"; a plain GET of http://<DomainName>/<key> returns the object body currently at the origin; after the origin object is overwritten WITHOUT an invalidation, the same GET still returns the PREVIOUS body (the cache policy's MinTTL/DefaultTTL govern it because S3 sends no Cache-Control); CreateInvalidation answers with an Invalidation whose Status is NOT "Completed" (it is asynchronous — AWS returns "InProgress" and only later transitions), GetInvalidation on that id eventually reports Status "Completed"; and only after that does the same GET return the NEW body. The private origin bucket (all four Block Public Access settings on, a bucket policy granting s3:GetObject to Service cloudfront.amazonaws.com conditioned on AWS:SourceArn == the distribution ARN) must admit the distribution and nothing else. Every body/name/id above is a per-run generated value filling that template, not part of it.

Emulator: no response is ever obtained through the distribution's edge domain. The check consumes its entire budget on the control-plane setup plus the first edge fetch and is cut off at the 60 s harness cap ("Check timed out after 60.0s") without a single edge response, so the initial fetch, the stale hit, the asynchronous CreateInvalidation status and the post-invalidation refresh are all unobservable. Hypothesis: the distribution's DomainName is not routable/served at all in the emulator, which is the same absence the related open issues describe for other checks; the replay output reports only the cap, not the failing step.

Why: a client relying on AWS's behavior uses this exact sequence to validate its cache strategy — that content is cached for the TTL it configured, that an origin write alone does NOT change what viewers see, and that an invalidation is the thing that does (and is asynchronous, so it must be polled to Completed before the refresh is guaranteed). Against an emulator whose edge answers nothing, that validation cannot pass or fail on the real property: a user cannot distinguish "caches correctly" from "caches nothing" or "invalidation is a no-op", and CI that depends on the refresh step hangs to its timeout rather than reporting a result.

Fix: the cloudfront crate's edge/data-plane request path — the distribution's DomainName must resolve and answer requests, route them to the configured origin per the matching cache behavior (signing the origin request with the attached OriginAccessControl for an S3 origin and honouring the origin bucket policy's SourceArn condition), cache the response under the cache policy's TTLs so a subsequent viewer request is served from cache, and have the invalidation state machine in the invalidation handler/state actually evict the matching paths when an invalidation transitions to Completed (and return a non-Completed Status from CreateInvalidation).

Traces: cloudfront.e2e-distribution-invalidation-cache-refresh.

Related open issues describe a missing edge data plane for OTHER checks and do not carry this check's replay key: 20261027T001538Z-cloudfront-the-edge-data-plane-serves-nothing-so.md, 20261024T182555Z-cloudfront-ten-fidelity-divergences-distribution-policy-tenant-validation.md and 20261024T182555Z-cloudfront-s3-distributions-over-s3-origins-serve-nothing.md.

## Evidence
Cause 1 — cloudfront.e2e-distribution-invalidation-cache-refresh [mismatch]:
  Error: Check timed out after 60.0s

The check: creates a private bucket (PutPublicAccessBlock with all four settings true), puts page.txt with body v1; CreateCachePolicy with MinTTL=300 / DefaultTTL=3600 / MaxTTL=86400 and no cookies/headers/query-strings in the cache key; CreateOriginAccessControl (type s3, SigningBehavior always, sigv4); CreateDistribution with that OAC on an S3 origin and that CachePolicyId on the default behavior; TagResource; PutBucketPolicy granting s3:GetObject to cloudfront.amazonaws.com conditioned on AWS:SourceArn == the distribution ARN; polls GetDistribution to Status "Deployed"; then fetches http://<DomainName>/page.txt.

Expected (recorded AWS): first fetch returns v1; after overwriting page.txt with v2 the same fetch still returns v1; CreateInvalidation for ["/page.txt"] returns Invalidation.Status != "Completed"; GetInvalidation reaches Status "Completed"; the next fetch returns v2.

Actual (emulator): no response is ever obtained from the distribution's edge domain — the check is killed at the 60 s cap, so not one of the four edge/invalidation observations above is produced. The failure is the ABSENCE of any edge response, not a wrong value in one. The bucket/distribution/OAC names, ids, domain name and the v1/v2 body strings are per-run generated values and are not requirements.

## Steps to Reproduce
1. Create an S3 bucket with all four Block Public Access settings enabled and PutObject key page.txt with body "v1".
2. CreateCachePolicy with MinTTL=300, DefaultTTL=3600, MaxTTL=86400 and CookieBehavior/HeaderBehavior/QueryStringBehavior all "none".
3. CreateOriginAccessControl with OriginAccessControlOriginType=s3, SigningBehavior=always, SigningProtocol=sigv4.
4. CreateDistribution with one S3 origin (DomainName <bucket>.s3.<region>.amazonaws.com, OriginAccessControlId from step 3, S3OriginConfig.OriginAccessIdentity "") and DefaultCacheBehavior { TargetOriginId, ViewerProtocolPolicy allow-all, CachePolicyId from step 2 }.
5. PutBucketPolicy allowing s3:GetObject to Principal Service cloudfront.amazonaws.com with Condition StringEquals AWS:SourceArn = the distribution ARN.
6. Poll GetDistribution until Distribution.Status == "Deployed".
7. GET http://<DomainName>/page.txt — expect body "v1". Observe: the request never completes; no edge response at all.
8. PutObject page.txt with body "v2" and GET again — expect the cached "v1".
9. CreateInvalidation with Paths {Quantity 1, Items ["/page.txt"]} — expect Invalidation.Status != "Completed"; poll GetInvalidation until "Completed".
10. GET http://<DomainName>/page.txt again — expect "v2". Observe: steps 7-10 produce nothing; the check is cut off at the 60 s cap.

Re-replay cloudfront.e2e-distribution-invalidation-cache-refresh to confirm.
