Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 53db56bf3158c7205a85c81c618ec0b3c4193a64

# UpdateStage changes autoParticipantRecordingConfiguration while recording is active instead of answering ConflictException

_Filed: 2026-10-27T05:02:21Z_
_Operation: UpdateStage | Class: error-response_

> _Note: this issue was identified against the code base at commit `53db56bf3158c7205a85c81c618ec0b3c4193a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states on the very member UpdateStage writes that it cannot be changed while recording is active, and the operation models `ConflictException` (HTTP 409) for exactly that kind of refusal. The handler writes the new recording configuration unconditionally: it resolves the configuration, looks the stage up, and assigns it without ever asking whether anything is recording in that stage. `UpdateStageErrorModeled::ConflictException` is declared in the error enum and is never constructed anywhere in the file, so the documented failure mode answers HTTP 200 and a mutated stage.

The crate already holds everything the check needs: `ParticipantRecord::recording_state` and `state::NOT_RECORDED_PARTICIPANT_RECORDING_STATE`, which `state::participant_recording_location` already uses as the test for "is this participant being recorded", and the sibling `UpdateIngestConfiguration` implements the analogous documented rule ("An IngestConfiguration that is active cannot be updated") by answering `ConflictException` before it touches the record. UpdateStage skips that pattern, so a caller can swap the storage configuration (or reset the whole object with `""`) out from under a running recording.

## Evidence

Smithy, `smithy_query(query="doc", shape="UpdateStageRequest", member="autoParticipantRecordingConfiguration")`: "Configuration object for individual participant recording, to attach to the stage. **Note that this cannot be updated while recording is active.**" — the handler's own modeled-contract docblock repeats it verbatim at services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/src/ops/update_stage.rs:50-52: "//! * `autoParticipantRecordingConfiguration` (AutoParticipantRecordingConfiguration) — //!   Configuration object for individual participant recording, to attach to the stage. Note //!   that this cannot be updated while recording is active."

The code under that doc (update_stage.rs:201-208):
```
        // Only the members the request supplied are changed; an absent member leaves
        // the stored value alone, so a later read still reports it.
        if let Some(name) = input.name {
            record.name = Some(name);
        }
        if let Some(configuration) = recording_configuration {
            record.auto_participant_recording_configuration = configuration;
        }
```
No recording check precedes it, and the declared variant is dead: update_stage.rs:95 `    ConflictException(ConflictException),` — a grep for `UpdateStageErrorModeled::ConflictException` across the crate returns nothing.

Sibling that DOES implement the analogous rule, src/ops/update_ingest_configuration.rs:205-221:
```
        // "An IngestConfiguration that is active cannot be updated": active is exactly
        // what a read of it reports as `ACTIVE` ...
        if let Some(publisher) =
            crate::state::ingest_configuration_publishers(&store, &record).first()
        {
            return Err(crate::state::conflict(
                UpdateIngestConfigurationErrorModeled::ConflictException,
```
The existing test for "is this participant recorded", src/state.rs:1897: `    if record.recording_state == NOT_RECORDED_PARTICIPANT_RECORDING_STATE {` with src/state.rs:1880-1882 "[`NOT_RECORDED_PARTICIPANT_RECORDING_STATE`] is the one value of `ParticipantRecordingState` that is not a state of a running recording" (`ParticipantRecordingState` = STARTING | ACTIVE | STOPPING | STOPPED | FAILED | DISABLED).

Test coverage: no test pins the recording-active case — `grep -r 'recording is active|cannot be updated while' tests/` returns "No matches."; tests/all/stages.rs:934 ("UpdateStage changes the members the request supplies, leaves every other member…") and tests/all/stage_storage_configuration_reference.rs:257 exercise only stages with no recording participant, so they stay green.

## Steps to Reproduce

1. CreateStorageConfiguration, then CreateStage with autoParticipantRecordingConfiguration.storageConfigurationArn set to it.
2. Put a participant into that stage whose recording is running (the data-plane seam `state::join_stage_session` with `recording_state: Some("ACTIVE")`, the way tests/all/get_participant.rs seeds joins); GetParticipant now reports recordingState ACTIVE and a recordingS3BucketName.
3. Call UpdateStage on that stage with a different autoParticipantRecordingConfiguration (or with storageConfigurationArn "" to reset it).
4. Expected: the modelled ConflictException (HTTP 409), since the model says this member "cannot be updated while recording is active" — the same way UpdateIngestConfiguration refuses an ACTIVE configuration.
5. Actual: HTTP 200, and GetStage reports the new configuration while the participant is still recording.
