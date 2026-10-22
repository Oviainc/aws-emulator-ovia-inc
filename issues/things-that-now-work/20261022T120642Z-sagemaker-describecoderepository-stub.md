Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.describe-code-repository-not-found
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.describe-code-repository-not-found|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# Implement sagemaker DescribeCodeRepository: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCodeRepository

## Description
`DescribeCodeRepository` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/describe_code_repository.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribeCodeRepository` call, so each one unblocks only when `DescribeCodeRepository` works: `sagemaker.describe-code-repository-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/describe_code_repository.rs`. Replaying the corpus, the emulator refused the `DescribeCodeRepository` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sagemaker_describe_code_repository_not_found.py", line 36, in run
    assert code == "ValidationException", f"expected ValidationException, got {code}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException, got NotImplemented: 'operation DescribeCodeRepository is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the sagemaker emulator
Call DescribeCodeRepository and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
