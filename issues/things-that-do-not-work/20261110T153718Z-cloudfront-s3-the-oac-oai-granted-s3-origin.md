Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__s3.oac-grant-scoped-to-another-distribution-arn-forbidden cloudfront__s3.oac-signed-origin-fetch-without-bucket-policy-grant-forbidden
Issue-Key: cloudfront__s3.oac-grant-scoped-to-another-distribution-arn-forbidden|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__s3.oac-signed-origin-fetch-without-bucket-policy-grant-forbidden|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# cloudfront__s3: the OAC/OAI-granted S3 origin fetch still never succeeds at the edge — both OAC authorization checks now exhaust the run budget with no verdict

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject (CloudFront edge origin fetch, S3 origin)

## Description
One root cause in the cloudfront__s3 suite, reached by two checks. The open issue 20261110T092303Z-cloudfront-s3-6-net-new-causes-in-the.md lists these two check ids under its Cause 2 ("an origin fetch the bucket policy/ACL grants to the distribution is still refused 403 AccessDenied at the edge") but does NOT carry their current replay keys: the failure signature has changed from an assertion inside _edge_status to the suite killing the check at its budget, so the replay report does not mark them owned. File-and-name rather than drop: a fixer closing that issue must re-replay these two, and if the edge now hangs instead of answering 403 that is a second, later failure mode in the same path.

Cause 1 of 1: a viewer GET at a distribution whose S3 origin is reachable only through an origin access control (or a legacy origin access identity) never produces the edge response the check waits for, in either the granted or the ungranted direction.
AWS: recorded behavior only — neither the CloudFront nor the S3 model carries this authorization rule. With an OAC attached to the S3 origin and a bucket policy allowing s3:GetObject to Principal {"Service": "cloudfront.amazonaws.com"} under Condition StringEquals on AWS:SourceArn = this distribution's ARN, the signed origin fetch succeeds and the edge answers HTTP 200 with the object's bytes. When the SourceArn condition names a DIFFERENT distribution's ARN, or when no bucket-policy grant exists at all, the origin fetch is refused and the edge relays HTTP 403 to the viewer. Both outcomes arrive within the distribution's deployment and edge-propagation window.
Emulator: neither outcome arrives. Both checks are reported "Error: Check timed out after 60.0s" with no assertion reached — the edge-status poll (which, per the sibling check in the same suite, repeatedly observes 403 with x-cache "Error from cloudfront") never satisfies its predicate and the harness kills the check, so the granted leg and the scoped-to-another-ARN negative are both unobservable.
Why: a client cannot serve any private object through CloudFront, and — worse for a test or an audit — cannot distinguish a correctly locked origin from a broken one, because the granted and the ungranted configuration behave identically. An OAC-protected static site or private asset path is simply unavailable.
Fix: the edge's origin-request signing for an S3 origin, plus the S3 bucket-policy evaluation for the cloudfront.amazonaws.com service principal and the AWS:SourceArn condition (and the OAI canonical-user READ grant for the legacy path) — the granted fetch must return the object and the ungranted/wrong-ARN fetch must be relayed to the viewer as 403, promptly in both cases rather than never.
Traces: cloudfront__s3.oac-grant-scoped-to-another-distribution-arn-forbidden, cloudfront__s3.oac-signed-origin-fetch-without-bucket-policy-grant-forbidden.

## Evidence
Both checks report:
  Error: Check timed out after 60.0s
with no assertion reached.

Expected: for the granted configuration, HTTP 200 at the distribution domain with the object's exact bytes; for an OAC grant whose AWS:SourceArn names another distribution, and for a bucket with no grant at all, HTTP 403 relayed to the viewer. Actual: the edge-status poll never satisfies either predicate inside the run budget, so the check is killed and no status/code/body was ever compared. The related open issue 20261110T092303Z-cloudfront-s3-6-net-new-causes-in-the.md records the same path answering, repeatedly to its own 300 s deadline, (403, {'x-cache': 'Error from cloudfront'}, '<Error><Code>AccessDenied</Code><Message>Access Denied</Message>...') on a bucket policy that grants the fetch — i.e. the granted leg has never succeeded. Distribution ids, domains, bucket names, request ids and host ids are per-run values.

## Steps to Reproduce
1. Create a private bucket with Block Public Access on and put an object in it.
2. CreateOriginAccessControl (SigV4, always) and CreateDistribution over the bucket's REST endpoint with that OAC on the origin; wait for Status=Deployed.
3. PutBucketPolicy allowing s3:GetObject to Principal {"Service":"cloudfront.amazonaws.com"} with Condition StringEquals AWS:SourceArn = the distribution's ARN; GET the object at the distribution domain — AWS answers 200 with the object bytes.
4. Replace the policy so AWS:SourceArn names a DIFFERENT distribution's ARN, and separately delete the policy entirely; GET again — AWS relays 403 to the viewer.
5. Re-replay cloudfront__s3.oac-grant-scoped-to-another-distribution-arn-forbidden and cloudfront__s3.oac-signed-origin-fetch-without-bucket-policy-grant-forbidden to confirm the fix.
