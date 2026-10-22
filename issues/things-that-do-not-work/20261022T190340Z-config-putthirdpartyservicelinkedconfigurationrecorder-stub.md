Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.put-third-party-service-linked-recorder-invalid-principal-rejected
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.put-third-party-service-linked-recorder-invalid-principal-rejected|run#8|AssertionError|assert control_code == "ValidationException", (
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Implement config PutThirdPartyServiceLinkedConfigurationRecorder: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutThirdPartyServiceLinkedConfigurationRecorder

## Description
`PutThirdPartyServiceLinkedConfigurationRecorder` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_third_party_service_linked_configuration_recorder.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PutThirdPartyServiceLinkedConfigurationRecorder` call, so each one unblocks only when `PutThirdPartyServiceLinkedConfigurationRecorder` works: `config.put-third-party-service-linked-recorder-invalid-principal-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_third_party_service_linked_configuration_recorder.rs`. Replaying the corpus, the emulator refused the `PutThirdPartyServiceLinkedConfigurationRecorder` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_config_put_third_party_service_linked_recorder_invalid_principal_rejected.py", line 49, in run
    assert control_code == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected the well-formed-but-unrecognized control principal to also be rejected with ValidationException, got 'NotImplemented' ('operation PutThirdPartyServiceLinkedConfigurationRecorder is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the config emulator
Call PutThirdPartyServiceLinkedConfigurationRecorder and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
