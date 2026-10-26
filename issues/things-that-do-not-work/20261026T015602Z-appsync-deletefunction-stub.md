Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.op-contract-update-function-deleted-function-id-not-found
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|DeleteFunction|NotImplemented
Issue-Commit: 4d92b800bb5d886658635a287b2dfefdc5faa046

# Implement appsync DeleteFunction: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `4d92b800bb5d886658635a287b2dfefdc5faa046`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFunction

## Description
`DeleteFunction` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/delete_function.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteFunction` call, so each one unblocks only when `DeleteFunction` works: `appsync.op-contract-update-function-deleted-function-id-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/delete_function.rs`. Replaying the corpus, the emulator refused the `DeleteFunction` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteFunction operation: operation DeleteFunction is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the appsync emulator
Call DeleteFunction and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
