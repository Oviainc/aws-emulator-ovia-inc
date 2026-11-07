Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied
Issue-Key: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied|_poll_until#30|Timeout|time.sleep(interval_s)
Issue-Commit: 547f35aa751bdfccaa69888f930e73773cf7bea8

# cloudfront: an OAC-protected S3 origin never serves at the edge, so the SSE-KMS key-policy denial contract over CloudFront is never exercised

> _Note: this issue was identified against the code base at commit `547f35aa751bdfccaa69888f930e73773cf7bea8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: a distribution whose single origin is an S3 bucket reached through an Origin Access Control never answers a viewer GET with the origin's object, so the whole KMS-key-policy-over-OAC flow cannot run and the check exhausts its time budget at its first barrier.

AWS: recorded behavior only (the CloudFront edge is a data plane, not a modelled API). With an OAC (OriginAccessControlOriginType=s3, SigningBehavior=always, SigningProtocol=sigv4) attached to an S3 origin, the managed CachingDisabled policy, and a bucket policy allowing s3:GetObject to Service cloudfront.amazonaws.com conditioned on AWS:SourceArn = the distribution ARN, the recording expects: (beat 1) a GET of the SSE-S3 object through the distribution domain answers HTTP 200 with the exact object bytes; (beat 2) in the same instant a GET of the SSE-KMS object answers HTTP 403 carrying S3's own error through the edge — Code 'AccessDenied', Message 'Access Denied' (both fixed text) — and leaks none of the object bytes, because the CMK's key policy names no cloudfront.amazonaws.com principal; (beat 4) after PutKeyPolicy admits cloudfront.amazonaws.com for that distribution ARN the same GET answers 200 with the object bytes; (beat 5) re-scoping the grant to another distribution's ARN makes the edge refuse again with AccessDenied.

Emulator: the check never gets past beat 1 — it is killed by its 60 s budget while polling the edge for a 200 on the SSE-S3 object (the check's own poll allows 240 s and raises its DATAPLANE_UNIMPLEMENTED marker on timeout). No edge response for the OAC-protected S3 origin is observed at all, so neither the AccessDenied refusal nor the repair nor the SourceArn scoping is reachable. Hypothesis: this is the same missing CloudFront S3-origin data plane already reported for the cloudfront__s3 service (open issues 20261024T182555Z-cloudfront-s3-distributions-over-s3-origins-serve-nothing.md and 20261029T175823Z-cloudfront-s3-an-oac-protected-s3-origin-never.md); neither carries this check's replay key and neither belongs to this service token.

Why: a client that fronts an SSE-KMS bucket with CloudFront + OAC cannot observe the single most important behaviour of that setup — that the edge serves the object only while the CMK's key policy admits cloudfront.amazonaws.com for exactly that distribution. Both the allow and the deny are invisible, so a misconfigured key policy (or one scoped to the wrong distribution) is indistinguishable from a correct one.

Fix: the CloudFront edge/data-plane handler for S3 origins — a viewer request must be resolved against the distribution's origin, signed per the attached OAC, authorized by the bucket policy and the CMK key policy, and the origin's response (including S3's 403 AccessDenied body) returned to the viewer.

Traces: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied.

## Evidence
Check cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied [mismatch].

Expected (beat 1 barrier): GET https://<distribution-domain>/<sse-s3-key> -> HTTP 200 with the uploaded bytes. Then (beat 2) GET https://<distribution-domain>/<sse-kms-key> -> HTTP 403 with S3 error Code='AccessDenied' and Message='Access Denied' and no object bytes in the body; (beat 4) after PutKeyPolicy admitting cloudfront.amazonaws.com with Condition AWS:SourceArn=<distribution ARN> -> HTTP 200 with the SSE-KMS bytes; (beat 5) after re-scoping that grant to another distribution ARN -> HTTP 403 AccessDenied again.

Actual: "Error: Check timed out after 60.0s" — the check is still polling the edge for the first 200 when its budget expires; no edge 200 and no edge 403 is ever observed for the OAC-protected S3 origin.

## Steps to Reproduce
1) CreateKey a CMK whose key policy names only the account root; CreateBucket and set its default encryption to that CMK; PutObject one SSE-KMS object and one SSE-S3 (AES256) object.
2) CreateOriginAccessControl(OriginAccessControlOriginType='s3', SigningBehavior='always', SigningProtocol='sigv4').
3) CreateDistribution with one S3 origin (DomainName=<bucket>.<s3 endpoint host>, S3OriginConfig.OriginAccessIdentity='', OriginAccessControlId=<oac>), DefaultCacheBehavior using managed CachingDisabled (4135ea2d-6df8-44a3-9df3-4b5a84be39ad), CustomErrorResponses ErrorCode 403 with ErrorCachingMinTTL 0.
4) PutBucketPolicy allowing s3:GetObject to Service cloudfront.amazonaws.com with Condition StringEquals AWS:SourceArn=<distribution ARN>.
5) Wait for the distribution to report Deployed, then GET https://<distribution-domain>/<sse-s3-key>: no 200 is ever served.
