Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-query-string-allexcept-and-all-shape-the-cache-key cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-request-policy-forwards-what-the-cache-key-excludes
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-query-string-allexcept-and-all-shape-the-cache-key|warm#1|Timeout|out = lam.invoke(FunctionName=function_name,
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-request-policy-forwards-what-the-cache-key-excludes|warm#1|Timeout|out = lam.invoke(FunctionName=function_name,
Issue-Commit: a983232b5da502bccde79d757bd972103fe44705

# cloudfront__dynamodb__lambda: two more deep-cloudfront-3 cache-key checks never complete — a Deployed distribution's edge domain answers nothing, so query-string cache-key configuration is never exercised

> _Note: this issue was identified against the code base at commit `a983232b5da502bccde79d757bd972103fe44705`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: a distribution created over a Lambda function-URL origin never becomes usable end to end — the edge domain serves no response — so both query-string cache-key checks run out their budget without ever obtaining an edge response.

AWS: recorded behavior only for the data plane — CloudFront's Smithy model covers the control plane (CreateDistribution/GetDistribution/CreateCachePolicy/CreateOriginRequestPolicy and the DistributionConfig, CachePolicyConfig.ParametersInCacheKeyAndForwardedToOrigin.QueryStringsConfig members used here) but not the edge request path, so the recording is the authority. After CreateDistribution, GetDistribution settles to Status "Deployed"; a GET to https://<DomainName><path> is then relayed to the configured origin and answered 200 with the origin's body, and the cache policy's QueryStringsConfig (QueryStringBehavior ALL vs ALL_EXCEPT with a named list) decides whether a second request with a different query string is a cache Hit or a Miss, observable both in the X-Cache response header and in the origin's own invocation counters.

Emulator: both checks are killed by the suite's 60 s cap with no verdict. The checks' own helpers poll GetDistribution until Status == "Deployed" (900 s budget) and then poll the distribution's DomainName for a 200 (240 s budget, raising a DATAPLANE_UNIMPLEMENTED marker when exhausted); neither predicate is satisfied inside the cap, so no edge response, no X-Cache value and no origin invocation count is ever produced. Nothing about cache-key behaviour can be observed because nothing is served.

Why: a client that puts content behind a distribution gets no content at all. Cache-key configuration (QueryStringBehavior ALL vs ALL_EXCEPT) silently cannot be relied on: an application that depends on two distinct query strings being cached separately — or on excluded query strings collapsing to one cache entry — has no way to validate it, and origin-load assumptions built on cache hits are unverifiable.

Fix: the CloudFront edge data plane — request routing from a distribution's DomainName to its configured origin (here a Lambda function-URL origin reached with OAC/SigV4), the cache-key computation from the attached cache policy, and the X-Cache/Age response headers — together with whatever prevents GetDistribution from settling to Status "Deployed" for such a distribution.

Related open issues that describe the same area but do NOT carry these two replay keys (so they do not cover these checks): 20261105T114920Z-cloudfront-dynamodb-lambda-all-eight-deep-cloudfront-3.md, 20261105T111951Z-cloudfront-dynamodb-lambda-a-deployed-distribution-never-serves.md, 20261105T205807Z-cloudfront-dynamodb-lambda-the-edge-data-plane-never.md, 20261105T133059Z-cloudfront-dynamodb-lambda-the-edge-data-plane-serves.md.

Traces: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-query-string-allexcept-and-all-shape-the-cache-key and cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-request-policy-forwards-what-the-cache-key-excludes.

## Evidence
Both checks are reported as [mismatch] with detail "Check timed out after 60.0s" and produce no HTTP response from the distribution.

Expected (AWS): GetDistribution reaches Status "Deployed"; a GET to https://<DomainName>/<path> returns 200 with the origin's echo body; a repeat with a query string the cache policy includes yields X-Cache: Miss from cloudfront and a new origin invocation, while a repeat with a query string the policy excludes yields X-Cache: Hit from cloudfront and no new origin invocation.

Actual (emulator): neither the Deployed poll (GetDistribution, 900 s budget) nor the edge 200 poll (240 s budget, DATAPLANE_UNIMPLEMENTED marker) completes; the check is cut off at the 60 s harness cap with zero edge responses observed. The checks' second half — comparing the cache policy's included/excluded query strings against the origin-request policy's forwarded set — is never reached.

## Steps to Reproduce
1. Create a Lambda function with a function URL that echoes the path, raw query string, headers and cookies it receives and atomically bumps DynamoDB counters per rawPath.
2. Create a cache policy whose ParametersInCacheKeyAndForwardedToOrigin.QueryStringsConfig uses QueryStringBehavior=ALL_EXCEPT with a named query string, plus a second policy with QueryStringBehavior=ALL; set MinTTL=DefaultTTL=MaxTTL=3600 so nothing can expire mid-check.
3. CreateDistribution over the function-URL origin with those cache behaviours, grant cloudfront.amazonaws.com lambda:InvokeFunctionUrl/lambda:InvokeFunction on the distribution ARN.
4. Poll GetDistribution for Status == "Deployed", then GET https://<DomainName>/<path>?<query>.
5. Observe that no 200 is ever served from the distribution domain — the request path produces nothing, so the Hit/Miss cache-key verdict can never be taken and the check only ever times out.
