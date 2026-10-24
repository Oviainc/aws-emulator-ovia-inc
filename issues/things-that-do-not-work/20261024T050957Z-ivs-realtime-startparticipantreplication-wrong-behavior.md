Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2cb6c27e7920aa8b3ecbca8018912df677b4544f

# A started replication is never visible as a participant: the destination session gets no REPLICA participant and the source participant is never marked SOURCE

_Filed: 2026-10-24T05:09:57Z_
_Operation: StartParticipantReplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2cb6c27e7920aa8b3ecbca8018912df677b4544f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartParticipantReplication "Starts replicating a publishing participant from a source stage to a destination stage." The replicated participant is a participant of the destination stage session, and the model says so on ParticipantSummary: replicationType ("Indicates if the participant has been replicated to another stage or is a replica from another stage. Default: NONE"), sourceStageArn ("Source stage ARN from which this participant is replicated, if replicationType is REPLICA"), sourceSessionId ("ID of the session within the source stage, if replicationType is REPLICA") and replicationState.

Expected: after a successful start, ListParticipants on the DESTINATION stage + the session the replication reports as destinationSessionId reports a participant whose replicationType is REPLICA, carrying sourceStageArn / sourceSessionId and replicationState ACTIVE; and the participant in the SOURCE session reports replicationType SOURCE with replicationState ACTIVE (and, with filterByState/filterByRecordingState unset, is selectable by filterByPublished).

Actual: the handler pushes a ParticipantReplicaRecord into store.participant_replicas and opens a session on each stage via crate::state::ensure_active_session, but it writes no ParticipantRecord into either session's participants list and never updates any existing record's replication_type / replication_state / source_stage_arn / source_session_id. Observable result: GetStage on the destination stage now advertises an activeSessionId, yet ListParticipants for that (stageArn, sessionId) answers an empty participants list, and the three replication-describing members of ParticipantSummary can never be anything but NONE/absent. Only ListParticipantReplicas sees the replication.

## Evidence

Spec: smithy_query(query="doc", shape="StartParticipantReplication"); smithy_query(query="shape", shape="ParticipantSummary") → members replicationType, replicationState, sourceStageArn ("…if replicationType is REPLICA"), sourceSessionId.
Handler: services/.../src/ops/start_participant_replication.rs (handle, lines ~306-351): ensure_active_session for both stages, then store.participant_replicas.entry(source).push(record) — no write to any StageSessionRecord::participants, no mutation of a ParticipantRecord.
State: src/state.rs documents the intent and shows it unfulfilled — REPLICATION_TYPE_NONE's doc says "StartParticipantReplication … is what moves a participant off it", and ParticipantRecord carries replication_type / replication_state / source_stage_arn / source_session_id fields that only crate::state::join_stage_session (called by no operation) can set.
Tests: tests/all/participant_replication.rs asserts ListParticipantReplicas output, conflicts, paging and stage-delete reclamation only; tests/all/participants.rs seeds participants directly through the engine and never exercises a replica, so no test covers the destination session's participant view.

## Steps to Reproduce

1. POST /CreateStage {} twice → source stage ARN A, destination stage ARN B
2. POST /CreateParticipantToken {"stageArn":"A"} → note participantId P (capabilities default to PUBLISH,SUBSCRIBE)
3. POST /StartParticipantReplication {"sourceStageArn":"A","destinationStageArn":"B","participantId":"P"} → 200
4. POST /GetStage {"arn":"B"} → note the now non-empty activeSessionId S
5. POST /ListParticipants {"stageArn":"B","sessionId":"S"}
6. Expected: one participant with replicationType REPLICA, sourceStageArn A, sourceSessionId = the source session, replicationState ACTIVE. Actual: participants is empty, and no participant anywhere reports replicationType SOURCE/REPLICA
