Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.e2e-fine-tune-deploy-invoke-teardown bedrock.model-customization-job-lifecycle bedrock.model-customization-job-stop-transitions bedrock.model-customization-job-stop-transitions-to-stopped bedrock.stop-terminated-model-customization-job-is-an-idempotent-no-op bedrock.timestamp-precision-preserved-model-customization-job
Issue-Key: bedrock.e2e-fine-tune-deploy-invoke-teardown|run#16|AssertionError|assert code == "AccessDeniedException", (code, message)
Issue-Key: bedrock.model-customization-job-lifecycle|run#12|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code} ({message!r})"
Issue-Key: bedrock|CreateModelCustomizationJob|NotImplemented
Issue-Commit: 0de7718ade8f368be0e47aca52d0dd18a585e082

# Implement bedrock CreateModelCustomizationJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0de7718ade8f368be0e47aca52d0dd18a585e082`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateModelCustomizationJob

## Description
`CreateModelCustomizationJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_model_customization_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `CreateModelCustomizationJob` call, so each one unblocks only when `CreateModelCustomizationJob` works: `bedrock.e2e-fine-tune-deploy-invoke-teardown`, `bedrock.model-customization-job-lifecycle`, `bedrock.model-customization-job-stop-transitions`, `bedrock.model-customization-job-stop-transitions-to-stopped`, `bedrock.stop-terminated-model-customization-job-is-an-idempotent-no-op`, `bedrock.timestamp-precision-preserved-model-customization-job`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_model_customization_job.rs`. Replaying the corpus, the emulator refused the `CreateModelCustomizationJob` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_bedrock_e2e_fine_tune_deploy_invoke_teardown.py", line 68, in run
    assert code == "AccessDeniedException", (code, message)
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: ('NotImplemented', 'operation CreateModelCustomizationJob is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the bedrock emulator
Call CreateModelCustomizationJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
