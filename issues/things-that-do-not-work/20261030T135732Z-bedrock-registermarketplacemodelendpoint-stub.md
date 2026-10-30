Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.register-marketplace-endpoint-invalid-arn-rejected bedrock.register-marketplace-endpoint-malformed-arn-rejected
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.register-marketplace-endpoint-invalid-arn-rejected|run#11|AssertionError|assert control_code == "ResourceNotFoundException", f"{control_code}: {control_message!r}"
Issue-Key: bedrock.register-marketplace-endpoint-malformed-arn-rejected|run#11|AssertionError|assert code_missing == "ResourceNotFoundException", (
Issue-Commit: 07e872c7593558b582731e73c173338530c40226

# Implement bedrock RegisterMarketplaceModelEndpoint: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `07e872c7593558b582731e73c173338530c40226`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterMarketplaceModelEndpoint

## Description
`RegisterMarketplaceModelEndpoint` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/register_marketplace_model_endpoint.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `RegisterMarketplaceModelEndpoint` call, so each one unblocks only when `RegisterMarketplaceModelEndpoint` works: `bedrock.register-marketplace-endpoint-invalid-arn-rejected`, `bedrock.register-marketplace-endpoint-malformed-arn-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/register_marketplace_model_endpoint.rs`. Replaying the corpus, the emulator refused the `RegisterMarketplaceModelEndpoint` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_bedrock_register_marketplace_endpoint_invalid_arn_rejected.py", line 57, in run
    assert control_code == "ResourceNotFoundException", f"{control_code}: {control_message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: NotImplemented: 'operation RegisterMarketplaceModelEndpoint is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the bedrock emulator
Call RegisterMarketplaceModelEndpoint and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
