Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: macie2.findings-filter-delete-then-not-found
Issue-Shape-Id: com.amazonaws.macie2#Macie2
Issue-Key: macie2|CreateFindingsFilter|NotImplemented
Issue-Commit: 85498e1f8b4eef953ced847d4ecb0ca61f30a5a5

# Implement macie2 CreateFindingsFilter: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `85498e1f8b4eef953ced847d4ecb0ca61f30a5a5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFindingsFilter

## Description
`CreateFindingsFilter` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-macie2-macie2/src/ops/create_findings_filter.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateFindingsFilter` call, so each one unblocks only when `CreateFindingsFilter` works: `macie2.findings-filter-delete-then-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-macie2-macie2/src/ops/create_findings_filter.rs`. Replaying the corpus, the emulator refused the `CreateFindingsFilter` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateFindingsFilter operation: operation CreateFindingsFilter is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the macie2 emulator
Call CreateFindingsFilter and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
