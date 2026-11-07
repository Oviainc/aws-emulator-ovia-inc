Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__s3.cloudfront-standard-logging-grants-awslogsdelivery-full-control-on-the-bucket
Issue-Key: cloudfront__s3.cloudfront-standard-logging-grants-awslogsdelivery-full-control-on-the-bucket|_poll#11|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# Enabling CloudFront standard logging does not add the awslogsdelivery FULL_CONTROL grant to the target bucket's ACL

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
One root cause, grounded in a failing check this report does NOT mark ALREADY FILED or OWNED. Open issue 20261102T102137Z-cloudfront-s3-four-causes-the-s3-origin-edge.md owns other checks of this service and does not carry this replay key.

Cause 1 of 1: the log-delivery grantee is never added to the logging bucket's ACL.
AWS: when a distribution is created or updated with Logging.Enabled=true and Logging.Bucket set, CloudFront adds a Grant to that bucket's ACL for the awslogsdelivery canonical user (Grantee of type CanonicalUser, Permission FULL_CONTROL), in addition to the bucket owner's own grant. GetBucketAcl then reports two grants: the owner's and awslogsdelivery's. The canonical user id of the log-delivery account is a fixed AWS-published value, not a per-run value; the owner id is the caller's.
Emulator: GetBucketAcl keeps reporting exactly one grant — the bucket owner's own FULL_CONTROL — after logging is enabled; the check polls 60 s and reports "the awslogsdelivery grant to appear not met within 60.0s. Last result: [(<owner-canonical-id>, 'FULL_CONTROL')]". The single id in that last result is the bucket owner's, i.e. the awslogsdelivery grantee was never appended.
Why: a client that checks the bucket ACL to confirm CloudFront can write access logs (the documented way to diagnose missing logs, and what hardening/compliance tooling asserts) sees a bucket that CloudFront cannot write to, and cannot tell a mis-wired distribution from a correctly wired one.
Fix: the CloudFront CreateDistribution/UpdateDistribution handler's logging-configuration side effect — the step that writes the awslogsdelivery CanonicalUser FULL_CONTROL grant into the target bucket's ACL (and removes it when logging is disabled).
Traces: cloudfront__s3.cloudfront-standard-logging-grants-awslogsdelivery-full-control-on-the-bucket.

## Evidence
Expected: after enabling standard logging, GetBucketAcl on the log bucket lists an additional CanonicalUser grant for awslogsdelivery with Permission FULL_CONTROL. Actual: the ACL still lists only the bucket owner's FULL_CONTROL grant — 'AssertionError: the awslogsdelivery grant to appear not met within 60.0s. Last result: [(<owner-id>, 'FULL_CONTROL')]'.

## Steps to Reproduce
1) Create an ACL-enabled bucket to receive logs.
2) CreateDistribution (or UpdateDistribution) with Logging {Enabled: true, Bucket: <bucket>.s3.amazonaws.com, Prefix: 'cf-std/'}.
3) GetBucketAcl on the log bucket and look for the awslogsdelivery CanonicalUser FULL_CONTROL grant.
