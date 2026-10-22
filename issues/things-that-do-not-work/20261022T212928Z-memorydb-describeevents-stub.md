Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: memorydb.events-record-user-and-acl-lifecycle
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|DescribeEvents|NotImplemented
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# Implement memorydb DescribeEvents: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEvents

## Description
`DescribeEvents` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-memorydb-amazonmemorydb/src/ops/describe_events.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribeEvents` call, so each one unblocks only when `DescribeEvents` works: `memorydb.events-record-user-and-acl-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-memorydb-amazonmemorydb/src/ops/describe_events.rs`. Replaying the corpus, the emulator refused the `DescribeEvents` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeEvents operation: operation DescribeEvents is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the memorydb emulator
Call DescribeEvents and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
