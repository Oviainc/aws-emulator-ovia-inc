Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.sm-version-publish-list-delete
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: sfn|DeleteStateMachineVersion|NotImplemented
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# Implement sfn DeleteStateMachineVersion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteStateMachineVersion

## Description
`DeleteStateMachineVersion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/delete_state_machine_version.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteStateMachineVersion` call, so each one unblocks only when `DeleteStateMachineVersion` works: `sfn.sm-version-publish-list-delete`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/delete_state_machine_version.rs`. Replaying the corpus, the emulator refused the `DeleteStateMachineVersion` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteStateMachineVersion operation: operation DeleteStateMachineVersion is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sfn emulator
Call DeleteStateMachineVersion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
