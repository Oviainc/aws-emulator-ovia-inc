Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.change-message-visibility-expired-handle-accepted sqs.change-message-visibility-extends sqs.change-message-visibility-extends-timeout sqs.change-message-visibility-zero-makes-immediately-visible sqs.changevis0-reexposes sqs.dead-letter-queue-redrive-after-max-receive-count sqs.dlq-move-preserves-standard-enqueue-timestamp-and-resets-it-for-fifo sqs.dlq-move-withheld-until-the-maxreceivecount-th-delivery sqs.dlq-redrive sqs.fifo-group-blocked-by-a-poison-message-resumes-after-the-dlq-move sqs.fifo-group-held-for-the-extended-visibility-timeout-not-the-original sqs.message-move-task-completes-and-drains-dlq sqs.message-retention-period-expires-old-messages sqs.op-visibility-timeout-out-of-range-rejected sqs.purge-empties sqs.receive-count-increments sqs.sender-id-and-sent-timestamp-derive-from-the-caller-and-the-live-clock sqs.visibility-extension-capped-at-twelve-hours-from-receive-rejected sqs.visibility-heartbeat-hides-a-message-from-concurrent-pollers sqs.visibility-timeout-shorten-and-receive-override
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs|ChangeMessageVisibility|NotImplemented
Issue-Key: sqs.op-visibility-timeout-out-of-range-rejected|run#9|AssertionError|assert err.get("Code") == "InvalidParameterValue", f"expected InvalidParameterValue for VisibilityTimeout=43201, got {err!r}"
Issue-Key: sqs.visibility-extension-capped-at-twelve-hours-from-receive-rejected|run#12|AssertionError|assert (code, msg) == ("InvalidParameterValue", _CAP_MESSAGE), (
Issue-Key: sqs.visibility-heartbeat-hides-a-message-from-concurrent-pollers|run#20|AssertionError|assert heartbeat_errors == [], f"a heartbeat extension was refused: {heartbeat_errors!r}"
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# Implement sqs ChangeMessageVisibility: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ChangeMessageVisibility

## Description
`ChangeMessageVisibility` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sqs-amazonsqs/src/ops/change_message_visibility.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

20 recorded unit(s) fail on their `ChangeMessageVisibility` call, so each one unblocks only when `ChangeMessageVisibility` works: `sqs.change-message-visibility-expired-handle-accepted`, `sqs.change-message-visibility-extends`, `sqs.change-message-visibility-extends-timeout`, `sqs.change-message-visibility-zero-makes-immediately-visible`, `sqs.changevis0-reexposes`, `sqs.dead-letter-queue-redrive-after-max-receive-count`, `sqs.dlq-move-preserves-standard-enqueue-timestamp-and-resets-it-for-fifo`, `sqs.dlq-move-withheld-until-the-maxreceivecount-th-delivery`, `sqs.dlq-redrive`, `sqs.fifo-group-blocked-by-a-poison-message-resumes-after-the-dlq-move`, `sqs.fifo-group-held-for-the-extended-visibility-timeout-not-the-original`, `sqs.message-move-task-completes-and-drains-dlq`, `sqs.message-retention-period-expires-old-messages`, `sqs.op-visibility-timeout-out-of-range-rejected`, `sqs.purge-empties`, `sqs.receive-count-increments`, `sqs.sender-id-and-sent-timestamp-derive-from-the-caller-and-the-live-clock`, `sqs.visibility-extension-capped-at-twelve-hours-from-receive-rejected`, `sqs.visibility-heartbeat-hides-a-message-from-concurrent-pollers`, `sqs.visibility-timeout-shorten-and-receive-override`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sqs-amazonsqs/src/ops/change_message_visibility.rs`. Replaying the corpus, the emulator refused the `ChangeMessageVisibility` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ChangeMessageVisibility operation: operation ChangeMessageVisibility is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the sqs emulator
Call ChangeMessageVisibility and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
