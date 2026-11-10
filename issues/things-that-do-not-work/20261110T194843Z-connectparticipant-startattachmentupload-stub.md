Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectparticipant.attachment-upload-lifecycle-and-download connectparticipant.complete-attachment-upload-twice-conflict-rejected connectparticipant.get-attachment-before-completion-rejected
Issue-Shape-Id: com.amazonaws.connectparticipant#AmazonConnectParticipantServiceLambda
Issue-Key: connectparticipant|StartAttachmentUpload|NotImplemented
Issue-Commit: 7ddd296fa3724341be1a6933a8dc37e7146d5c05

# Implement connectparticipant StartAttachmentUpload: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7ddd296fa3724341be1a6933a8dc37e7146d5c05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAttachmentUpload

## Description
`StartAttachmentUpload` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda/src/ops/start_attachment_upload.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `StartAttachmentUpload` call, so each one unblocks only when `StartAttachmentUpload` works: `connectparticipant.attachment-upload-lifecycle-and-download`, `connectparticipant.complete-attachment-upload-twice-conflict-rejected`, `connectparticipant.get-attachment-before-completion-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda/src/ops/start_attachment_upload.rs`. Replaying the corpus, the emulator refused the `StartAttachmentUpload` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartAttachmentUpload operation: operation StartAttachmentUpload is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the connectparticipant emulator
Call StartAttachmentUpload and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
