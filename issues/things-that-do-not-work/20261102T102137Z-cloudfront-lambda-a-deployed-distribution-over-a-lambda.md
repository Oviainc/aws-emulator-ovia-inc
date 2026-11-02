Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected|_poll_until#43|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied|_poll_until#39|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudfront__lambda: a Deployed distribution over a Lambda origin answers nothing at its edge domain, so AllowedMethods enforcement and signed-URL source-IP denial are unreachable

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDistribution

## Description
Cause 1 of 1: the distribution's edge domain never produces a response, so no edge enforcement can be observed.
AWS: once the distribution reports Status=Deployed, a request to its DomainName is answered by the edge. A method outside the cache behavior's AllowedMethods is refused 403 by CloudFront itself (the origin is never reached), and an unsigned request to a trusted-key-group behavior is 403 while a correctly signed URL whose policy pins the viewer's source IP is 200 — the check first probes an unsigned /whoami behavior for its 200 baseline.
Emulator: no response is ever obtained from the distribution domain. One check ends at the 60 s per-check budget ("Check timed out after 60.0s"); the other raises "TimeoutError: 200 from the unsigned /whoami behavior: the run budget ended first; not graded" while polling that same domain.
Why: the edge enforces nothing because it answers nothing — a client can neither fetch through the distribution nor observe any of the refusals CloudFront is responsible for, while the control plane reports a healthy Deployed distribution.
Fix: the CloudFront edge data plane (distribution-domain routing to an edge handler, Lambda function-URL origin fetch, AllowedMethods and trusted-key-group/signed-URL evaluation) in the cloudfront service. Open issues 20261027T001538Z-cloudfront-lambda-the-edge-enforces-nothing-because-it.md and 20261029T080904Z-cloudfront-lambda-a-deployed-distribution-s-edge-domain.md describe the same absence for other checks but do not carry these two replay keys.
Traces: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected, cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied.

## Evidence
cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-allowed-methods-post-rejected: "Error: Check timed out after 60.0s" (expected a 403 from the edge for a POST to a GET/HEAD-only behavior, with the origin never invoked).
cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-signed-url-source-ip-denied: "TimeoutError: 200 from the unsigned /whoami behavior: the run budget ended first; not graded" — the baseline 200 through the distribution never arrives, so the signed-URL source-IP denial is never reached.

## Steps to Reproduce
1. Create a Lambda function URL origin and a distribution in front of it; wait for GetDistribution Status=Deployed.
2. GET https://<distribution domain>/whoami -> expect 200 from the origin through the edge.
3. POST to a behavior whose AllowedMethods are GET/HEAD -> expect 403 from CloudFront without the origin being invoked.
Observed: neither request ever completes; the checks end at their budget.
