Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# DisconnectParticipant leaves a replicated publisher's replication ACTIVE and its replica connected

_Filed: 2026-10-28T13:36:04Z_
_Operation: DisconnectParticipant | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DisconnectParticipant` takes the named participant out of every session of the stage it is connected in (`crate::state::leave_stage_session`), but it never touches the participant replications this store holds for that publisher. When the disconnected participant is the source of a running replication, the `ParticipantReplicaRecord` keeps `replication_state = ACTIVE` and the replica record the replication put into the DESTINATION stage's session stays `CONNECTED`.

So after a successful DisconnectParticipant:
* `GetParticipant`/`ListParticipants` on the source stage report the publisher `DISCONNECTED` while still reporting `replicationState: ACTIVE` for it (both read the stored replication through `crate::state::session_participant_replications`),
* `ListParticipantReplicas` still reports the replica with `replicationState: ACTIVE`, and
* the destination stage still holds a `CONNECTED` participant that exists only because of that replication, so that stage never empties (its `activeSessionId` is never cleared and a composition of it never starts the empty-stage timeout, because `observe_stage_occupancy` reads `stage_has_participants`).

The crate's own comment on the field that models this case asserts the opposite premise — that nothing in the emulator disconnects a publisher — which is exactly what this handler does, and the sibling operation that ends a replication does take the replica back out of the destination session.

## Evidence

The handler disconnects the publisher and writes nothing else about replication — `src/ops/disconnect_participant.rs:222-245`:
```
        // "Disconnects a specified participant from a specified stage": the participant
        // leaves the stage through the one place a participant leaves, ...
        for session_id in &connected_sessions {
            crate::state::leave_stage_session(
                &mut store,
                &input.stage_arn,
                session_id,
                &input.participant_id,
            );
        }
```
(the only other write in the handler is `configuration.clear_stage_association()`).

The state module's own comment on the replication field claims no publisher is ever disconnected — `src/state.rs:2474-2478`:
```
    /// The window a disconnected publisher may reconnect in before replication ends,
    /// or [`DEFAULT_REPLICATION_RECONNECT_WINDOW_SECONDS`] when the request omitted it.
    /// Nothing disconnects an emulated publisher, so no clock reads it yet; it is
    /// recorded because it is what the caller asked replication to do.
    pub reconnect_window_seconds: i32,
```

The sibling that ends a replication DOES take the replica out of the destination session — `src/ops/stop_participant_replication.rs:258-282`:
```
        // The replica is what the replication put into the destination stage, so stopping
        // it takes that participant back out of the destination stage's session, through
        // the one place a participant leaves a session
        // (`crate::state::leave_stage_session`) ...
        if !still_replicated {
            crate::state::leave_stage_session(
                &mut store,
                &stopped.destination_stage_arn,
                &stopped.destination_session_id,
                &stopped.participant_id,
            );
        }
```

The model ties the replication's ACTIVE state to the publisher's connection — `smithy_query(query="doc", shape="StartParticipantReplicationRequest", member="reconnectWindowSeconds")`: "If the participant disconnects and then reconnects within the specified interval, replication will continue to be `ACTIVE`. Default: 0." The stored `reconnect_window_seconds` is read by no code path at all (grep for `reconnect_window_seconds` in `src/` matches only the struct field and the assignment in `src/ops/start_participant_replication.rs:346-348`).

Test coverage: `tests/all/disconnect_participant.rs` contains no `replica`/`replication` assertion (grep for `replica|replication` in that file: no matches), and `tests/all/participant_replication_stop.rs` only covers the explicit stop — so no test pins the behaviour described here.

## Steps to Reproduce

1. CreateStage twice (a source and a destination stage).
2. CreateParticipantToken on the source stage with capabilities ["PUBLISH"]; note the minted participantId.
3. Record the publisher joining the source stage the way the crate's own tests seed the data plane: `state::join_stage_session(&mut store, source_stage_arn, JoinedParticipant { participant_id, published: true, .. })` (see tests/all/disconnect_participant.rs:172).
4. StartParticipantReplication with sourceStageArn=<source>, destinationStageArn=<destination>, participantId=<id>; expect 200.
5. DisconnectParticipant with stageArn=<source>, participantId=<id>; expect 200.
6. ListParticipantReplicas with sourceStageArn=<source>, participantId=<id>: the replica is still reported with replicationState "ACTIVE" even though the publisher is now DISCONNECTED.
7. ListParticipants on the destination stage's activeSessionId: the replicated participant is still reported state "CONNECTED", and GetStage on the destination still reports a non-empty activeSessionId.
