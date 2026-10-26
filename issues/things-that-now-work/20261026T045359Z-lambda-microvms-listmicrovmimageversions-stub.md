Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda-microvms.concurrent-create-microvm-image-same-client-token-idempotent
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda-microvms|ListMicrovmImageVersions|NotImplemented
Issue-Commit: 40f687c7e46fd804b78a0310acfb55e1be355e66

# Implement lambda-microvms ListMicrovmImageVersions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `40f687c7e46fd804b78a0310acfb55e1be355e66`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMicrovmImageVersions

## Description
`ListMicrovmImageVersions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/list_microvm_image_versions.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListMicrovmImageVersions` call, so each one unblocks only when `ListMicrovmImageVersions` works: `lambda-microvms.concurrent-create-microvm-image-same-client-token-idempotent`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/list_microvm_image_versions.rs`. Replaying the corpus, the emulator refused the `ListMicrovmImageVersions` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListMicrovmImageVersions operation: operation ListMicrovmImageVersions is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the lambda-microvms emulator
Call ListMicrovmImageVersions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
