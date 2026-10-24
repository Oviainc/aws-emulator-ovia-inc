Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ca029f426fda2d51f4b9c87edd69d4539a13e33a

# UpdateStage accepts an autoParticipantRecordingConfiguration.storageConfigurationArn that names no storage configuration

_Filed: 2026-10-24T10:07:18Z_
_Operation: UpdateStage | Class: validation_

> _Note: this issue was identified against the code base at commit `ca029f426fda2d51f4b9c87edd69d4539a13e33a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateStage attaches a new individual-participant-recording configuration to an existing stage; `storageConfigurationArn` is a reference to a StorageConfiguration resource this emulator holds in its own store. UpdateStage models ValidationException (400).

Expected: an update whose `autoParticipantRecordingConfiguration.storageConfigurationArn` names no storage configuration in the account/Region is refused with ValidationException (400) and the stage keeps the configuration it had.

Actual: the handler overwrites the stored configuration with `crate::state::effective_recording_configuration(Some(configuration))`, which resolves nothing — any well-formed ARN is accepted and reported back by the UpdateStage response and every later GetStage, leaving a stage bound to a recording destination that does not exist.

## Evidence

Spec: smithy_query(query="operation", operation="UpdateStage") — input carries autoParticipantRecordingConfiguration and the operation models ValidationException (400); smithy_query(query="shape", shape="AutoParticipantRecordingConfiguration") documents storageConfigurationArn as the StorageConfiguration resource to use.
Handler: src/ops/update_stage.rs `handle` lines 174-179 — `record.auto_participant_recording_configuration = crate::state::effective_recording_configuration(Some(configuration))`; src/state.rs lines 190-211 never consult `state.storage_configurations`.
Tests: tests/all/stages.rs `update_stage_changes_only_the_members_supplied` (lines 918-990) and the configuration-default rows (lines 1600-1700) send a fabricated storage-configuration ARN and assert HTTP 200 — acceptance is codified, nothing asserts a refusal.

## Steps to Reproduce

1. POST /CreateStage {"name":"plain"} → note stage.arn
2. POST /UpdateStage {"arn":"<stage arn>","autoParticipantRecordingConfiguration":{"storageConfigurationArn":"arn:aws:ivs:us-east-1:000000000000:storage-configuration/abcd1234"}}
3. Expected: HTTP 400 ValidationException; GetStage still reports the stage's previous configuration
4. Actual: HTTP 200 and GetStage reports the dangling storageConfigurationArn as the stage's recording destination
