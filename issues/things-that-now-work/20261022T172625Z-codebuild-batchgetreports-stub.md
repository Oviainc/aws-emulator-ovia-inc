Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.batch-get-reports-invalid-arn
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.batch-get-reports-invalid-arn|run#3|AssertionError|assert (code, message) == ("InvalidInputException", "Invalid report ARN"), (code, message)
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codebuild BatchGetReports: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetReports

## Description
`BatchGetReports` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/batch_get_reports.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `BatchGetReports` call, so each one unblocks only when `BatchGetReports` works: `codebuild.batch-get-reports-invalid-arn`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/batch_get_reports.rs`. Replaying the corpus, the emulator refused the `BatchGetReports` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_codebuild_batch_get_reports_invalid_arn.py", line 23, in run
    assert (code, message) == ("InvalidInputException", "Invalid report ARN"), (code, message)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: ('NotImplemented', 'operation BatchGetReports is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the codebuild emulator
Call BatchGetReports and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
