Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__s3.oac-signed-origin-fetch-without-bucket-policy-grant-forbidden cloudfront__s3.oac-grant-scoped-to-another-distribution-arn-forbidden cloudfront__s3.oac-origin-tenant-directory-path-rejected-until-list-reveals-no-such-key cloudfront__s3.invalidation-refreshes-once-and-the-edge-re-caches-the-updated-object cloudfront__s3.viewer-request-function-edge-runs-live-code-until-an-edit-is-published cloudfront__s3.e2e-invalidate-cloudfront-cache-lifecycle cloudfront__s3.cloudfront-caller-reference-replay-changing-only-logging-returns-the-original-unchanged cloudfront__s3.cloudfront-update-enabling-logging-into-an-acl-disabled-bucket-is-rejected
Issue-Key: cloudfront__s3.oac-signed-origin-fetch-without-bucket-policy-grant-forbidden|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__s3.oac-grant-scoped-to-another-distribution-arn-forbidden|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__s3.oac-origin-tenant-directory-path-rejected-until-list-reveals-no-such-key|_get_until#10|Timeout|time.sleep(5)
Issue-Key: cloudfront__s3.invalidation-refreshes-once-and-the-edge-re-caches-the-updated-object|_get_until#14|Timeout|time.sleep(5)
Issue-Key: cloudfront__s3.viewer-request-function-edge-runs-live-code-until-an-edit-is-published|_get_until#12|Timeout|time.sleep(5)
Issue-Key: cloudfront__s3.e2e-invalidate-cloudfront-cache-lifecycle|phase#8|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: cloudfront__s3|CreateDistribution|DistributionAlreadyExists
Issue-Key: cloudfront__s3.cloudfront-update-enabling-logging-into-an-acl-disabled-bucket-is-rejected|_error#9|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudfront__s3: four causes — the S3-origin edge serves nothing, redirect-to-https answers 403 instead of 301, a logging-only caller-reference replay is refused, and logging into an ACL-disabled bucket is accepted

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Four net-new root causes on distributions over S3 origins. Open issues naming neighbouring symptoms (20261024T182555Z-cloudfront-s3-distributions-over-s3-origins-serve-nothing.md, 20261029T175823Z-cloudfront-s3-an-oac-protected-s3-origin-never.md) do NOT carry these replay keys.

Cause 1 of 4: the edge never answers for an S3 origin.
AWS: after Status=Deployed, a GET to the distribution DomainName is served from the S3 origin — 200 with the object for a granted OAC, 403 AccessDenied (S3 error XML) when the bucket policy does not grant the distribution, 403 then 404 NoSuchKey for a directory-style tenant path once s3:ListBucket is added, the re-cached object after an invalidation, and the live function's rewritten object until an edit is published.
Emulator: no response is ever obtained from the distribution domain; each of these five checks polls to the 60 s budget ("Error: Check timed out after 60.0s").
Why: neither the success path nor the refusal path of an S3-origin distribution can be observed, so OAC/bucket-policy enforcement and invalidation semantics are entirely unverifiable.
Fix: the CloudFront edge data plane (distribution-domain routing, S3 origin fetch with OAC signing, cache/invalidation state) in the cloudfront service, with the S3 origin read path.
Traces: cloudfront__s3.oac-signed-origin-fetch-without-bucket-policy-grant-forbidden, cloudfront__s3.oac-grant-scoped-to-another-distribution-arn-forbidden, cloudfront__s3.oac-origin-tenant-directory-path-rejected-until-list-reveals-no-such-key, cloudfront__s3.invalidation-refreshes-once-and-the-edge-re-caches-the-updated-object, cloudfront__s3.viewer-request-function-edge-runs-live-code-until-an-edit-is-published.

Cause 2 of 4: ViewerProtocolPolicy=redirect-to-https is not applied — plain HTTP is answered 403 with no Location.
AWS: a cache behavior with ViewerProtocolPolicy=redirect-to-https answers a plain-HTTP viewer request with 301 and a Location header holding the same URL over HTTPS (host and path copied from the request; here https://<distribution domain>/index.html).
Emulator: the plain-HTTP request is answered 403 and no Location header at all.
Why: every client that follows the redirect (browsers, curl -L, SDK-less fetchers) is refused instead of upgraded, so an http:// link to a distribution is dead where AWS would upgrade it.
Fix: viewer-protocol-policy handling on the request path of the cloudfront edge handler (301 + Location before any authorization decision).
Traces: cloudfront__s3.e2e-invalidate-cloudfront-cache-lifecycle.

Cause 3 of 4: a CreateDistribution replay under a used CallerReference that changes only Logging.Prefix is refused instead of returning the original distribution.
AWS: replaying CreateDistribution under an already-used CallerReference with only the Logging prefix changed returns the ORIGINAL distribution, with its original prefix, and mints nothing new (the caller must follow up with UpdateDistribution to change logging).
Emulator: the replay is refused 409 DistributionAlreadyExists — "The caller reference that you are using to create a distribution is associated with another distribution. Already exists: <existing distribution id>" — so the check aborts before it can read the distribution back.
Why: a retried deploy that happens to edit its logging prefix fails hard on the emulator where AWS returns the existing distribution, so idempotent re-deploys break.
Fix: CreateDistribution's caller-reference replay comparison (which members are significant for replay) in the cloudfront CreateDistribution handler.
Traces: cloudfront__s3.cloudfront-caller-reference-replay-changing-only-logging-returns-the-original-unchanged.

Cause 4 of 4: UpdateDistribution accepts turning standard logging on into an ACL-disabled (BucketOwnerEnforced) bucket.
AWS: enabling Logging into a bucket with ObjectOwnership=BucketOwnerEnforced is refused 400 InvalidArgument (Smithy: UpdateDistribution error InvalidArgument, httpError 400) with "The S3 bucket that you specified for CloudFront logs does not enable ACL access: <logging bucket domain>" — fixed text plus the Logging.Bucket value from the request. Logging stays off and the ETag is not rotated; the identical update naming a BucketOwnerPreferred bucket succeeds.
Emulator: the update SUCCEEDS (no error raised), so logging is configured into a bucket that can never receive the logs.
Why: the operator believes access logging is on while nothing can ever be delivered — exactly the misconfiguration AWS refuses at the API.
Fix: Logging-target validation (bucket ObjectOwnership/ACL access) in the cloudfront UpdateDistribution handler, shared with CreateDistribution.
Traces: cloudfront__s3.cloudfront-update-enabling-logging-into-an-acl-disabled-bucket-is-rejected.

## Evidence
Cause 1: five checks end with "Error: Check timed out after 60.0s" while polling the distribution domain for any response (expected 200 with the object, or S3's 403 AccessDenied / 404 NoSuchKey through the edge).
Cause 2: "AssertionError: [phase 1/7 deploy] plain HTTP answered 403 -> None; redirect-to-https should 301 to HTTPS" — expected (301, 'https://<distribution domain>/index.html').
Cause 3: "botocore.errorfactory.DistributionAlreadyExists: An error occurred (DistributionAlreadyExists) when calling the CreateDistribution operation: The caller reference that you are using to create a distribution is associated with another distribution. Already exists: <id>" — expected the original distribution returned unchanged.
Cause 4: "AssertionError: the call succeeded; real AWS rejects it, so there is no error" — expected ('InvalidArgument', 'The S3 bucket that you specified for CloudFront logs does not enable ACL access: <bucket>.s3.amazonaws.com').

## Steps to Reproduce
1. Cause 1: create a bucket, put an object, create a distribution with an OAC over that S3 origin, wait for Deployed, GET the distribution domain -> expect 200 (granted) or S3's 403/404 through the edge; observed no response at all.
2. Cause 2: create an enabled distribution whose default cache behavior uses ViewerProtocolPolicy=redirect-to-https and GET http://<domain>/index.html -> expect 301 with Location https://<domain>/index.html; observed 403 with no Location.
3. Cause 3: CreateDistribution with CallerReference R and Logging prefix "a/", then CreateDistribution again with R and only the prefix changed to "b/" -> expect the original distribution (original prefix) returned; observed 409 DistributionAlreadyExists.
4. Cause 4: create two buckets (ObjectOwnership BucketOwnerEnforced and BucketOwnerPreferred) and a distribution with logging off, then UpdateDistribution enabling Logging into the enforced bucket -> expect 400 InvalidArgument and logging still off under the same ETag; observed success.
