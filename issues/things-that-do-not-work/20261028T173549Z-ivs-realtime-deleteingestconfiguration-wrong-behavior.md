Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# A forced delete disconnects the publisher but leaves its replication ACTIVE and the replica CONNECTED in the destination stage

_Filed: 2026-10-28T17:35:49Z_
_Operation: DeleteIngestConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteIngestConfiguration` with `force: true` takes every publisher of the configuration out of the session it is in (`crate::state::leave_stage_session`) and then drops the configuration record. It writes nothing about the participant replications this store holds for that publisher.

An ingest-configuration publisher CAN be the source of a replication: `crate::state::stage_participant` resolves a configuration's minted participant with `publishes: true`, which is exactly what `StartParticipantReplication` requires. So after a forced delete of such a configuration:

* the `ParticipantReplicaRecord` keeps `replication_state = ACTIVE`, so `ListParticipantReplicas` still reports the replica as running while its publisher is `DISCONNECTED`,
* `GetParticipant`/`ListParticipants` on the source stage report the publisher `DISCONNECTED` while still reporting `replicationState: ACTIVE` for it (both read the stored replication through `crate::state::session_participant_replications`), and
* the destination stage still holds a `CONNECTED` participant that exists only because of that replication, so that stage never empties: its `activeSessionId` is never cleared and a composition of it never starts the empty-stage timeout, because `observe_stage_occupancy` turns on `stage_has_participants`.

This crate already owns the opposite rule in one place: `release_ended_replica` is declared "the ONE place an ended replication's replica leaves, whichever way the replication ended", and `DeleteStage` calls `end_replications_of_stage` for exactly this reason. The forced-delete path is the one publisher-removing path that calls neither.

## Evidence

The forced delete disconnects the publisher and writes nothing else about replication — `src/ops/delete_ingest_configuration.rs:195-213`:
```
        // With `force`, "the participant is disconnected from the stage, followed by
        // deletion of the IngestConfiguration": each publisher leaves the session it is
        // in through the one place a participant leaves
        // (`crate::state::leave_stage_session`), ...
        for publisher in &publishers {
            crate::state::leave_stage_session(
                &mut store,
                &configuration.stage_arn,
                &publisher.session_id,
                &publisher.participant_id,
            );
        }
        ...
        store.ingest_configurations.remove(&input.arn);
```
(`leave_stage_session`, `src/state.rs:2836-2907`, touches only the session records, the events and `observe_stage_occupancy` — it contains no reference to `participant_replicas`.)

The crate's own single-place rule it skips — `src/state.rs:2909-2923`:
```
/// Take the replica a now-ENDED replication put into its destination stage back out of
/// that stage's session.
///
/// The ONE place an ended replication's replica leaves, whichever way the replication
/// ended — a caller stopping it, or a stage it names being deleted — so no participant
/// stays [`PARTICIPANT_STATE_CONNECTED`] in a stage on behalf of a replication this store
/// no longer runs.
```
and the sibling that honours it — `src/ops/delete_stage.rs:167-174`:
```
        // So does every replication the stage is an end of: ... so both halves happen in
        // the one place that ends a stage's replications
        // (`crate::state::end_replications_of_stage`).
        crate::state::end_replications_of_stage(&mut store, &input.arn);
```

The state module's comment on the field that models this case asserts the premise this handler falsifies — `src/state.rs:2474-2478`:
```
    /// The window a disconnected publisher may reconnect in before replication ends,
    /// or [`DEFAULT_REPLICATION_RECONNECT_WINDOW_SECONDS`] when the request omitted it.
    /// Nothing disconnects an emulated publisher, so no clock reads it yet; it is
    /// recorded because it is what the caller asked replication to do.
    pub reconnect_window_seconds: i32,
```

Model, `smithy_query(query="doc", shape="DeleteIngestConfigurationRequest", member="force")`: "If this is set to `true` when a participant is actively publishing, **the participant is disconnected from the stage**, followed by deletion of the IngestConfiguration." and `smithy_query(query="doc", shape="StartParticipantReplicationRequest", member="reconnectWindowSeconds")`: "If the participant disconnects and then reconnects within the specified interval, replication will continue to be `ACTIVE`. Default: 0." — so a disconnect outside that window (the default 0) is not a state in which the replication stays ACTIVE.

An ingest publisher is replicable — `src/state.rs:4100-4106`:
```
        .map(|configuration| StageParticipant {
            // An ingest configuration is a publisher's credential: it carries the ingest
            // key a broadcast is published with, so its participant publishes.
            publishes: true,
```

Test coverage: `tests/all/delete_ingest_configuration.rs` and `tests/all/participant_replication_stop.rs` pin nothing here — grep for `replica|replication` in `tests/all/delete_ingest_configuration.rs` returns no match, and the replication-stop suite covers only the explicit `StopParticipantReplication`.

## Steps to Reproduce

1. CreateStage twice (a source and a destination stage).
2. CreateIngestConfiguration with stageArn=<source>, ingestProtocol=RTMPS; note `ingestConfiguration.arn` and `ingestConfiguration.participantId`.
3. Record the publisher joining the source stage the way this crate's own tests seed the data plane: `state::join_stage_session(&mut store, source_stage_arn, JoinedParticipant { participant_id: <participantId>, published: true, ingest_configuration_arn: Some(<arn>), .. })`.
4. StartParticipantReplication with sourceStageArn=<source>, destinationStageArn=<destination>, participantId=<participantId>; expect 200.
5. DeleteIngestConfiguration with arn=<arn>, force=true; expect 200.
6. ListParticipantReplicas with sourceStageArn=<source>, participantId=<participantId>: the replica is still reported with replicationState "ACTIVE" although its publisher is now DISCONNECTED.
7. ListParticipants on the destination stage's activeSessionId: the replicated participant is still reported state "CONNECTED", and GetStage on the destination stage still reports a non-empty activeSessionId, so that stage never empties.
