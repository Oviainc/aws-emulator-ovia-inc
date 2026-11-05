Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.concurrent-resync-mfa-device-race-rejected iam.delete-with-dependents-returns-deleteconflict iam.enable-mfa-device-already-enrolled-rejected iam.invalid-mfa-authentication-code-rejected iam.mfa-device-full-lifecycle iam.mfa-device-resync-code-window-enforced iam.mfa-devices-list-order-stable iam.op-contract-resync-mfa-device iam.resync-mfa-device-invalid-code-rejected iam.virtual-mfa-device-lifecycle iam.virtual-mfa-device-server-generated-seed-and-qrcode-contract
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|EnableMFADevice|NotImplemented
Issue-Key: iam.resync-mfa-device-invalid-code-rejected|_enable_device_with_retry#14|AssertionError|raise AssertionError(f"could not enable MFA device: {last_err}")
Issue-Commit: 089026a1c9416fdeaf52bd064325368a385b884c

# Implement iam EnableMFADevice: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `089026a1c9416fdeaf52bd064325368a385b884c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableMFADevice

## Description
`EnableMFADevice` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/enable_mfa_device.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

11 recorded unit(s) fail on their `EnableMFADevice` call, so each one unblocks only when `EnableMFADevice` works: `iam.concurrent-resync-mfa-device-race-rejected`, `iam.delete-with-dependents-returns-deleteconflict`, `iam.enable-mfa-device-already-enrolled-rejected`, `iam.invalid-mfa-authentication-code-rejected`, `iam.mfa-device-full-lifecycle`, `iam.mfa-device-resync-code-window-enforced`, `iam.mfa-devices-list-order-stable`, `iam.op-contract-resync-mfa-device`, `iam.resync-mfa-device-invalid-code-rejected`, `iam.virtual-mfa-device-lifecycle`, `iam.virtual-mfa-device-server-generated-seed-and-qrcode-contract`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/enable_mfa_device.rs`. Replaying the corpus, the emulator refused the `EnableMFADevice` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the EnableMFADevice operation: operation EnableMFADevice is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iam emulator
Call EnableMFADevice and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
