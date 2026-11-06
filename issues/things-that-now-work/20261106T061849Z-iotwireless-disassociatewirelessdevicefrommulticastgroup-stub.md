Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotwireless.multicast-group-device-association-tracks-membership-count
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iot-wireless|DisassociateWirelessDeviceFromMulticastGroup|NotImplemented
Issue-Commit: 502370b3ab52e8f9258a465387106a9118ec6835

# Implement iotwireless DisassociateWirelessDeviceFromMulticastGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `502370b3ab52e8f9258a465387106a9118ec6835`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateWirelessDeviceFromMulticastGroup

## Description
`DisassociateWirelessDeviceFromMulticastGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iotwireless-iotwireless/src/ops/disassociate_wireless_device_from_multicast_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DisassociateWirelessDeviceFromMulticastGroup` call, so each one unblocks only when `DisassociateWirelessDeviceFromMulticastGroup` works: `iotwireless.multicast-group-device-association-tracks-membership-count`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iotwireless-iotwireless/src/ops/disassociate_wireless_device_from_multicast_group.rs`. Replaying the corpus, the emulator refused the `DisassociateWirelessDeviceFromMulticastGroup` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DisassociateWirelessDeviceFromMulticastGroup operation: operation DisassociateWirelessDeviceFromMulticastGroup is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iotwireless emulator
Call DisassociateWirelessDeviceFromMulticastGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
