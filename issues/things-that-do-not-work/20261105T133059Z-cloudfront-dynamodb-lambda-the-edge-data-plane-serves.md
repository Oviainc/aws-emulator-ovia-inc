Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-path-prefixes-the-path-the-origin-sees cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-edge-generates-viewer-country-and-forwarded-proto
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-path-prefixes-the-path-the-origin-sees|warm#1|Timeout|out = lam.invoke(FunctionName=function_name,
Issue-Key: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-edge-generates-viewer-country-and-forwarded-proto|warm#1|Timeout|out = lam.invoke(FunctionName=function_name,
Issue-Commit: 269019b5b42949e66be9fce8117dfdac4ac9abf5

# cloudfront__dynamodb__lambda: the edge data plane serves nothing for a Deployed distribution — OriginPath prefixing and generated viewer headers never reach a verdict

> _Note: this issue was identified against the code base at commit `269019b5b42949e66be9fce8117dfdac4ac9abf5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: a distribution the emulator reports as Deployed never answers a request on its own edge domain, so neither the OriginPath rewrite nor the edge-generated viewer headers can be observed. Failing checks: cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-path-prefixes-the-path-the-origin-sees, cloudfront__dynamodb__lambda.deep-cloudfront-3-cf-origin-edge-generates-viewer-country-and-forwarded-proto.

AWS: CreateDistribution (Smithy: POST /2020-05-31/distribution, 201, output Distribution with DomainName; errors include InvalidOrigin/NoSuchOrigin/InvalidArgument) mints a distribution whose Distribution$DomainName is reachable once Distribution$Status is "Deployed". A GET on that domain is then served through the matched cache behaviour: the origin is asked for Origin$OriginPath + the viewer path (so the origin never sees the unprefixed path, and a second origin on the same backend with no OriginPath sees the bare path), and the edge adds its own generated request headers — CloudFront-Viewer-Country and X-Forwarded-Proto — to what the origin receives. The viewer sees HTTP 200 and the origin's body. The HTTP status, headers and body of that edge response are the requirement; the distribution id, edge domain name and the counter values in the recordings are per-run values, not part of it.

Emulator: recorded behavior only — both checks poll `GET https://<DomainName>/<path>` for a 200 (240 s budget, 5 s interval) and never receive one; the harness reports "Check timed out after 60.0s" for each, and the checks' own DATAPLANE_UNIMPLEMENTED marker ("the endpoint serves no edge at all") is what the poll would raise. No response is observed at all, so neither the prefixed origin path nor the generated headers are ever exercised.

Why: a client that fetches content through a distribution this emulator created gets nothing back. Any application or test that depends on CloudFront actually proxying to its origin — path prefixing via OriginPath, and the CloudFront-Viewer-Country / X-Forwarded-Proto headers an origin reads to do geo- or scheme-dependent work — cannot run against the emulator, and an origin-side assertion silently never fires rather than failing loudly.

Fix: the CloudFront edge data plane — routing a request arriving on a distribution's DomainName to the configured origin, and the request-shaping stage in front of it that applies Origin$OriginPath and injects the edge-generated viewer headers.

Note: open issue 20261105T111951Z-cloudfront-dynamodb-lambda-a-deployed-distribution-never-serves.md records the same symptom for six sibling deep-cloudfront-3-cf-origin-* checks but does not carry the replay keys of these two, which the suite therefore reported as unowned; closing it does not close these.

Traces: the two check ids in trace_ids.

## Evidence
Expected: after CreateDistribution over a Lambda function-URL origin and Status=Deployed, `GET https://<DomainName>/<path>` returns 200 with the origin's echoed body; the echo shows the origin was asked for `/<OriginPath>/<path>` (and the unprefixed path was never requested), and shows CloudFront-Viewer-Country and X-Forwarded-Proto present among the headers the origin received.

Actual: no 200 is ever observed on the edge domain. Both checks end with "Error: Check timed out after 60.0s" while inside their `_poll_until(..., lambda s: s == 200, timeout_s=240.0, interval_s=5.0, description=f"200 from https://{domain}{path}")` wait; the origin's invocation counters in DynamoDB are never bumped by an edge-originated request.

## Steps to Reproduce
1. Create a Lambda function-URL "echo" origin that records its invocation and echoes rawPath, query string, cookies and headers.
2. CreateDistribution with that origin, a cache policy with MinTTL=DefaultTTL=MaxTTL=3600, and one origin carrying Origin$OriginPath plus a second origin on the same backend without it.
3. Poll GetDistribution until Distribution$Status == "Deployed".
4. GET https://<Distribution$DomainName>/<path> — expect 200 and an echo whose rawPath is /<OriginPath>/<path>; observed: no response satisfying 200 within the poll window.
5. GET the same domain and inspect the echoed request headers for CloudFront-Viewer-Country and X-Forwarded-Proto — expect both present; observed: no response at all.
