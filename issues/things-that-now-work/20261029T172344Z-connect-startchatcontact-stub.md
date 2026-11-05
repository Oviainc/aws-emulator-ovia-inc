Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectparticipant.attachment-upload-lifecycle-and-download connectparticipant.authentication-operations-invalid-session-id-rejected connectparticipant.concurrent-send-message-no-loss-no-duplication connectparticipant.create-participant-connection-multi-type-channels connectparticipant.create-participant-connection-rejected-inputs connectparticipant.describe-view-unknown-token-rejected connectparticipant.disconnect-participant-invalidates-connection-token connectparticipant.get-transcript-pagination-completeness connectparticipant.get-transcript-sort-order-matches-send-order connectparticipant.participant-display-name-reflected-in-transcript connectparticipant.send-event-client-token-caches-on-content-not-contenttype connectparticipant.send-event-typing-appears-as-typing-item connectparticipant.send-message-client-token-idempotency connectparticipant.send-message-transcript-roundtrip connectparticipant.send-multiple-events-produce-distinct-transcript-ids
Issue-Shape-Id: com.amazonaws.connectparticipant#AmazonConnectParticipantServiceLambda
Issue-Key: connect|StartChatContact|NotImplemented
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# Implement connect StartChatContact: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartChatContact

## Description
`StartChatContact` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-connect-amazonconnectservice/src/ops/start_chat_contact.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

15 recorded unit(s) fail on their `StartChatContact` call, so each one unblocks only when `StartChatContact` works: `connectparticipant.attachment-upload-lifecycle-and-download`, `connectparticipant.authentication-operations-invalid-session-id-rejected`, `connectparticipant.concurrent-send-message-no-loss-no-duplication`, `connectparticipant.create-participant-connection-multi-type-channels`, `connectparticipant.create-participant-connection-rejected-inputs`, `connectparticipant.describe-view-unknown-token-rejected`, `connectparticipant.disconnect-participant-invalidates-connection-token`, `connectparticipant.get-transcript-pagination-completeness`, `connectparticipant.get-transcript-sort-order-matches-send-order`, `connectparticipant.participant-display-name-reflected-in-transcript`, `connectparticipant.send-event-client-token-caches-on-content-not-contenttype`, `connectparticipant.send-event-typing-appears-as-typing-item`, `connectparticipant.send-message-client-token-idempotency`, `connectparticipant.send-message-transcript-roundtrip`, `connectparticipant.send-multiple-events-produce-distinct-transcript-ids`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-connect-amazonconnectservice/src/ops/start_chat_contact.rs`. Replaying the corpus, the emulator refused the `StartChatContact` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartChatContact operation: operation StartChatContact is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the connect emulator
Call StartChatContact and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
