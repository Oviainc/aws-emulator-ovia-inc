Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3vectors.deleted-vector-key-reusable-and-deleted-index-refuses-writes s3vectors.e2e-put-query-delete-removes-from-results s3vectors.e2e-vector-lifecycle-flow s3vectors.query-vectors-reranks-when-a-nearer-vector-is-inserted
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors|DeleteVectors|NotImplemented
Issue-Commit: c1cdf4b562f81ba1ac84179127e5df6b90873b05

# Implement s3vectors DeleteVectors: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c1cdf4b562f81ba1ac84179127e5df6b90873b05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVectors

## Description
`DeleteVectors` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3vectors-s3vectors/src/ops/delete_vectors.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `DeleteVectors` call, so each one unblocks only when `DeleteVectors` works: `s3vectors.deleted-vector-key-reusable-and-deleted-index-refuses-writes`, `s3vectors.e2e-put-query-delete-removes-from-results`, `s3vectors.e2e-vector-lifecycle-flow`, `s3vectors.query-vectors-reranks-when-a-nearer-vector-is-inserted`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3vectors-s3vectors/src/ops/delete_vectors.rs`. Replaying the corpus, the emulator refused the `DeleteVectors` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteVectors operation: operation DeleteVectors is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3vectors emulator
Call DeleteVectors and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
