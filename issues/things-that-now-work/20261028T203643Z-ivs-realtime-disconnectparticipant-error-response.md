Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 825b0e930e0bc7d005dad99ff95af4e6881702a4

# DisconnectParticipant answers 404 for a replica participant the same stage reports CONNECTED

_Filed: 2026-10-28T20:36:43Z_
_Operation: DisconnectParticipant | Class: error-response_

> _Note: this issue was identified against the code base at commit `825b0e930e0bc7d005dad99ff95af4e6881702a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A running replication puts a REAL participant record into the destination stage's session: `crate::state::record_replicated_participant` joins the replicated publisher into that stage through `join_stage_session`, and `ListParticipants` / `GetParticipant` on the destination stage report it `state: "CONNECTED"`, `replicationType: "REPLICA"` (pinned by `tests/all/replicated_participant_views.rs`).

`DisconnectParticipant` cannot touch that participant. Its not-found gate is `crate::state::stage_participant`, which resolves a participant ONLY from (a) the token records keyed by the named stage's own ARN and (b) the ingest configurations whose `stage_arn` equals the named stage. A replica's identity was issued against the SOURCE stage, so neither branch matches and the handler returns `ResourceNotFoundException` (HTTP 404) naming the `participantId` — for a participant that the very same stage's readers report as present and connected.

The inconsistency is inside one handler: two lines after the 404 gate the handler calls `crate::state::connected_sessions_of`, whose own doc calls it "The ONE lookup for 'where in this stage is this participant still present'", and that lookup DOES hold the replica's session. `stage_participant`'s doc calls itself "The ONE lookup for 'does this stage have this participant, and may it publish'". Both claim to be the single answer to the same question and they disagree for a replica, and the one that answers 404 is consulted first — so the departure loop that would work is never reached and no operation of this service can take a replica out of its destination stage except `StopParticipantReplication` / `DeleteStage`.

## Evidence

The 404 gate and the lookup that contradicts it sit in the same handler — `src/ops/disconnect_participant.rs:192-209`:
```
        if crate::state::stage_participant(&store, &input.stage_arn, &input.participant_id).is_none()
        {
            return Err(crate::state::resource_not_found(
                DisconnectParticipantErrorModeled::ResourceNotFoundException,
                &input.participant_id,
            ));
        }
...
        let connected_sessions =
            crate::state::connected_sessions_of(&store, &input.stage_arn, &input.participant_id);
```

The gate's lookup can only see identities issued FOR that stage — `src/state.rs:4061-4069` (doc) and `src/state.rs:4074-4099` (body):
```
/// A participant id is "assigned by IVS and returned by `CreateParticipantToken`" ...
/// so the identities this store holds for a stage are exactly its token records and the
/// credentials of the ingest configurations associated with it. The ONE lookup for "does
/// this stage have this participant, and may it publish", so no two operations can
/// disagree about it.
```
```
    if let Some(record) = state
        .participant_tokens
        .get(stage_arn)
...
    state
        .ingest_configurations
        .values()
        .find(|configuration| {
            configuration.stage_arn == stage_arn
                && configuration.mints_participant(participant_id)
        })
```

The other "ONE lookup", used by the same handler, answers the opposite — `src/state.rs:2990-2997`:
```
/// The sessions of the stage `stage_arn` in which `participant_id` is still
/// [`PARTICIPANT_STATE_CONNECTED`], in the order the stage holds them.
///
/// The ONE lookup for "where in this stage is this participant still present", beside the
/// [`leave_stage_session`] that acts on the answer, so an operation that takes a
/// participant out of a stage and the records `ListParticipants` reports for it cannot
/// disagree about which sessions it was in.
```

The destination-stage record the gate cannot see is written by this crate — `src/state.rs:3177-3193`:
```
        let joined = JoinedParticipant {
            participant_id: replica.participant_id.clone(),
...
        if join_stage_session(state, stage_arn, joined).is_none() {
```

And the crate's own test pins that the destination stage reports it — `tests/all/replicated_participant_views.rs:644-679`:
```
        // Each destination session: one participant, and it is the replica.
...
                "case {case:?}: the destination session holds exactly the replica, \
                 got {listed}"
...
            assert_eq!(
                text(replica, "state"),
                CONNECTED,
                "case {case:?}: a replica is in the destination stage, got {listed}"
            );
```

Model text for the operation (`smithy_query(query="doc", shape="DisconnectParticipant")`): "Disconnects a specified participant from a specified stage."

Test coverage: the nearest existing case is `tests/all/disconnect_participant.rs:806-812`, which pins 404 for "a participant of another stage" — a token minted against another stage that never joined the named one. No test covers a participant that IS a connected record of the named stage via replication, so nothing pins the behaviour described here.

## Steps to Reproduce

1. CreateStage twice: a source stage and a destination stage.
2. CreateParticipantToken on the source stage with capabilities ["PUBLISH"]; note the minted participantId.
3. Record the publisher joining the source stage through the data-plane seam the crate's own suites use: `state::join_stage_session(&mut store, source_stage_arn, JoinedParticipant { participant_id, published: true, .. })` (see tests/all/replicated_participant_views.rs).
4. StartParticipantReplication with sourceStageArn=<source>, destinationStageArn=<destination>, participantId=<id>; expect 200.
5. GetStage on the destination stage to read its activeSessionId, then ListParticipants on that stage/session: the participantId is reported with state "CONNECTED" and replicationType "REPLICA".
6. DisconnectParticipant with stageArn=<destination>, participantId=<id>: expected the participant the previous step just reported to be disconnected; actual HTTP 404 ResourceNotFoundException naming the participantId, and ListParticipants on the destination stage still reports it CONNECTED.
