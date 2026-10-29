Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: comprehend.batch-detect-entities-batch-size-limit-exceeded-rejected comprehend.batch-detect-entities-batch-size-limit-rejected
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|BatchDetectEntities|NotImplemented
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# Implement comprehend BatchDetectEntities: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDetectEntities

## Description
`BatchDetectEntities` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-comprehend-comprehend-20171127/src/ops/batch_detect_entities.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `BatchDetectEntities` call, so each one unblocks only when `BatchDetectEntities` works: `comprehend.batch-detect-entities-batch-size-limit-exceeded-rejected`, `comprehend.batch-detect-entities-batch-size-limit-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-comprehend-comprehend-20171127/src/ops/batch_detect_entities.rs`. Replaying the corpus, the emulator refused the `BatchDetectEntities` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the BatchDetectEntities operation: operation BatchDetectEntities is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the comprehend emulator
Call BatchDetectEntities and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
