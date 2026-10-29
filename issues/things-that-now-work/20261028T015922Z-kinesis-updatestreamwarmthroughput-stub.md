Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.update-stream-warm-throughput-applied-to-describe kinesis.update-stream-warm-throughput-on-provisioned-stream-must-fail kinesis.update-stream-warm-throughput-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|UpdateStreamWarmThroughput|NotImplemented
Issue-Key: kinesis.update-stream-warm-throughput-on-provisioned-stream-must-fail|run#15|AssertionError|assert provisioned_code in ("InvalidArgumentException", "ValidationException"), (
Issue-Commit: 74fb8d07c8cc63563f3320f6510f3db7405456f9

# Implement kinesis UpdateStreamWarmThroughput: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `74fb8d07c8cc63563f3320f6510f3db7405456f9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStreamWarmThroughput

## Description
`UpdateStreamWarmThroughput` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/update_stream_warm_throughput.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `UpdateStreamWarmThroughput` call, so each one unblocks only when `UpdateStreamWarmThroughput` works: `kinesis.update-stream-warm-throughput-applied-to-describe`, `kinesis.update-stream-warm-throughput-on-provisioned-stream-must-fail`, `kinesis.update-stream-warm-throughput-out-of-range-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/update_stream_warm_throughput.rs`. Replaying the corpus, the emulator refused the `UpdateStreamWarmThroughput` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateStreamWarmThroughput operation: operation UpdateStreamWarmThroughput is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kinesis emulator
Call UpdateStreamWarmThroughput and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
