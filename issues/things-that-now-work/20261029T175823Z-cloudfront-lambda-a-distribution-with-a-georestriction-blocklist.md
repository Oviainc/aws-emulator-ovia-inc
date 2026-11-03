Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked
Issue-Key: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# cloudfront__lambda: a distribution with a GeoRestriction blocklist never answers at its edge domain, so the geo-blocked 403 and the allowed 200 are both unreachable

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
One net-new root cause.

Cause 1 of 1: the distribution's edge domain answers nothing, so GeoRestriction enforcement cannot be observed.
AWS: a distribution whose DistributionConfig.Restrictions.GeoRestriction is RestrictionType=blacklist with a country in Items answers a viewer request from that country with HTTP 403 and CloudFront's own error body, and a request from any other country with the origin's 200 response. Smithy models Restrictions/GeoRestriction (and CreateDistribution's InvalidGeoRestrictionParameter / InvalidLocationCode refusals) on the control plane, but the edge enforcement itself is not modelled — recorded behavior only: the recording shows an HTTP response from the edge within seconds of Status=Deployed, 403 for the blocked viewer and 200 for the allowed one.
Emulator: no response ever arrives at the distribution's DomainName. The check polls to the harness cap and the suite reports `Error: Check timed out after 60.0s`. The control-plane setup (the Lambda function URL origin, CreateDistribution, and the wait for Deployed) all succeeded in the same run, so only the edge fetch is missing.
Why: geo restriction is a security control. A client cannot verify that viewers from a blocked country are refused, and cannot distinguish "blocked" from "the edge is not serving" — both look like no response.
Fix: the CloudFront edge data-plane handler in the cloudfront crate — the host-based router must map d<id>.cloudfront.net to the stored distribution, and the viewer-request stage must apply Restrictions.GeoRestriction (403 for a blocked country code) before fetching from the origin. Open issue 20261027T001538Z-cloudfront-lambda-the-edge-enforces-nothing-because-it.md describes the same surface for AllowedMethods and signed-URL enforcement but does not carry this check's replay key.
Traces: cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked.

## Evidence
Verbatim from the replay:
  cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked
  Error: Check timed out after 60.0s

Expected: HTTP 403 from the distribution's edge domain for a viewer in a blocklisted country, and HTTP 200 (the origin's body) for a viewer outside it.
Actual: no HTTP response from the edge domain at all; the 60 s poll budget is exhausted. Sibling cloudfront__lambda checks in the same run record the same symptom (`TimeoutError: 200 from the unsigned /whoami behavior: the run budget ended first; not graded`), confirming the edge, not the Lambda origin, is the missing component.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay cloudfront__lambda.deep-cloudfront-2-cf-edge-enforce-geo-restriction-blocklist-blocked.
2. Create a Lambda function with a function URL as the origin, then CreateDistribution with Restrictions={GeoRestriction:{RestrictionType:"blacklist", Quantity:1, Items:["<country-code>"]}}.
3. Poll GetDistribution until Status=Deployed.
4. HTTP GET https://<DomainName>/ with the CloudFront-Viewer-Country header (or from the blocked country) and again without it. Observe that neither request receives any response before the 60 s cap.
