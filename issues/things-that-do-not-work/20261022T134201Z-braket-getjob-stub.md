Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: braket.braket-malformed-job-arn-returns-validation-not-not-found
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket.braket-malformed-job-arn-returns-validation-not-not-found|run#7|AssertionError|assert control["Code"] == "ResourceNotFoundException", (
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# Implement braket GetJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJob

## Description
`GetJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-braket-braket/src/ops/get_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetJob` call, so each one unblocks only when `GetJob` works: `braket.braket-malformed-job-arn-returns-validation-not-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-braket-braket/src/ops/get_job.rs`. Replaying the corpus, the emulator refused the `GetJob` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_braket_braket_malformed_job_arn_returns_validation_not_not_found.py", line 35, in run
    assert control["Code"] == "ResourceNotFoundException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceNotFoundException for a well-formed but nonexistent jobArn, got {'Message': 'operation GetJob is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the braket emulator
Call GetJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
