Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__pipes__sqs.events-pipes-enrichment-sees-whole-batch-and-target-gets-its-single-result lambda__pipes__sqs.events-pipes-enrichment-stopped-pipe-consumes-nothing-and-start-drains-backlog lambda__sns__sqs.sqs-lambda-esm-batch-window-respected
Issue-Key: sqs|SendMessageBatch|NotImplemented
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement sqs SendMessageBatch: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessageBatch

## Description
`SendMessageBatch` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sqs-amazonsqs/src/ops/send_message_batch.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `SendMessageBatch` call, so each one unblocks only when `SendMessageBatch` works: `lambda__pipes__sqs.events-pipes-enrichment-sees-whole-batch-and-target-gets-its-single-result`, `lambda__pipes__sqs.events-pipes-enrichment-stopped-pipe-consumes-nothing-and-start-drains-backlog`, `lambda__sns__sqs.sqs-lambda-esm-batch-window-respected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sqs-amazonsqs/src/ops/send_message_batch.rs`. Replaying the corpus, the emulator refused the `SendMessageBatch` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SendMessageBatch operation: operation SendMessageBatch is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sqs emulator
Call SendMessageBatch and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
