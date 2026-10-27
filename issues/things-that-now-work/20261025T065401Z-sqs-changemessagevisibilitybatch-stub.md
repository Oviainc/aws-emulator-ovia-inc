Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.change-message-visibility-batch-partial sqs.change-message-visibility-batch-partial-failure sqs.change-message-visibility-batch-repeated-receipt-handle-last-entry-wins
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|ChangeMessageVisibilityBatch|NotImplemented
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# Implement sqs ChangeMessageVisibilityBatch: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangeMessageVisibilityBatch

## Description
`ChangeMessageVisibilityBatch` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sqs-amazonsqs/src/ops/change_message_visibility_batch.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `ChangeMessageVisibilityBatch` call, so each one unblocks only when `ChangeMessageVisibilityBatch` works: `sqs.change-message-visibility-batch-partial`, `sqs.change-message-visibility-batch-partial-failure`, `sqs.change-message-visibility-batch-repeated-receipt-handle-last-entry-wins`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sqs-amazonsqs/src/ops/change_message_visibility_batch.rs`. Replaying the corpus, the emulator refused the `ChangeMessageVisibilityBatch` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ChangeMessageVisibilityBatch operation: operation ChangeMessageVisibilityBatch is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sqs emulator
Call ChangeMessageVisibilityBatch and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
