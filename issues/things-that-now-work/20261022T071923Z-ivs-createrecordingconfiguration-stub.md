Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ivs.create-recording-configuration-active-transition ivs.create-recording-configuration-custom-rendition-empty-list-rejected ivs.delete-recording-configuration-referenced-by-channel-rejected ivs.delete-recording-configuration-referenced-conflict-rejected ivs.list-recording-configurations-pagination-with-next-token ivs.recording-configuration-active-state-transition ivs.recording-configuration-reaches-active-state ivs.recording-configuration-rendition-selection-persists ivs.recording-configuration-thumbnail-and-rendition-persisted ivs.recording-configuration-thumbnail-configuration-persists ivs.recording-configurations-list-reflects-lifecycle ivs.update-channel-disables-recording-with-empty-arn
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|CreateRecordingConfiguration|NotImplemented
Issue-Key: ivs.create-recording-configuration-custom-rendition-empty-list-rejected|run#5|AssertionError|assert error["Code"] == "ValidationException", f"expected ValidationException, got {error!r}"
Issue-Commit: d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea

# Implement ivs CreateRecordingConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d6cf4ead3dce27ec3b8bcbc3d4a389da0fa571ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRecordingConfiguration

## Description
`CreateRecordingConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/create_recording_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

12 recorded unit(s) fail on their `CreateRecordingConfiguration` call, so each one unblocks only when `CreateRecordingConfiguration` works: `ivs.create-recording-configuration-active-transition`, `ivs.create-recording-configuration-custom-rendition-empty-list-rejected`, `ivs.delete-recording-configuration-referenced-by-channel-rejected`, `ivs.delete-recording-configuration-referenced-conflict-rejected`, `ivs.list-recording-configurations-pagination-with-next-token`, `ivs.recording-configuration-active-state-transition`, `ivs.recording-configuration-reaches-active-state`, `ivs.recording-configuration-rendition-selection-persists`, `ivs.recording-configuration-thumbnail-and-rendition-persisted`, `ivs.recording-configuration-thumbnail-configuration-persists`, `ivs.recording-configurations-list-reflects-lifecycle`, `ivs.update-channel-disables-recording-with-empty-arn`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/create_recording_configuration.rs`. Replaying the corpus, the emulator refused the `CreateRecordingConfiguration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateRecordingConfiguration operation: operation CreateRecordingConfiguration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ivs emulator
Call CreateRecordingConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
