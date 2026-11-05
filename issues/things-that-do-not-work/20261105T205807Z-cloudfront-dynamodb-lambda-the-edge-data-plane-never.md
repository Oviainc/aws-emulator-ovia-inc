Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-brotli-enabled-cache-policy-serves-br-and-keys-on-encoding cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-unlisted-header-cookie-query-blocked-from-origin
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-brotli-enabled-cache-policy-serves-br-and-keys-on-encoding|warm#1|Timeout|out = lam.invoke(FunctionName=function_name,
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-unlisted-header-cookie-query-blocked-from-origin|warm#1|Timeout|out = lam.invoke(FunctionName=function_name,
Issue-Commit: a6bd8162e254cd53a71309df843cd006b7af8737

# cloudfront__dynamodb__lambda: the edge data plane never answers for a Deployed distribution, so Brotli compression keying and unlisted header/cookie/query stripping are never observable

> _Note: this issue was identified against the code base at commit `a6bd8162e254cd53a71309df843cd006b7af8737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: a distribution created over a Lambda function-URL origin never serves a request on its edge domain, so neither the Accept-Encoding (br) cache-key/compression stage nor the cache-policy/origin-request-policy stage that decides what reaches the origin can be exercised. Failing check ids for this cause: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-brotli-enabled-cache-policy-serves-br-and-keys-on-encoding, cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-unlisted-header-cookie-query-blocked-from-origin.

AWS: Smithy (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31) CreateDistribution POST /2020-05-31/distribution -> 201 with Distribution (containing DomainName) and an ETag header; the modeled surface has no error for "the edge does not serve", because once Status becomes Deployed a GET on that DomainName is served through the matching CacheBehavior. For the behaviours under test, AWS's documented data-plane rules are: with Compress=true and a cache policy whose EnableAcceptEncodingBrotli is true, a viewer request carrying Accept-Encoding: br is answered with a Brotli-encoded body and Content-Encoding: br, and the encoding is part of the cache key so requests with and without br are separate cache entries (X-Cache: Hit from cloudfront vs Miss from cloudfront); and a header, cookie or query-string parameter that is in neither the cache policy nor the origin request policy is stripped at the edge and never reaches the origin, so the origin's echo of the request it received does not contain it. The rest is recorded behavior only: the checks poll the edge domain and assert on the echo-origin body and the X-Cache header.

Emulator: recorded behavior only - no response that satisfies either check's predicate is ever observed; both checks poll the distribution's edge domain until the harness aborts them with "Check timed out after 60.0s". Nothing is served, so there is no status, no Content-Encoding, no X-Cache value and no origin echo to compare.

Why: a client that provisions a distribution against this emulator can read its control-plane record back but can never fetch anything through it. Compression negotiation, cache-key separation by encoding, and the edge's stripping of unlisted headers/cookies/query parameters are all silently unverifiable; code that depends on CloudFront removing a secret header or cookie before it reaches the origin would be shipped untested, and a client waiting for a 200 from the edge hangs instead of being answered.

Fix: the CloudFront data plane - the request path that resolves a distribution's edge domain and forwards to the configured origin, plus the cache-key/compression stage (Accept-Encoding br/gzip handling, X-Cache) and the cache-policy/origin-request-policy filtering stage in front of it. The control-plane handler for CreateDistribution is not the defect; it already returns a DomainName and a Deployed status.

Related open issues that do NOT carry these two replay keys: 20261105T111951Z-cloudfront-dynamodb-lambda-a-deployed-distribution-never-serves.md, 20261105T114920Z-cloudfront-dynamodb-lambda-all-eight-deep-cloudfront-3.md and 20261105T133059Z-cloudfront-dynamodb-lambda-the-edge-data-plane-serves.md describe the same family of symptom for other checks in this group; the suite reports these two checks as unowned, so they are filed here.

Traces: the two failing check ids listed above (both reported under the cloudfront__dynamodb__lambda heading).

## Evidence
Expected (AWS): after CreateDistribution returns 201 with a DomainName and GetDistribution reports Status=Deployed, GET https://<DomainName>/<path> with Accept-Encoding: br returns HTTP 200 with Content-Encoding: br and an X-Cache header, and a second GET with the same cache key returns X-Cache: Hit from cloudfront while a request with a different Accept-Encoding is a Miss; a GET carrying a header, cookie and query parameter that appear in neither the cache policy nor the origin request policy returns HTTP 200 whose origin-echoed body (headers/cookies/rawQueryString) does not contain them, and the per-path invocation counter in DynamoDB shows the origin was reached exactly once.

Actual (emulator): no response ever satisfies the predicate; the harness reports for both checks:
  Error: Check timed out after 60.0s
No HTTP status, Content-Encoding, X-Cache value or origin echo is produced at all, so the edge served nothing for a distribution the control plane reports as Deployed.

## Steps to Reproduce
1. Create a DynamoDB counter table and a Lambda function-URL origin that echoes rawPath, rawQueryString, cookies and headers and bumps a per-path counter (as the two checks do).
2. CreateDistribution with that function URL as a custom origin, Compress=true, and a cache policy with MinTTL=DefaultTTL=MaxTTL=3600, EnableAcceptEncodingBrotli=true, plus an origin request policy that forwards only a named allowlist.
3. Poll GetDistribution until Status=Deployed and read DomainName.
4. GET https://<DomainName>/<path> with Accept-Encoding: br -> expect 200, Content-Encoding: br, X-Cache present; repeat to expect X-Cache: Hit from cloudfront, and repeat without Accept-Encoding to expect a separate (Miss) cache entry.
5. GET https://<DomainName>/<path>?unlisted=1 with an unlisted header and an unlisted cookie -> expect 200 and an origin echo that omits the unlisted header, cookie and query parameter.
Observed: steps 4 and 5 never receive any response from the edge; both checks poll until the 60s harness deadline.
