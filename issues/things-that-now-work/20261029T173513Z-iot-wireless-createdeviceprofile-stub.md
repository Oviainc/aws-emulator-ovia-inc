Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotwireless.duplicate-dev-eui-rejected-within-account iotwireless.fuota-task-device-association-reflects-in-filtered-list iotwireless.network-analyzer-configuration-tracks-added-and-removed-devices iotwireless.resource-event-configuration-update-round-trip iotwireless.resource-log-level-override-and-reset-round-trip iotwireless.wireless-device-mac-version-must-match-device-profile
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iot-wireless|CreateDeviceProfile|NotImplemented
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# Implement iot-wireless CreateDeviceProfile: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeviceProfile

## Description
`CreateDeviceProfile` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iotwireless-iotwireless/src/ops/create_device_profile.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `CreateDeviceProfile` call, so each one unblocks only when `CreateDeviceProfile` works: `iot-wireless.duplicate-dev-eui-rejected-within-account`, `iot-wireless.fuota-task-device-association-reflects-in-filtered-list`, `iot-wireless.network-analyzer-configuration-tracks-added-and-removed-devices`, `iot-wireless.resource-event-configuration-update-round-trip`, `iot-wireless.resource-log-level-override-and-reset-round-trip`, `iot-wireless.wireless-device-mac-version-must-match-device-profile`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iotwireless-iotwireless/src/ops/create_device_profile.rs`. Replaying the corpus, the emulator refused the `CreateDeviceProfile` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDeviceProfile operation: operation CreateDeviceProfile is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iot-wireless emulator
Call CreateDeviceProfile and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
