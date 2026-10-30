Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.backup-copy-job-cross-region-event-detail-fields backup__events.backup-job-eventbridge-detail-type-list-matches-multiple-states backup__events.copy-job-state-change-event-delivered backup__events.copy-job-state-change-event-triggers-rule
Issue-Key: backup|StartCopyJob|NotImplemented
Issue-Key: backup__events.copy-job-state-change-event-delivered|run#18|AssertionError|assert error_code == "ResourceNotFoundException", (
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Implement backup StartCopyJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCopyJob

## Description
`StartCopyJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/start_copy_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `StartCopyJob` call, so each one unblocks only when `StartCopyJob` works: `backup__events.backup-copy-job-cross-region-event-detail-fields`, `backup__events.backup-job-eventbridge-detail-type-list-matches-multiple-states`, `backup__events.copy-job-state-change-event-delivered`, `backup__events.copy-job-state-change-event-triggers-rule`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/start_copy_job.rs`. Replaying the corpus, the emulator refused the `StartCopyJob` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartCopyJob operation: operation StartCopyJob is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the backup emulator
Call StartCopyJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
