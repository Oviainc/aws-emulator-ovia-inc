Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sso-admin.abac-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin.abac-configuration-lifecycle|run#5|AssertionError|assert create_code == "ValidationException", (
Issue-Commit: 692676341d77292a02c0c0941a8d422bd5873aeb

# Implement sso-admin CreateInstanceAccessControlAttributeConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `692676341d77292a02c0c0941a8d422bd5873aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInstanceAccessControlAttributeConfiguration

## Description
`CreateInstanceAccessControlAttributeConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_instance_access_control_attribute_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateInstanceAccessControlAttributeConfiguration` call, so each one unblocks only when `CreateInstanceAccessControlAttributeConfiguration` works: `sso-admin.abac-configuration-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_instance_access_control_attribute_configuration.rs`. Replaying the corpus, the emulator refused the `CreateInstanceAccessControlAttributeConfiguration` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_sso_admin_abac_configuration_lifecycle.py", line 32, in run
    assert create_code == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException from CreateInstanceAccessControlAttributeConfiguration on an account instance, got 'NotImplemented' ('operation CreateInstanceAccessControlAttributeConfiguration is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the sso-admin emulator
Call CreateInstanceAccessControlAttributeConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
