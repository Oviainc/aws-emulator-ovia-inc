Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.decrease-retention-below-minimum-fails kinesis.increase-retention-below-current-fails kinesis.increase-retention-period-success kinesis.increase-then-decrease-retention-period kinesis.retention-period-limits-record-visibility-window
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|IncreaseStreamRetentionPeriod|NotImplemented
Issue-Commit: d5c75e29b1e90f1499e967e91f7af40c6cd43d58

# Implement kinesis IncreaseStreamRetentionPeriod: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d5c75e29b1e90f1499e967e91f7af40c6cd43d58`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
IncreaseStreamRetentionPeriod

## Description
`IncreaseStreamRetentionPeriod` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/increase_stream_retention_period.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `IncreaseStreamRetentionPeriod` call, so each one unblocks only when `IncreaseStreamRetentionPeriod` works: `kinesis.decrease-retention-below-minimum-fails`, `kinesis.increase-retention-below-current-fails`, `kinesis.increase-retention-period-success`, `kinesis.increase-then-decrease-retention-period`, `kinesis.retention-period-limits-record-visibility-window`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/increase_stream_retention_period.rs`. Replaying the corpus, the emulator refused the `IncreaseStreamRetentionPeriod` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the IncreaseStreamRetentionPeriod operation: operation IncreaseStreamRetentionPeriod is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the kinesis emulator
Call IncreaseStreamRetentionPeriod and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
