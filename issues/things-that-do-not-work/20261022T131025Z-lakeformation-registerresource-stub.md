Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__lakeformation__s3.lf-column-grant-projects-granted-columns-and-excluded-column-denied athena__glue__lakeformation__s3.lf-revoke-select-stops-athena-read-and-rehides-table athena__glue__lakeformation__s3.lf-row-filter-restricts-athena-rows-to-matching-predicate athena__glue__lakeformation__s3.lf-table-select-grant-readable-through-athena-and-ungranted-table-denied
Issue-Key: lakeformation|RegisterResource|NotImplemented
Issue-Commit: 6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1

# Implement lakeformation RegisterResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6f82ed95aacdd57d8c8c0220ec6b50c75b2537e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterResource

## Description
`RegisterResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-lakeformation-awslakeformation/src/ops/register_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `RegisterResource` call, so each one unblocks only when `RegisterResource` works: `athena__glue__lakeformation__s3.lf-column-grant-projects-granted-columns-and-excluded-column-denied`, `athena__glue__lakeformation__s3.lf-revoke-select-stops-athena-read-and-rehides-table`, `athena__glue__lakeformation__s3.lf-row-filter-restricts-athena-rows-to-matching-predicate`, `athena__glue__lakeformation__s3.lf-table-select-grant-readable-through-athena-and-ungranted-table-denied`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-lakeformation-awslakeformation/src/ops/register_resource.rs`. Replaying the corpus, the emulator refused the `RegisterResource` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RegisterResource operation: operation RegisterResource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the lakeformation emulator
Call RegisterResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
