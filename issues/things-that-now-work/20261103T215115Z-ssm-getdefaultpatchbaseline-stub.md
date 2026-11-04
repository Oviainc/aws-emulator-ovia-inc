Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.default-patch-baseline-register-and-update ssm.register-default-patch-baseline-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|GetDefaultPatchBaseline|NotImplemented
Issue-Commit: 69e3918bc598d03b2c669434388419738bf97909

# Implement ssm GetDefaultPatchBaseline: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `69e3918bc598d03b2c669434388419738bf97909`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDefaultPatchBaseline

## Description
`GetDefaultPatchBaseline` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssm-amazonssm/src/ops/get_default_patch_baseline.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `GetDefaultPatchBaseline` call, so each one unblocks only when `GetDefaultPatchBaseline` works: `ssm.default-patch-baseline-register-and-update`, `ssm.register-default-patch-baseline-nonexistent-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssm-amazonssm/src/ops/get_default_patch_baseline.rs`. Replaying the corpus, the emulator refused the `GetDefaultPatchBaseline` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetDefaultPatchBaseline operation: operation GetDefaultPatchBaseline is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ssm emulator
Call GetDefaultPatchBaseline and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
