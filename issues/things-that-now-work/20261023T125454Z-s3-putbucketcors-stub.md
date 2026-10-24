Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudfront__s3.cli-s3-cors-cloudfront-preflight-workflow
Issue-Key: apigateway__cloudfront__s3.cli-s3-cors-cloudfront-preflight-workflow|_run_shell#14|AssertionError|raise AssertionError(
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement s3 PutBucketCors: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketCors

## Description
`PutBucketCors` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3-amazons3/src/ops/put_bucket_cors.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PutBucketCors` call, so each one unblocks only when `PutBucketCors` works: `apigateway__cloudfront__s3.cli-s3-cors-cloudfront-preflight-workflow`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3-amazons3/src/ops/put_bucket_cors.rs`. Replaying the corpus, the emulator refused the `PutBucketCors` call: File "check_apigateway__cloudfront__s3_cli_s3_cors_cloudfront_preflight_workflow.py", line 173, in _run_aws
    proc = _run_shell(ctx, ["aws", "--output", "json", *args], timeout_s=timeout_s, check=check)
  File "check_apigateway__cloudfront__s3_cli_s3_cors_cloudfront_preflight_workflow.py", line 165, in _run_shell
    raise AssertionError(
    ...<2 lines>...
    )
AssertionError: Command failed (rc=254):
  cmd: ['aws', '--output', 'json', 's3api', 'put-bucket-cors', '--bucket', 'sim2real-cfcors-bkt-c238240e51b8', '--cors-configuration', 'file:///tmp/tmpmbxietok.json']
  stderr: 
aws: [ERROR]: An error occurred (NotImplemented) when calling the PutBucketCors operation: operation PutBucketCors is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3 emulator
Call PutBucketCors and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
