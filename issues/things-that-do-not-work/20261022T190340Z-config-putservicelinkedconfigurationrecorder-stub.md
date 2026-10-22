Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.delete-service-linked-recorder-conflict-while-active-blocked config.delete-service-linked-recorder-while-recording-conflict-blocked config.duplicate-service-linked-recorder-rejected config.put-service-linked-recorder-limit-exceeded-blocked config.put-service-linked-recorder-limit-exceeded-rejected config.server-side-service-linked-recorder-policy-enforced config.service-linked-configuration-recorder-duplicate-conflict-rejected config.service-linked-configuration-recorder-lifecycle config.service-linked-recorder-lifecycle
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config-service|PutServiceLinkedConfigurationRecorder|NotImplemented
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Implement config PutServiceLinkedConfigurationRecorder: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutServiceLinkedConfigurationRecorder

## Description
`PutServiceLinkedConfigurationRecorder` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_service_linked_configuration_recorder.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `PutServiceLinkedConfigurationRecorder` call, so each one unblocks only when `PutServiceLinkedConfigurationRecorder` works: `config.delete-service-linked-recorder-conflict-while-active-blocked`, `config.delete-service-linked-recorder-while-recording-conflict-blocked`, `config.duplicate-service-linked-recorder-rejected`, `config.put-service-linked-recorder-limit-exceeded-blocked`, `config.put-service-linked-recorder-limit-exceeded-rejected`, `config.server-side-service-linked-recorder-policy-enforced`, `config.service-linked-configuration-recorder-duplicate-conflict-rejected`, `config.service-linked-configuration-recorder-lifecycle`, `config.service-linked-recorder-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_service_linked_configuration_recorder.rs`. Replaying the corpus, the emulator refused the `PutServiceLinkedConfigurationRecorder` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutServiceLinkedConfigurationRecorder operation: operation PutServiceLinkedConfigurationRecorder is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the config emulator
Call PutServiceLinkedConfigurationRecorder and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
