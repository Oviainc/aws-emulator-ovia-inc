Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.submit-service-job-to-unknown-service-environment-fails
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.submit-service-job-to-unknown-service-environment-fails|run#6|AssertionError|assert code == "ClientException", (
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# Implement batch SubmitServiceJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitServiceJob

## Description
`SubmitServiceJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/submit_service_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `SubmitServiceJob` call, so each one unblocks only when `SubmitServiceJob` works: `batch.submit-service-job-to-unknown-service-environment-fails`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/submit_service_job.rs`. Replaying the corpus, the emulator refused the `SubmitServiceJob` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_batch_submit_service_job_to_unknown_service_environment_fails.py", line 48, in run
    assert code == "ClientException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ClientException for a non-existent jobQueue, got code='NotImplemented' message='operation SubmitServiceJob is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the batch emulator
Call SubmitServiceJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
