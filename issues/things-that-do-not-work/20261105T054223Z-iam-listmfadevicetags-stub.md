Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.mfa-device-tagging iam.mfa-device-tags-lifecycle
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ListMFADeviceTags|NotImplemented
Issue-Commit: 089026a1c9416fdeaf52bd064325368a385b884c

# Implement iam ListMFADeviceTags: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `089026a1c9416fdeaf52bd064325368a385b884c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMFADeviceTags

## Description
`ListMFADeviceTags` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/list_mfa_device_tags.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ListMFADeviceTags` call, so each one unblocks only when `ListMFADeviceTags` works: `iam.mfa-device-tagging`, `iam.mfa-device-tags-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/list_mfa_device_tags.rs`. Replaying the corpus, the emulator refused the `ListMFADeviceTags` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListMFADeviceTags operation: operation ListMFADeviceTags is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iam emulator
Call ListMFADeviceTags and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
