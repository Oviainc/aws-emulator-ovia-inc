Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.alias-create-describe-update-delete-lifecycle stepfunctions.alias-list-ordering-consistent stepfunctions.list-aliases-pagination
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: sfn|ListStateMachineAliases|NotImplemented
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# Implement sfn ListStateMachineAliases: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStateMachineAliases

## Description
`ListStateMachineAliases` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/list_state_machine_aliases.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `ListStateMachineAliases` call, so each one unblocks only when `ListStateMachineAliases` works: `sfn.alias-create-describe-update-delete-lifecycle`, `sfn.alias-list-ordering-consistent`, `sfn.list-aliases-pagination`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/list_state_machine_aliases.rs`. Replaying the corpus, the emulator refused the `ListStateMachineAliases` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListStateMachineAliases operation: operation ListStateMachineAliases is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sfn emulator
Call ListStateMachineAliases and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
