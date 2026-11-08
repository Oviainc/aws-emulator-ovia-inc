Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__s3.cf-origin-access-control-blocks-direct-s3-access-rejected
Issue-Key: cloudfront__s3.cf-origin-access-control-blocks-direct-s3-access-rejected|_poll_until#22|Timeout|raise AssertionError(f"timed out waiting for {description} after {timeout_s}s (last: {last!r})")
Issue-Commit: 18c9e77d26f229766ffd8c87453fda7296284d7c

# cloudfront__s3: an OAC-locked S3 origin is never served through the distribution's edge domain, so the anonymous-direct-to-S3 403 AccessDenied contract is never reached

> _Note: this issue was identified against the code base at commit `18c9e77d26f229766ffd8c87453fda7296284d7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CloudFront edge data plane (viewer GET of an OAC-signed S3 origin)

## Description
One net-new root cause, grounded in one failing check that the replay report marks neither ALREADY FILED nor OWNED.

Cause 1 of 1: a Deployed distribution whose single origin is an OAC-locked S3 bucket never answers a viewer GET at its own DomainName, so the check's first positive control never completes and the OAC contract (edge reads the object, anonymous direct-to-S3 is refused) is never exercised.

AWS: The CloudFront Smithy model (com.amazonaws.cloudfront) models only the control plane (CreateOriginAccessControl, CreateDistribution, GetDistribution); the edge request path is not modeled, so the edge behaviour is recorded behavior only. Recorded: with an S3 origin referenced by OriginAccessControlId whose OAC has SigningBehavior=always/SigningProtocol=sigv4, a bucket policy granting Principal {"Service": "cloudfront.amazonaws.com"} s3:GetObject under Condition StringEquals AWS:SourceArn = <the distribution's ARN>, and the bucket's public access fully blocked, a plain HTTP GET of https://<DomainName>/<key> is answered 200 with the object's exact bytes once GetDistribution reports Status == "Deployed" — CloudFront signs the origin request with SigV4 as the cloudfront.amazonaws.com service principal. The negative half is S3's own documented refusal: an unsigned, credential-free GET of https://s3.<region>.amazonaws.com/<bucket>/<key> is answered HTTP 403 with an S3 Error XML body whose <Code> is "AccessDenied" and whose <Message> is "Access Denied", and whose body carries none of the object's bytes. Both of those strings are entirely fixed text with no variable parts; the bucket, key and region in the URL are values taken from the request.

Emulator: the edge serves nothing. The check completes its setup — CreateBucket, PutPublicAccessBlock (all four flags true), PutObject, CreateOriginAccessControl, CreateDistribution with CachePolicyId 4135ea2d-6df8-44a3-9df3-4b5a84be39ad (managed CachingDisabled), PutBucketPolicy, poll to Status == Deployed — and then polls https://<DomainName>/asset.txt for a 200 every 5 s for up to 180 s. No 200 ever arrives, the harness's 180 s check budget is exhausted inside that poll, and the check is killed with "Check timed out after 180.0s" before its own DATAPLANE_UNIMPLEMENTED assertion or the anonymous-S3 negative can run. The two downstream assertions — the caller-credential GetObject positive control and the anonymous 403 AccessDenied "Access Denied" at the S3 REST endpoint — are therefore unverified on this path, not shown to pass. Related open issues describe the same edge gap for other check sets (20261102T102137Z-cloudfront-s3-four-causes-the-s3-origin-edge.md, 20261029T175823Z-cloudfront-s3-an-oac-protected-s3-origin-never.md, 20261024T182555Z-cloudfront-s3-distributions-over-s3-origins-serve-nothing.md); none of them carries this check's replay key and the report did not mark it OWNED, so it is filed here.

Why: OAC is the only supported way to lock an S3 origin to one distribution, and its whole value is the pair of outcomes — the edge can read, nobody else can. With no edge response, a client cannot distinguish "my OAC and bucket policy are correct" from "my origin is unreachable": the emulator gives no signal either way, so a misconfigured bucket policy or SourceArn condition passes review and the first real deployment 403s. It also means a security test that asserts direct S3 access is blocked can never run, so an over-permissive bucket is never caught.

Fix: the CloudFront edge data-plane request path for an S3 origin — route the distribution's DomainName host to a viewer-request handler, select the default cache behaviour, perform the SigV4-signed origin fetch as the cloudfront.amazonaws.com service principal (so the bucket policy's AWS:SourceArn condition is evaluated against the distribution ARN), and return the origin's 200 and bytes to the viewer; the S3 side must continue to refuse an unsigned request with 403 AccessDenied / "Access Denied" and no payload.

Traces: cloudfront__s3.cf-origin-access-control-blocks-direct-s3-access-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1: 326/584 passed (55.8%); service cloudfront__s3 5/22 (23%), 17 failed, of which 16 were marked OWNED by an open issue and skipped.

[mismatch] cloudfront__s3.cf-origin-access-control-blocks-direct-s3-access-rejected
  Error: Check timed out after 180.0s

Expected (check body): after _wait_distribution_deployed, _poll_until(cf_probe, r.status_code == 200, timeout_s=180, interval_s=5) returns a 200 whose content is b"sim2real-oac-protected-object"; then s3.get_object(Bucket, Key) returns the same bytes; then an unsigned requests.Session() (trust_env=False) GET of <s3 endpoint>/<bucket>/asset.txt returns status 403 whose Error XML gives (Code, Message) == ("AccessDenied", "Access Denied") and whose body does not contain the object's bytes.

Actual: the first poll never sees a 200 from the distribution's DomainName and consumes the entire 180 s check budget, so the harness terminates the check ("Check timed out after 180.0s") and the check's own DATAPLANE_UNIMPLEMENTED-prefixed AssertionError is never raised. The check is tagged plane:data / dp:endpoint and the URL is the DomainName the emulator itself minted for a distribution it reports as Deployed, so the missing response is the emulator's, not a transport or name-resolution problem.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay suite simparity-behavioral-aws-v1 (service cloudfront__s3).
3. Replay check cloudfront__s3.cf-origin-access-control-blocks-direct-s3-access-rejected: create an S3 bucket in the runner's region; PutPublicAccessBlock with BlockPublicAcls/IgnorePublicAcls/BlockPublicPolicy/RestrictPublicBuckets all true; PutObject asset.txt = b"sim2real-oac-protected-object"; CreateOriginAccessControl(OriginAccessControlOriginType="s3", SigningBehavior="always", SigningProtocol="sigv4"); CreateDistribution with one origin whose DomainName is <bucket>.s3.<region>.amazonaws.com, S3OriginConfig.OriginAccessIdentity = "" and OriginAccessControlId = that OAC, and DefaultCacheBehavior using managed CachingDisabled (4135ea2d-6df8-44a3-9df3-4b5a84be39ad); PutBucketPolicy allowing Principal {"Service":"cloudfront.amazonaws.com"} s3:GetObject on arn:aws:s3:::<bucket>/* with Condition StringEquals AWS:SourceArn = the distribution ARN; poll GetDistribution until Status == "Deployed".
4. HTTP GET https://<DomainName>/asset.txt with no credentials. Observe that no response is ever returned (the poll runs to the 180 s cap) instead of 200 with the object's exact bytes.
5. Then, for the negative half that this check never reaches: GET <s3 endpoint>/<bucket>/asset.txt with a credential-free client and confirm 403 with Error XML Code "AccessDenied", Message "Access Denied" and no object bytes in the body.
