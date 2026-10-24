Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-managed-integrations.connector-destination-referential-integrity
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations.connector-destination-referential-integrity|run#7|AssertionError|assert missing_code == "ValidationException", (
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement iot-managed-integrations CreateConnectorDestination: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnectorDestination

## Description
`CreateConnectorDestination` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/create_connector_destination.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateConnectorDestination` call, so each one unblocks only when `CreateConnectorDestination` works: `iot-managed-integrations.connector-destination-referential-integrity`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/create_connector_destination.rs`. Replaying the corpus, the emulator refused the `CreateConnectorDestination` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_iot_managed_integrations_connector_destination_referential_integrity.py", line 54, in run
    assert missing_code == "ValidationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ValidationException for a bogus CloudConnectorId, got NotImplemented: operation CreateConnectorDestination is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iot-managed-integrations emulator
Call CreateConnectorDestination and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
