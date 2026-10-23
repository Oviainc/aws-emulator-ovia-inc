Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.concurrent-put-lookup-table-rows-last-writer-wins-deterministically logs.create-lookup-table-limit-exceeded-unreachable logs.delete-lookup-table-already-deleted-rejected logs.delete-lookup-table-nonexistent-fails logs.e2e-lookup-table-enriches-transformed-log-record logs.e2e-lookup-table-enrichment-flow logs.get-deleted-lookup-table-fails logs.list-lookup-tables-reflects-create-and-delete logs.lookup-table-duplicate-name-rejected logs.lookup-table-lifecycle logs.lookup-table-numeric-value-precision-preserved logs.update-lookup-table-nonexistent-fails logs.update-lookup-table-stale-version-write-is-accepted
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|CreateLookupTable|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement logs CreateLookupTable: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLookupTable

## Description
`CreateLookupTable` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_lookup_table.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `CreateLookupTable` call, so each one unblocks only when `CreateLookupTable` works: `logs.concurrent-put-lookup-table-rows-last-writer-wins-deterministically`, `logs.create-lookup-table-limit-exceeded-unreachable`, `logs.delete-lookup-table-already-deleted-rejected`, `logs.delete-lookup-table-nonexistent-fails`, `logs.e2e-lookup-table-enriches-transformed-log-record`, `logs.e2e-lookup-table-enrichment-flow`, `logs.get-deleted-lookup-table-fails`, `logs.list-lookup-tables-reflects-create-and-delete`, `logs.lookup-table-duplicate-name-rejected`, `logs.lookup-table-lifecycle`, `logs.lookup-table-numeric-value-precision-preserved`, `logs.update-lookup-table-nonexistent-fails`, `logs.update-lookup-table-stale-version-write-is-accepted`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_lookup_table.rs`. Replaying the corpus, the emulator refused the `CreateLookupTable` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateLookupTable operation: operation CreateLookupTable is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the logs emulator
Call CreateLookupTable and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
