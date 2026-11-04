Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotwireless.create-wireless-gateway-task-rejects-unknown-task-definition
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless.create-wireless-gateway-task-rejects-unknown-task-definition|run#16|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}: {message!r}"
Issue-Commit: 4108925797c66ed723c81e138f9788fc7f375a72

# Implement iotwireless CreateWirelessGatewayTask: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `4108925797c66ed723c81e138f9788fc7f375a72`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWirelessGatewayTask

## Description
`CreateWirelessGatewayTask` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iotwireless-iotwireless/src/ops/create_wireless_gateway_task.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateWirelessGatewayTask` call, so each one unblocks only when `CreateWirelessGatewayTask` works: `iotwireless.create-wireless-gateway-task-rejects-unknown-task-definition`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iotwireless-iotwireless/src/ops/create_wireless_gateway_task.rs`. Replaying the corpus, the emulator refused the `CreateWirelessGatewayTask` call: File "run.py", line 348, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_iotwireless_create_wireless_gateway_task_rejects_unknown_task_definition.py", line 90, in run
    assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceNotFoundException, got NotImplemented: 'operation CreateWirelessGatewayTask is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the iotwireless emulator
Call CreateWirelessGatewayTask and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
