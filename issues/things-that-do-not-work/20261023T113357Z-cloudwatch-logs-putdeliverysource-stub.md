Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected logs__s3.xsvc-logs-3-s3-access-log-delivery-denied-without-destination-grant
Issue-Key: cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected|run#11|AssertionError|assert code == "ValidationException", (
Issue-Key: cloudwatch-logs|PutDeliverySource|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement cloudwatch-logs PutDeliverySource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeliverySource

## Description
`PutDeliverySource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_delivery_source.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `PutDeliverySource` call, so each one unblocks only when `PutDeliverySource` works: `cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected`, `logs__s3.xsvc-logs-3-s3-access-log-delivery-denied-without-destination-grant`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_delivery_source.rs`. Replaying the corpus, the emulator refused the `PutDeliverySource` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cloudfront__logs_xsvc_cloudfront_3_access_logs_delivery_source_outside_us_east_1_rejected.py", line 104, in run
    assert code == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: PutDeliverySource(ACCESS_LOGS) for a CloudFront distribution in us-west-2 must be rejected with ValidationException, got 'NotImplemented': 'operation PutDeliverySource is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the cloudwatch-logs emulator
Call PutDeliverySource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
