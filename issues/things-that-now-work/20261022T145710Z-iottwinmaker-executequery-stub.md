Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iottwinmaker.execute-query-filters-by-entity-name iottwinmaker.execute-query-invalid-syntax-rejected
Issue-Shape-Id: com.amazonaws.iottwinmaker#AWSIoTTwinMaker
Issue-Key: iottwinmaker|ExecuteQuery|NotImplemented
Issue-Key: iottwinmaker.execute-query-invalid-syntax-rejected|run#9|AssertionError|assert err["Code"] == "ValidationException", f"expected ValidationException, got {err!r}"
Issue-Commit: dd79a542b09a035cf711a2d0c52670e9a3e4e70a

# Implement iottwinmaker ExecuteQuery: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `dd79a542b09a035cf711a2d0c52670e9a3e4e70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteQuery

## Description
`ExecuteQuery` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/execute_query.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ExecuteQuery` call, so each one unblocks only when `ExecuteQuery` works: `iottwinmaker.execute-query-filters-by-entity-name`, `iottwinmaker.execute-query-invalid-syntax-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/execute_query.rs`. Replaying the corpus, the emulator refused the `ExecuteQuery` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ExecuteQuery operation: operation ExecuteQuery is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iottwinmaker emulator
Call ExecuteQuery and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
