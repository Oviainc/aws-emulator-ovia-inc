Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.deploy-distribution-cfn-serves-live-traffic cloudfront.e2e-webacl-association-blocks-then-unblocks-requests cloudfront.cf-continuous-deployment-single-header-routes-to-staging-distribution
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.deploy-distribution-cfn-serves-live-traffic|run#9|AssertionError|assert status == 200, f"expected 200 from live distribution, got {status}"
Issue-Key: cloudfront.e2e-webacl-association-blocks-then-unblocks-requests|run#20|AssertionError|assert status == 200 and body == b"ok", "baseline fetch before any web ACL must succeed"
Issue-Key: cloudfront.cf-continuous-deployment-single-header-routes-to-staging-distribution|_poll_until#29|Timeout|raise AssertionError(f"timed out waiting for {description} after {timeout_s}s (last: {last!r})")
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# cloudfront: a Deployed distribution's edge domain answers 404, so CFN-deployed content, continuous-deployment header routing and web-ACL block/unblock at the edge are all unobservable

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GET (distribution edge domain)

## Description
One net-new root cause. Related open issues describe the same area but carry none of the check ids below, so none of them covers these: 20261102T102137Z-cloudfront-a-deployed-distribution-s-edge-domain-answers.md and 20261027T001538Z-cloudfront-the-edge-data-plane-serves-nothing-so.md.

=== Cause 1 of 1: a viewer request to a Deployed distribution's domain is answered 404 instead of being served from the origin ===
AWS: the CloudFront viewer data plane has no Smithy operation — recorded behavior only. The recordings show that once GetDistribution reports Status == "Deployed", a GET on the distribution's DomainName returns HTTP 200 with the origin object's exact bytes. Layered on that, the recordings show two further contracts that depend on it: (a) with a continuous-deployment policy using SingleHeaderConfig, a viewer request carrying the configured header (its name must start with the fixed prefix aws-cf-cd-; the rest of the name and its value come from the request) is routed to the staging distribution while a request without it is served by the primary; (b) with a CLOUDFRONT-scope WAFv2 web ACL whose DefaultAction is Block associated via AssociateDistributionWebACL, the same GET becomes HTTP 403, and reverts to 200 after DisassociateDistributionWebACL.
Emulator: the GET answers HTTP 404. The baseline fetch that must precede any of the above never succeeds, so the checks either assert on 404 directly or poll to the harness cap.
Why: a client cannot fetch anything through a distribution the emulator reports as Deployed. Every deployment smoke test fails, continuous-deployment traffic splitting cannot be exercised, and WAF association/disassociation at the edge is silently unverifiable — an emulator that never serves also never blocks, so a Block web ACL looks indistinguishable from no web ACL.
Fix: the edge/data-plane front door — host-based routing for distribution domains and the origin-fetch path behind it — plus the continuous-deployment header router and the web-ACL evaluation hook on that path.
Traces: cloudfront.deploy-distribution-cfn-serves-live-traffic, cloudfront.e2e-webacl-association-blocks-then-unblocks-requests, cloudfront.cf-continuous-deployment-single-header-routes-to-staging-distribution.

## Evidence
deploy-distribution-cfn-serves-live-traffic — expected: 200 from the live distribution. Actual: "AssertionError: expected 200 from live distribution, got 404".
e2e-webacl-association-blocks-then-unblocks-requests — expected: baseline GET before any web ACL returns 200 with body b"ok", then 403 after AssociateDistributionWebACL with a Block web ACL, then 200 again after disassociation. Actual: "AssertionError: baseline fetch before any web ACL must succeed" — the baseline 200/b"ok" never happens, so neither the 403 nor the revert is ever reached.
cf-continuous-deployment-single-header-routes-to-staging-distribution — expected: a GET with the aws-cf-cd-* header served by the staging distribution and one without it served by the primary. Actual: "Check timed out after 180.0s"; the check carries its own DATAPLANE_UNIMPLEMENTED marker for "an edge data plane that serves no traffic at all".

## Steps to Reproduce
Create an S3 (or custom) origin with an object, CreateDistribution over it, poll GetDistribution until Status == "Deployed", then GET https://<DomainName>/<key>: the emulator answers 404 instead of 200 with the object bytes. With that baseline broken, repeat with (a) a continuous-deployment policy using SingleHeaderConfig on a header named aws-cf-cd-<suffix> and send the header — no routing to the staging distribution is observable; (b) a CLOUDFRONT-scope WAFv2 web ACL with DefaultAction=Block created through the us-east-1 wafv2 endpoint and associated via AssociateDistributionWebACL — no 403 is observable, and no 200 after DisassociateDistributionWebACL.
