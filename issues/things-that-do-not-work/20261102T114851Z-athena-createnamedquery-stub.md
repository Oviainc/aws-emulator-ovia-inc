Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.athena-named-query-creation-in-a-disabled-workgroup-is-rejected
Issue-Key: athena|CreateNamedQuery|NotImplemented
Issue-Commit: 9b689e7a3706c4d3e9e7bd76513ebd3e34560844

# Implement athena CreateNamedQuery: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `9b689e7a3706c4d3e9e7bd76513ebd3e34560844`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNamedQuery

## Description
`CreateNamedQuery` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-athena-amazonathena/src/ops/create_named_query.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateNamedQuery` call, so each one unblocks only when `CreateNamedQuery` works: `athena__glue__s3.athena-named-query-creation-in-a-disabled-workgroup-is-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-athena-amazonathena/src/ops/create_named_query.rs`. Replaying the corpus, the emulator refused the `CreateNamedQuery` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateNamedQuery operation: operation CreateNamedQuery is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the athena emulator
Call CreateNamedQuery and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
