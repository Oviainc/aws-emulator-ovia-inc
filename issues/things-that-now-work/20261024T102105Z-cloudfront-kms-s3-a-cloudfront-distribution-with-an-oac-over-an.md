Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied
Issue-Key: cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied|_poll_until#30|Timeout|time.sleep(interval_s)
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# A CloudFront distribution with an OAC over an SSE-KMS/SSE-S3 bucket never serves anything at its edge domain, so the OAC key-policy check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1 — the distribution's edge dataplane never answers, so the OAC-plus-KMS authorization contract cannot be observed at all.
AWS: once a distribution with an S3 origin, an Origin Access Control (SigningBehavior 'always', sigv4) and a matching bucket policy for the cloudfront.amazonaws.com service principal reaches Deployed, a GET of the distribution's domain returns the origin object — HTTP 200 with the exact bytes for the SSE-S3 object. The object encrypted with a CMK whose key policy has NO cloudfront principal is then answered HTTP 403 with S3 error code AccessDenied and the object bytes are not leaked, while the SSE-S3 object keeps returning 200; after the key policy is rescoped the KMS object is served, and rescoping it away returns it to AccessDenied.
Emulator: the check never gets past its first barrier — repeated GETs of the distribution domain never yield HTTP 200 for the plain SSE-S3 object and the check ends as 'Check timed out after 60.0s' (its own barrier budget is 240 s, 5 s apart, and tolerates transport errors as 'still propagating'). No verdict was reached for any of the 403/AccessDenied assertions.
Why: nothing served through CloudFront can be exercised — a client cannot test an OAC setup, an origin bucket policy, or the KMS key-policy requirement that makes OAC-over-SSE-KMS work; the control plane reports a healthy, deployed distribution that delivers nothing.
Fix: the CloudFront edge dataplane (serving a distribution's configured S3 origin at its minted DomainName, including the OAC-signed origin request and the propagation of DistributionConfig/CustomErrorResponses). Distinct from the already-open '20261023T140640Z-cloudfront-iam-sts-cloudfront-performs-no-iam-authorization-an-assumed-role.md', which is about control-plane authorization, not the edge.

## Evidence
cloudfront__kms__s3.xsvc-cloudfront-2-oac-kms-key-policy-without-cloudfront-principal-denied: 'Error: Check timed out after 60.0s'. Before the hang the check verified PutObject reported ('aws:kms', <key arn>) and 'AES256' for its two objects, created an OriginAccessControl, created the distribution (DomainName returned), set the bucket policy allowing cloudfront.amazonaws.com with AWS:SourceArn=<dist arn>, and waited for Deployed; it then polled 'the SSE-S3 object to be served at <edge url>' for a 200 and never got one. Expected: HTTP 200 with the SSE-S3 bytes at the edge, then HTTP 403 with S3 Error Code 'AccessDenied' (and no object bytes in the body) for the SSE-KMS object.

## Steps to Reproduce
1. kms:CreateKey; s3:CreateBucket; PutObject one object with SSE-KMS under that key and one with SSE-S3. 2. cloudfront:CreateOriginAccessControl (s3, SigningBehavior 'always', sigv4). 3. cloudfront:CreateDistribution with the bucket as origin and that OAC. 4. s3:PutBucketPolicy allowing s3:GetObject to cloudfront.amazonaws.com with AWS:SourceArn=<distribution arn>. 5. Wait for Deployed, then HTTP GET '<distribution domain>/<sse-s3 key>' — the emulator never returns 200. 6. GET the SSE-KMS key — must be 403/AccessDenied.
