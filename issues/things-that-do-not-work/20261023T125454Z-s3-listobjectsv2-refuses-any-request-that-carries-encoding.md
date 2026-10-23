Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cli-s3-sync-cloudfront-invalidate-workflow
Issue-Key: apigateway__cloudfront__s3.cli-s3-sync-cloudfront-invalidate-workflow|_run_shell#13|AssertionError|raise AssertionError(
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# S3 ListObjectsV2 refuses any request that carries encoding-type, so `aws s3 sync` cannot list a bucket

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjectsV2

## Description
Cause 1 of 1 — ListObjectsV2 answers NotImplemented whenever the request carries the encoding-type parameter, which the AWS CLI sets on every `s3 sync` / `s3 ls` listing.
AWS: ListObjectsV2 accepts EncodingType=url as an optional query parameter and returns the normal ListBucketResult with the key names percent-encoded; it is never an error.
Emulator: HTTP error (NotImplemented) 'operation ListObjectsV2 with encoding-type is scaffolded but not implemented in this emulator'. The operation itself is implemented — only this parameter path is missing, so no per-operation stub issue covers it (there is no open ListObjectsV2 issue).
Why: every high-level CLI workflow against S3 fails at the first listing (`aws s3 sync` exits 1 with 'fatal error'), so any scenario that stages content into a bucket — here, content served through a CloudFront distribution and then invalidated — cannot run at all.
Fix: the encoding-type branch of the ListObjectsV2 handler (apply EncodingType to the returned key/prefix/delimiter values instead of refusing).
Traces: apigateway__cloudfront__s3.cli-s3-sync-cloudfront-invalidate-workflow.

## Evidence
AssertionError: Command failed (rc=1): cmd ['aws', 's3', 'sync', '/tmp/sim2real-cfs3inv-<id>', 's3://sim2real-cfs3inv-bkt-<id>/'] — stderr: fatal error: An error occurred (NotImplemented) when calling the ListObjectsV2 operation: operation ListObjectsV2 with encoding-type is scaffolded but not implemented in this emulator. Expected: the sync completes and the objects are listable.

## Steps to Reproduce
1. CreateBucket, write a file locally.
2. Run `aws s3 sync <local-dir> s3://<bucket>/` (the CLI sends ListObjectsV2 with encoding-type=url) — expect the sync to succeed, observe rc=1 with NotImplemented.
3. Equivalent direct call: ListObjectsV2(Bucket=<bucket>, EncodingType='url').
