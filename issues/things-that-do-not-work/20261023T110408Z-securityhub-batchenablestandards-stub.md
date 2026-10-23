Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: securityhub.batch-enable-standards-invalid-arn-rejected securityhub.standards-lifecycle-enable-ready-disable-removed securityhub.update-standards-control-disable-requires-reason-rejected
Issue-Shape-Id: com.amazonaws.securityhub#SecurityHubAPIService
Issue-Key: securityhub.batch-enable-standards-invalid-arn-rejected|run#7|AssertionError|assert code == "InvalidInputException", f"expected InvalidInputException, got {code} ({message})"
Issue-Key: securityhub|BatchEnableStandards|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement securityhub BatchEnableStandards: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchEnableStandards

## Description
`BatchEnableStandards` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-securityhub-securityhubapiservice/src/ops/batch_enable_standards.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `BatchEnableStandards` call, so each one unblocks only when `BatchEnableStandards` works: `securityhub.batch-enable-standards-invalid-arn-rejected`, `securityhub.standards-lifecycle-enable-ready-disable-removed`, `securityhub.update-standards-control-disable-requires-reason-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-securityhub-securityhubapiservice/src/ops/batch_enable_standards.rs`. Replaying the corpus, the emulator refused the `BatchEnableStandards` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_securityhub_batch_enable_standards_invalid_arn_rejected.py", line 31, in run
    assert code == "InvalidInputException", f"expected InvalidInputException, got {code} ({message})"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected InvalidInputException, got NotImplemented (operation BatchEnableStandards is scaffolded but not implemented in this emulator)

## Steps to Reproduce
Boot the securityhub emulator
Call BatchEnableStandards and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
