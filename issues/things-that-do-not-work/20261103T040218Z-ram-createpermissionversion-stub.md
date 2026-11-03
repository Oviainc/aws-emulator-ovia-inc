Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ram.create-permission-version-disallowed-action-rejected
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram.create-permission-version-disallowed-action-rejected|run#9|AssertionError|assert code == "InvalidPolicyException", f"expected InvalidPolicyException, got {code}: {message!r}"
Issue-Commit: 5e177a4f4d28f6dc4714f734dc35ed5ad192c177

# Implement ram CreatePermissionVersion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5e177a4f4d28f6dc4714f734dc35ed5ad192c177`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePermissionVersion

## Description
`CreatePermissionVersion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/create_permission_version.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreatePermissionVersion` call, so each one unblocks only when `CreatePermissionVersion` works: `ram.create-permission-version-disallowed-action-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/create_permission_version.rs`. Replaying the corpus, the emulator refused the `CreatePermissionVersion` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_ram_create_permission_version_disallowed_action_rejected.py", line 59, in run
    assert code == "InvalidPolicyException", f"expected InvalidPolicyException, got {code}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected InvalidPolicyException, got NotImplemented: 'operation CreatePermissionVersion is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the ram emulator
Call CreatePermissionVersion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
