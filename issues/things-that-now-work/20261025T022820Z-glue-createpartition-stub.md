Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.batch-get-and-delete-partition glue.get-partitions-expression-on-a-real-non-partition-column-refused glue.get-unfiltered-partitions-metadata-malformed-expression-blocked glue.partition-batch-create-partial-failure glue.partition-batch-get-omits-missing glue.partition-index-backfill-rejects-a-type-violating-partition-value glue.partition-index-lifecycle
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|CreatePartition|NotImplemented
Issue-Commit: e9591ea0ffea53e5939624d35a733297d20386ca

# Implement glue CreatePartition: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `e9591ea0ffea53e5939624d35a733297d20386ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePartition

## Description
`CreatePartition` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-glue-awsglue/src/ops/create_partition.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `CreatePartition` call, so each one unblocks only when `CreatePartition` works: `glue.batch-get-and-delete-partition`, `glue.get-partitions-expression-on-a-real-non-partition-column-refused`, `glue.get-unfiltered-partitions-metadata-malformed-expression-blocked`, `glue.partition-batch-create-partial-failure`, `glue.partition-batch-get-omits-missing`, `glue.partition-index-backfill-rejects-a-type-violating-partition-value`, `glue.partition-index-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-glue-awsglue/src/ops/create_partition.rs`. Replaying the corpus, the emulator refused the `CreatePartition` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreatePartition operation: operation CreatePartition is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the glue emulator
Call CreatePartition and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
