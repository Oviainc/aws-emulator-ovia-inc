Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9a3acde976cfb747ba29bb78f67d8041a784763f

# A stage's recordParticipantReplicas is never consulted: a replica participant is reported as being recorded into the stage's bucket

_Filed: 2026-10-29T03:06:43Z_
_Operation: GetParticipant | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9a3acde976cfb747ba29bb78f67d8041a784763f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AutoParticipantRecordingConfiguration$recordParticipantReplicas` is stored on the stage record and echoed back by `GetStage`/`CreateStage`/`UpdateStage`, but no code path ever READS it. The one place this crate decides where (and whether) a participant's media is being recorded — `crate::state::participant_recording_location` — consults exactly two of the three conditions the model states: (1) the stage's `autoParticipantRecordingConfiguration.storageConfigurationArn` names a storage configuration this store holds, and (2) the participant's `recordingState` is not `DISABLED`. The third condition, which the model states on the sibling member of the SAME structure the function already reads, is skipped: a participant that is a replica must not be recorded when the stage's configuration sets `recordParticipantReplicas` to `false`.

Consequences, all computed from that one helper:
* `GetParticipant` reports a non-empty `recordingS3BucketName` / `recordingS3Prefix` for a participant the stage reports as `replicationType: REPLICA`, although the stage's own configuration says replica participants are not recorded.
* `crate::state::stage_recording_participants` (built on the same helper) therefore counts that replica as a running recording, so `UpdateStage` answers ConflictException ("recording active") for a stage whose only "recording" is a replica its configuration excludes.

The crate has everything needed to apply the rule in that same function: it already loads the stage's `auto_participant_recording_configuration` there, and `crate::state::session_participant_replications` is the crate's one answer to "is this participant a replica of this session" (used by `ParticipantRecord::to_participant`, two functions away).

## Evidence

Smithy model, `AutoParticipantRecordingConfiguration$recordParticipantReplicas` (smithy_query query="shape" shape="AutoParticipantRecordingConfiguration"):
  "Optional field to disable replica participant recording. If this is set to `false` when a participant is a replica, replica participants are not recorded. Default: `true`."

The crate quotes that same sentence for its default constant — services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/src/state.rs:91-100:
  "/// Documented on `AutoParticipantRecordingConfiguration$recordParticipantReplicas`:
   /// \"Optional field to disable replica participant recording. … Default: `true`.\""
  "pub const DEFAULT_RECORD_PARTICIPANT_REPLICAS: bool = true;"

But the decision function never looks at it — src/state.rs:2035-2061:
  "pub fn participant_recording_location(" …
  "    if record.recording_state == NOT_RECORDED_PARTICIPANT_RECORDING_STATE {
         return ParticipantRecordingLocation::not_recorded();
     }
     let configured = state
         .stages
         .get(stage_arn)
         .map(|stage| {
             stage
                 .auto_participant_recording_configuration
                 .storage_configuration_arn
                 .as_str()
         })
         .filter(|arn| *arn != NO_STORAGE_CONFIGURATION_ARN)
         .and_then(|arn| state.storage_configurations.get(arn));
     match configured {
         Some(configuration) => ParticipantRecordingLocation { bucket_name: configuration.bucket_name.clone(), …"

The sibling helper states the rule it implements as only two halves plus a destination — src/state.rs:2081-2088:
  "/// a recording is running for a participant that is still
   /// [`PARTICIPANT_STATE_CONNECTED`], whose recorder state is a running one
   /// ([`participant_recording_running`]), and whose media has somewhere to go — the stage's
   /// `autoParticipantRecordingConfiguration` names a storage configuration this store
   /// holds"

And `record_participant_replicas` is read nowhere in a decision — every occurrence in `src/` is defaulting or serde:
  src/state.rs:196  "record_participant_replicas: Some(DEFAULT_RECORD_PARTICIPANT_REPLICAS),"
  src/state.rs:239-241 "record_participant_replicas: Some(documented_default(requested.record_participant_replicas, DEFAULT_RECORD_PARTICIPANT_REPLICAS)),"
  src/ops/types/shared_types.rs:253, 457, 473, 509, 524 (field declaration + wire codec only)

Caller: src/ops/get_participant.rs:208-210
  "Ok(GetParticipantOutput { participant: Some(record.to_participant(&store, &input.stage_arn, &input.session_id)) })"
and src/state.rs:2250 "let recording = participant_recording_location(store, stage_arn, session_id, self);"

Test coverage: tests/all/get_participant.rs:654-658 states the rule the implementation follows as only two halves — "Where a participant is being recorded takes both halves of \"individual participant recording is enabled\": the stage's `autoParticipantRecordingConfiguration` names the storage configuration whose bucket the media lands in, and the recorder reports a state other than `DISABLED` for that participant." — and its case table (lines 675-706) contains no replica row, so no existing test pins the replica behaviour.

## Steps to Reproduce

1. CreateStorageConfiguration with bucket `replica-recordings`.
2. CreateStage A with `autoParticipantRecordingConfiguration: { "storageConfigurationArn": <that ARN> }`.
3. CreateStage B with `autoParticipantRecordingConfiguration: { "storageConfigurationArn": <that ARN>, "recordParticipantReplicas": false }` — GetStage B echoes `recordParticipantReplicas: false`.
4. CreateParticipantToken on stage A (default capabilities include PUBLISH) and note the returned `participantId` P.
5. Record P joining stage B over the stage data plane with a running recorder state, the way tests/all/get_participant.rs does: `state::join_stage_session(&mut store, B, JoinedParticipant { participant_id: P, published: true, recording_state: Some("ACTIVE"), .. })`.
6. StartParticipantReplication with `sourceStageArn: A`, `destinationStageArn: B`, `participantId: P` (the already-CONNECTED branch of `record_replicated_participant` keeps P's `recordingState: ACTIVE`).
7. GetParticipant on stage B's active session for P: it reports `replicationType: "REPLICA"` AND `recordingS3BucketName: "replica-recordings"` with a non-empty `recordingS3Prefix`, although stage B's configuration says replica participants are not recorded — expected the documented empty strings.
8. Call UpdateStage on B with any `autoParticipantRecordingConfiguration`: it answers 409 ConflictException naming P's running recording, a recording stage B's own `recordParticipantReplicas: false` excludes.
