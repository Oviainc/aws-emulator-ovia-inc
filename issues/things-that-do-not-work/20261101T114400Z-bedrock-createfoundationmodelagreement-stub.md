Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.create-foundation-model-agreement-conflict
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.create-foundation-model-agreement-conflict|run#8|AssertionError|assert code == "AccessDeniedException", (
Issue-Commit: 9d2e170c3a479b68290b0c2532ee0534a60c9186

# Implement bedrock CreateFoundationModelAgreement: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `9d2e170c3a479b68290b0c2532ee0534a60c9186`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFoundationModelAgreement

## Description
`CreateFoundationModelAgreement` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_foundation_model_agreement.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateFoundationModelAgreement` call, so each one unblocks only when `CreateFoundationModelAgreement` works: `bedrock.create-foundation-model-agreement-conflict`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/create_foundation_model_agreement.rs`. Replaying the corpus, the emulator refused the `CreateFoundationModelAgreement` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_bedrock_create_foundation_model_agreement_conflict.py", line 65, in run
    assert code == "AccessDeniedException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected CreateFoundationModelAgreement to be refused with AccessDeniedException for this account class, got 'NotImplemented' ('operation CreateFoundationModelAgreement is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the bedrock emulator
Call CreateFoundationModelAgreement and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
