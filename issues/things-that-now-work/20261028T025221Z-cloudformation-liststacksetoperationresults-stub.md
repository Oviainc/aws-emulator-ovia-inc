Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.stack-set-operation-lifecycle cloudformation.stackset-not-found-across-apis-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|ListStackSetOperationResults|NotImplemented
Issue-Key: cloudformation.stackset-not-found-across-apis-rejected|run#12|AssertionError|assert negative == expected, f"{label} returned {negative!r}, expected {expected!r}"
Issue-Commit: ccdeb765042b765c77fa3564f54cdee577f4e50e

# Implement cloudformation ListStackSetOperationResults: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ccdeb765042b765c77fa3564f54cdee577f4e50e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStackSetOperationResults

## Description
`ListStackSetOperationResults` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/list_stack_set_operation_results.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ListStackSetOperationResults` call, so each one unblocks only when `ListStackSetOperationResults` works: `cloudformation.stack-set-operation-lifecycle`, `cloudformation.stackset-not-found-across-apis-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/list_stack_set_operation_results.rs`. Replaying the corpus, the emulator refused the `ListStackSetOperationResults` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListStackSetOperationResults operation: operation ListStackSetOperationResults is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudformation emulator
Call ListStackSetOperationResults and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
