Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ivs.delete-recording-configuration-referenced-by-channel-rejected ivs.delete-recording-configuration-referenced-conflict-rejected ivs.recording-configurations-list-reflects-lifecycle
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs.delete-recording-configuration-referenced-by-channel-rejected|run#11|AssertionError|assert error["Code"] == "ConflictException", f"expected ConflictException, got {error!r}"
Issue-Key: ivs.delete-recording-configuration-referenced-conflict-rejected|run#17|AssertionError|assert error["Code"] == "ConflictException", f"expected ConflictException, got {error!r}"
Issue-Key: ivs|DeleteRecordingConfiguration|NotImplemented
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# Implement ivs DeleteRecordingConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRecordingConfiguration

## Description
`DeleteRecordingConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/delete_recording_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `DeleteRecordingConfiguration` call, so each one unblocks only when `DeleteRecordingConfiguration` works: `ivs.delete-recording-configuration-referenced-by-channel-rejected`, `ivs.delete-recording-configuration-referenced-conflict-rejected`, `ivs.recording-configurations-list-reflects-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/delete_recording_configuration.rs`. Replaying the corpus, the emulator refused the `DeleteRecordingConfiguration` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ivs_delete_recording_configuration_referenced_by_channel_rejected.py", line 45, in run
    assert error["Code"] == "ConflictException", f"expected ConflictException, got {error!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ConflictException, got {'Message': 'operation DeleteRecordingConfiguration is scaffolded but not implemented in this emulator', 'Code': 'NotImplemented'}

## Steps to Reproduce
Boot the ivs emulator
Call DeleteRecordingConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
