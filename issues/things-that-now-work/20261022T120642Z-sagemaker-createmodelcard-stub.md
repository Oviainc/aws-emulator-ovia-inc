Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.create-model-card-duplicate-name-conflict-rejected sagemaker.create-model-card-export-job-card-not-found sagemaker.duplicate-model-card-creation-rejected sagemaker.model-card-create-describe-delete sagemaker.model-card-lifecycle-transitions sagemaker.model-card-unicode-content-round-trip sagemaker.model-card-unicode-content-roundtrip
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.create-model-card-duplicate-name-conflict-rejected|run#8|AssertionError|assert len(successes) == 1, f"expected exactly 1 winner, got {len(successes)} (results={results!r})"
Issue-Key: sagemaker|CreateModelCard|NotImplemented
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# Implement sagemaker CreateModelCard: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateModelCard

## Description
`CreateModelCard` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_model_card.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `CreateModelCard` call, so each one unblocks only when `CreateModelCard` works: `sagemaker.create-model-card-duplicate-name-conflict-rejected`, `sagemaker.create-model-card-export-job-card-not-found`, `sagemaker.duplicate-model-card-creation-rejected`, `sagemaker.model-card-create-describe-delete`, `sagemaker.model-card-lifecycle-transitions`, `sagemaker.model-card-unicode-content-round-trip`, `sagemaker.model-card-unicode-content-roundtrip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_model_card.rs`. Replaying the corpus, the emulator refused the `CreateModelCard` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sagemaker_create_model_card_duplicate_name_conflict_rejected.py", line 42, in run
    assert len(successes) == 1, f"expected exactly 1 winner, got {len(successes)} (results={results!r})"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: expected exactly 1 winner, got 0 (results=[('error', ClientError('An error occurred (NotImplemented) when calling the CreateModelCard operation: operation CreateModelCard is scaffolded but not implemented in this emulator')), ('error', ClientError('An error occurred (NotImplemented) when calling the CreateModelCard operation: operation CreateModelCard is scaffolded but not implemented in this emulator'))])

## Steps to Reproduce
Boot the sagemaker emulator
Call CreateModelCard and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
