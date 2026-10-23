Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__firehose__glue__lambda__s3.fhtransform-dropped-records-never-become-athena-rows-until-the-verdict-is-repaired athena__firehose__glue__lambda__s3.fhtransform-lambda-rewritten-records-are-read-back-as-parquet-rows-by-athena athena__firehose__glue__lambda__s3.fhtransform-processing-failed-is-attributed-to-the-transform-not-the-parquet-conversion
Issue-Key: athena|StartQueryExecution|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement athena StartQueryExecution: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
`StartQueryExecution` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-athena-amazonathena/src/ops/start_query_execution.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `StartQueryExecution` call, so each one unblocks only when `StartQueryExecution` works: `athena__firehose__glue__lambda__s3.fhtransform-dropped-records-never-become-athena-rows-until-the-verdict-is-repaired`, `athena__firehose__glue__lambda__s3.fhtransform-lambda-rewritten-records-are-read-back-as-parquet-rows-by-athena`, `athena__firehose__glue__lambda__s3.fhtransform-processing-failed-is-attributed-to-the-transform-not-the-parquet-conversion`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-athena-amazonathena/src/ops/start_query_execution.rs`. Replaying the corpus, the emulator refused the `StartQueryExecution` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartQueryExecution operation: operation StartQueryExecution is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the athena emulator
Call StartQueryExecution and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
