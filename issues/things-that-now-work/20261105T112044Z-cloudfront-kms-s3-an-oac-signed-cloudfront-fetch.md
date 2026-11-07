Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied
Issue-Key: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied|_poll_until#30|Timeout|time.sleep(interval_s)
Issue-Commit: c1082b91874a267e16e5e882b92de2a31503a88d

# cloudfront__kms__s3: an OAC-signed CloudFront fetch of an SSE-KMS object never completes, so the key-policy denial check hangs to its 60s deadline

> _Note: this issue was identified against the code base at commit `c1082b91874a267e16e5e882b92de2a31503a88d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: the only failing check of this group makes no progress against the emulator and is killed at its deadline — the OAC-protected CloudFront edge over an SSE-KMS encrypted S3 origin never converges to a state the check can observe. Failing check: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied.

AWS: with an Origin Access Control attached and the managed CachingDisabled policy, every viewer request reaches S3 and the result of a KMS key-policy change is visible on the very next request: while the CMK's key policy does not name the cloudfront.amazonaws.com service principal (with the distribution ARN in `aws:SourceArn`), the signed origin fetch is refused and the edge answers the viewer an error (403-class), and once the key policy grants it the same request serves the object. Reaching that point requires the distribution to become reachable at all: CreateDistribution answers HTTP 201 and the distribution progresses to Deployed, after which its domain name serves requests. Recorded behavior only for the timing: the check polls for the distribution to serve and the emulator never answers, so no status or error code was produced to compare.

Emulator: the check produces no response to assert on and ends with `Check timed out after 60.0s`; the emulator never serves the distribution's domain, so neither the denied nor the granted outcome is ever observable. No HTTP status, error code or message was returned for the diagnosis to quote.

Why: a client cannot use CloudFront over an SSE-KMS origin at all through this emulator — the edge never serves, so the KMS authorization boundary that OAC exists to express (the distribution's own ARN must be granted on the CMK) cannot be exercised or verified, and a caller debugging a 403 has nothing to observe.

Fix: the CloudFront distribution lifecycle and its S3-origin data plane must make an OAC-attached distribution reachable and must perform the signed origin fetch (and the KMS Decrypt it implies) so the key-policy outcome is observable (the CloudFront distribution handler/state and the edge request path in services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src).

Open issues 20261102T102137Z-cloudfront-s3-four-causes-the-s3-origin-edge.md and 20261029T175823Z-cloudfront-s3-an-oac-protected-s3-origin-never.md describe an unreachable OAC S3 origin edge under the cloudfront__s3 grouping and do not carry this check's replay key; this is the KMS-encrypted variant under its own service heading.

Traces: see trace_ids.

## Evidence
Expected (AWS): after CreateDistribution (HTTP 201) with an OAC and CachingDisabled, the distribution's domain serves viewer requests; with the CMK key policy not naming cloudfront.amazonaws.com the request is refused at the edge, and after the key policy grants that principal for the distribution ARN the same request returns the object's bytes.

Actual (emulator): `Error: Check timed out after 60.0s` for cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied — the check never obtains a response from the distribution, so it produced no status, error code or message.

## Steps to Reproduce
1. Create a CMK, a bucket with SSE-KMS default encryption, and put an object.
2. Create an Origin Access Control and a distribution over that bucket with the managed CachingDisabled cache policy; grant the OAC in the bucket policy but do NOT name cloudfront.amazonaws.com in the CMK key policy.
3. Request the object through the distribution's domain name -> the emulator never serves a response; the check is killed at 60s.
4. Re-replay cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied to confirm.
