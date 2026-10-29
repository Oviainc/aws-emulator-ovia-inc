Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__s3.cf-origin-access-control-blocks-direct-s3-access-rejected cloudfront__s3.obs-cloudfront-3-standard-log-fields-hit-miss-and-disabled-control-silent cloudfront__s3.origin-with-both-oac-and-oai-rejected
Issue-Key: cloudfront__s3.cf-origin-access-control-blocks-direct-s3-access-rejected|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: cloudfront__s3.obs-cloudfront-3-standard-log-fields-hit-miss-and-disabled-control-silent|run#22|AssertionError|assert off_logging == {"Enabled": False, "IncludeCookies": False, "Bucket": "", "Prefix": ""}, (
Issue-Key: cloudfront__s3.origin-with-both-oac-and-oai-rejected|run#19|AssertionError|raise AssertionError(
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# cloudfront__s3: an OAC-protected S3 origin never answers at the edge, a disabled Logging block keeps its Bucket/Prefix, and CreateDistribution accepts an origin with both OAC and OAI

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
This issue bundles 3 net-new root causes in the cloudfront__s3 behavioral group. Open issues 20261024T182555Z-cloudfront-s3-distributions-over-s3-origins-serve-nothing.md and 20261023T140640Z-cloudfront-s3-a-cfn-deployed-distribution-does-not.md own neighbouring checks but not the three replay keys below.

Cause 1 of 3: a distribution whose S3 origin is protected by an OAC serves nothing at its edge domain, so the "direct S3 access is denied, the edge path is allowed" contract cannot be observed.
AWS: with an OAC (SigningProtocol sigv4, SigningBehavior always) on the origin and a bucket policy granting cloudfront.amazonaws.com with the AWS:SourceArn condition, a GET through the distribution's DomainName returns 200 with the object bytes, while a direct GET to the S3 object URL returns 403 AccessDenied with S3's XML error body. Smithy models OriginAccessControlId on Origin and the OAC config shapes; the edge protocol is not modelled, so: recorded behavior only.
Emulator: the edge domain never answers at all; the check runs to `Check timed out after 60.0s`, so the 200-through-the-edge half of the contract is unreachable and the 403-direct half cannot be attributed to the OAC.
Why: OAC is the supported way to keep an S3 origin private. A client cannot verify that its bucket is reachable only through CloudFront, and cannot distinguish a correctly locked-down origin from a dead edge.
Fix: the CloudFront edge data-plane handler and its OAC request-signing path (host-based router entry for d<id>.cloudfront.net -> stored distribution -> signed origin fetch).
Traces: cloudfront__s3.cf-origin-access-control-blocks-direct-s3-access-rejected.

Cause 2 of 3: a DistributionConfig Logging block submitted with Enabled=false retains the Bucket and Prefix it was sent with.
AWS: when Logging.Enabled is false, CloudFront stores the logging block with Bucket and Prefix CLEARED — GetDistributionConfig reads back `{'Enabled': False, 'IncludeCookies': False, 'Bucket': '', 'Prefix': ''}`. The empty strings are the requirement; the bucket/prefix values a run happens to submit are not.
Emulator: the submitted values survive the disable — the read reports Enabled=False together with the Bucket (`<bucket>.s3.amazonaws.com`) and Prefix the request carried.
Why: a caller that disables standard logging and then round-trips the config (GetDistributionConfig -> edit -> UpdateDistribution, the only supported update path) re-submits a log destination CloudFront says is not configured, and a caller reading the config cannot tell whether a log bucket is still designated.
Fix: the DistributionConfig logging normalisation in CreateDistribution/UpdateDistribution's state write — clear Bucket and Prefix when Enabled is false.
Traces: cloudfront__s3.obs-cloudfront-3-standard-log-fields-hit-miss-and-disabled-control-silent.

Cause 3 of 3: CreateDistribution accepts an origin that declares BOTH an OriginAccessControlId and an S3OriginConfig OriginAccessIdentity.
AWS: Smithy: CreateDistribution models IllegalOriginAccessConfiguration with httpError 400 for exactly this case — the two origin-access mechanisms are mutually exclusive on one origin. The call is refused and no distribution is created.
Emulator: 201 Created; the distribution is stored with both an OAC id and an OAI on the same origin.
Why: the stored configuration is ambiguous — nothing says which mechanism signs the origin request — and the client gets no signal at create time, so the mistake surfaces later as unexplained origin 403s.
Fix: the origin-access validation in CreateDistribution (and UpdateDistribution) — reject when an origin carries both OriginAccessControlId and S3OriginConfig.OriginAccessIdentity, with 400 IllegalOriginAccessConfiguration.
Traces: cloudfront__s3.origin-with-both-oac-and-oai-rejected.

## Evidence
Cause 1, verbatim: cloudfront__s3.cf-origin-access-control-blocks-direct-s3-access-rejected — `Error: Check timed out after 60.0s`. The CreateBucket/PutObject/CreateOriginAccessControl/PutBucketPolicy/CreateDistribution calls in the same run all succeeded.

Cause 2, verbatim: cloudfront__s3.obs-cloudfront-3-standard-log-fields-hit-miss-and-disabled-control-silent — AssertionError: a disabled Logging block must be stored with Bucket and Prefix cleared, got {'Enabled': False, 'IncludeCookies': False, 'Bucket': 'sim2real-obs3-std-log-14e1573bce5c.s3.amazonaws.com', 'Prefix': 'cf-std/'}
Expected: {'Enabled': False, 'IncludeCookies': False, 'Bucket': '', 'Prefix': ''} (the bucket name and prefix shown are per-run values).

Cause 3, verbatim: cloudfront__s3.origin-with-both-oac-and-oai-rejected — AssertionError: CreateDistribution ACCEPTED an origin with both OAC 'E7IYCGPKTANSF3' and OAI 'EJZ50Z92T9DP30': created 'EZGEITDQ1HLFY0'
Expected: 400 IllegalOriginAccessConfiguration, no distribution created (the ids shown are per-run values).

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the cloudfront__s3 behavioral checks listed in trace_ids.
2. Cause 1: create a bucket with one object, create an OAC (sigv4/always), create a distribution with the bucket as origin and that OAC, put a bucket policy granting cloudfront.amazonaws.com with the AWS:SourceArn condition, wait for Deployed, then GET through the distribution DomainName and directly against the S3 object URL. Observe no response from the edge.
3. Cause 2: CreateDistribution with Logging={Enabled: false, IncludeCookies: false, Bucket: "<b>.s3.amazonaws.com", Prefix: "cf-std/"}, then GetDistributionConfig. Observe Bucket/Prefix still populated.
4. Cause 3: CreateDistribution with one origin that sets both OriginAccessControlId=<oac-id> and S3OriginConfig.OriginAccessIdentity=origin-access-identity/cloudfront/<oai-id>. Observe 201.
