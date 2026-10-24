Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.update-nonexistent-marketplace-endpoint-rejected
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.update-nonexistent-marketplace-endpoint-rejected|run#13|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# Implement bedrock UpdateMarketplaceModelEndpoint: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateMarketplaceModelEndpoint

## Description
`UpdateMarketplaceModelEndpoint` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/update_marketplace_model_endpoint.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateMarketplaceModelEndpoint` call, so each one unblocks only when `UpdateMarketplaceModelEndpoint` works: `bedrock.update-nonexistent-marketplace-endpoint-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/update_marketplace_model_endpoint.rs`. Replaying the corpus, the emulator refused the `UpdateMarketplaceModelEndpoint` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_bedrock_update_nonexistent_marketplace_endpoint_rejected.py", line 59, in run
    assert code == "ResourceNotFoundException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected the update to be rejected with ResourceNotFoundException, got 'NotImplemented': operation UpdateMarketplaceModelEndpoint is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the bedrock emulator
Call UpdateMarketplaceModelEndpoint and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
