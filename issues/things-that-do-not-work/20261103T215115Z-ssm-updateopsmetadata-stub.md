Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.update-ops-metadata-invalid-argument-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.update-ops-metadata-invalid-argument-rejected|run#6|AssertionError|assert baseline_error.get("Code") == "OpsMetadataNotFoundException", (
Issue-Commit: 69e3918bc598d03b2c669434388419738bf97909

# Implement ssm UpdateOpsMetadata: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `69e3918bc598d03b2c669434388419738bf97909`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateOpsMetadata

## Description
`UpdateOpsMetadata` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssm-amazonssm/src/ops/update_ops_metadata.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateOpsMetadata` call, so each one unblocks only when `UpdateOpsMetadata` works: `ssm.update-ops-metadata-invalid-argument-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssm-amazonssm/src/ops/update_ops_metadata.rs`. Replaying the corpus, the emulator refused the `UpdateOpsMetadata` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_ssm_update_ops_metadata_invalid_argument_rejected.py", line 34, in run
    assert baseline_error.get("Code") == "OpsMetadataNotFoundException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected OpsMetadataNotFoundException for a well-formed update on a nonexistent OpsMetadataArn, got {'Message': 'operation UpdateOpsMetadata is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the ssm emulator
Call UpdateOpsMetadata and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
