Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.managed-instance-role-invalid-instance-rejected ssm.managed-instance-role-update-persists
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.managed-instance-role-invalid-instance-rejected|run#9|AssertionError|assert err["Code"] == "InvalidInstanceId", (
Issue-Key: ssm.managed-instance-role-update-persists|run#19|AssertionError|assert err["Code"] == "InvalidInstanceId", (
Issue-Commit: b58e513c3cf15b473d4f74499e96e777c32d6275

# Implement ssm UpdateManagedInstanceRole: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b58e513c3cf15b473d4f74499e96e777c32d6275`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateManagedInstanceRole

## Description
`UpdateManagedInstanceRole` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssm-amazonssm/src/ops/update_managed_instance_role.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `UpdateManagedInstanceRole` call, so each one unblocks only when `UpdateManagedInstanceRole` works: `ssm.managed-instance-role-invalid-instance-rejected`, `ssm.managed-instance-role-update-persists`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssm-amazonssm/src/ops/update_managed_instance_role.rs`. Replaying the corpus, the emulator refused the `UpdateManagedInstanceRole` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_ssm_managed_instance_role_invalid_instance_rejected.py", line 35, in run
    assert err["Code"] == "InvalidInstanceId", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: UpdateManagedInstanceRole against a nonexistent hybrid instance id should be rejected with InvalidInstanceId, got {'Message': 'operation UpdateManagedInstanceRole is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the ssm emulator
Call UpdateManagedInstanceRole and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
