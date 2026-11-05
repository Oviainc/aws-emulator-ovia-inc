Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.mfa-device-tag-lifecycle iam.mfa-device-tag-unicode-value-preserved iam.tag-mfadevice iam.tag-nonexistent-mfa-device-rejected-not-silently-accepted iam.untag-mfadevice iam.untag-nonexistent-mfa-device-returns-nosuchentity
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|TagMFADevice|NotImplemented
Issue-Commit: 089026a1c9416fdeaf52bd064325368a385b884c

# Implement iam TagMFADevice: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `089026a1c9416fdeaf52bd064325368a385b884c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagMFADevice

## Description
`TagMFADevice` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/tag_mfa_device.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `TagMFADevice` call, so each one unblocks only when `TagMFADevice` works: `iam.mfa-device-tag-lifecycle`, `iam.mfa-device-tag-unicode-value-preserved`, `iam.tag-mfadevice`, `iam.tag-nonexistent-mfa-device-rejected-not-silently-accepted`, `iam.untag-mfadevice`, `iam.untag-nonexistent-mfa-device-returns-nosuchentity`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/tag_mfa_device.rs`. Replaying the corpus, the emulator refused the `TagMFADevice` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagMFADevice operation: operation TagMFADevice is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iam emulator
Call TagMFADevice and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
