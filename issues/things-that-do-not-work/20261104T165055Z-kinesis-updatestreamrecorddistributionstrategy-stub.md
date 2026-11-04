Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.update-stream-record-distribution-strategy-auto-ignores-partition-key
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|UpdateStreamRecordDistributionStrategy|NotImplemented
Issue-Commit: ffd5f693012d77fe631946d21242c3542f59dce1

# Implement kinesis UpdateStreamRecordDistributionStrategy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ffd5f693012d77fe631946d21242c3542f59dce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStreamRecordDistributionStrategy

## Description
`UpdateStreamRecordDistributionStrategy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/update_stream_record_distribution_strategy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateStreamRecordDistributionStrategy` call, so each one unblocks only when `UpdateStreamRecordDistributionStrategy` works: `kinesis.update-stream-record-distribution-strategy-auto-ignores-partition-key`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/update_stream_record_distribution_strategy.rs`. Replaying the corpus, the emulator refused the `UpdateStreamRecordDistributionStrategy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateStreamRecordDistributionStrategy operation: operation UpdateStreamRecordDistributionStrategy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kinesis emulator
Call UpdateStreamRecordDistributionStrategy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
