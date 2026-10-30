Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.delete-custom-model-missing-conflict-not-notfound
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.delete-custom-model-missing-conflict-not-notfound|run#7|AssertionError|assert got == expected, f"expected {expected!r}, got {got!r}"
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# Implement bedrock DeleteCustomModel: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCustomModel

## Description
`DeleteCustomModel` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/delete_custom_model.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteCustomModel` call, so each one unblocks only when `DeleteCustomModel` works: `bedrock.delete-custom-model-missing-conflict-not-notfound`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/delete_custom_model.rs`. Replaying the corpus, the emulator refused the `DeleteCustomModel` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_bedrock_delete_custom_model_missing_conflict_not_notfound.py", line 38, in run
    assert got == expected, f"expected {expected!r}, got {got!r}"
           ^^^^^^^^^^^^^^^
AssertionError: expected ('ResourceNotFoundException', 'Custom model not found.'), got ('NotImplemented', 'operation DeleteCustomModel is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the bedrock emulator
Call DeleteCustomModel and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
