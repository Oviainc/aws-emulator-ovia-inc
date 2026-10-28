Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5cb591631d698c9098c00693287b08686d77b0d1

# Deleting a DESTINATION stage ends the replications into it without the REPLICATION_STOPPED occurrence the surviving source session gets when a caller stops them

_Filed: 2026-10-28T18:05:05Z_
_Operation: DeleteStage | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5cb591631d698c9098c00693287b08686d77b0d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteStage` ends every replication the deleted stage is an end of through `crate::state::end_replications_of_stage`, whose own doc states that "the stage that survives this delete is left exactly as it would be had each of those replications been stopped instead".

For the replications the deleted stage SOURCES, the helper honours that: each is moved to `STOPPED` and the replica it put into the surviving DESTINATION stage leaves through `release_ended_replica` -> `leave_stage_session`, which records the departure occurrences.

For the replications INTO the deleted stage, the surviving stage is the SOURCE stage, and the records are simply dropped by `replicas.retain(|replica| replica.destination_stage_arn != stage_arn)` with nothing written to that surviving session. The one thing `StopParticipantReplication` writes into the SOURCE session — the `REPLICATION_STOPPED` occurrence carrying `destinationStageArn` / `destinationSessionId` — is never recorded. The crate has no other writer of that event name.

Result: after `DeleteStage(destination)`, the publisher's own session in the surviving source stage reports a `REPLICATION_STARTED` occurrence with no counterpart, and its `replicationType` silently falls back to `NONE`, so `ListParticipantEvents` on the publisher no longer explains why. The exact same end of the exact same replication, reached through `StopParticipantReplication`, does record it. This is a path that skips an occurrence the crate already records from a sibling handler, not a new feature.

## Evidence

The helper's own stated rule — `src/state.rs:2950-2956`:

```
/// A replica is a participant of its source stage replicated into its destination stage, so
/// neither end of a replication can outlive the stage it names: a stage going away ends
/// every replication it sources and every replication into it. Ending one is not just
/// dropping its record — the replica that replication put into the DESTINATION stage leaves
/// that stage's session, through the one place an ended replication's replica leaves
/// ([`release_ended_replica`]) — so the stage that survives this delete is left exactly as
/// it would be had each of those replications been stopped instead.
```

The code under it, for the replications INTO the deleted stage — `src/state.rs:2982-2987`:

```
    // The replications sourced from this stage go with its key; the ones replicated INTO it
    // are dropped from the stages that source them.
    state.participant_replicas.remove(stage_arn);
    for replicas in state.participant_replicas.values_mut() {
        replicas.retain(|replica| replica.destination_stage_arn != stage_arn);
    }
```

Nothing between those lines records an occurrence in the surviving SOURCE session.

The sibling end of the same replication DOES record it — `src/ops/stop_participant_replication.rs:239-256`:

```
        // Replication stopping is an occurrence during the session the replicated publisher
        // is in, the counterpart of the one its start recorded: the model marks
        // `Event$destinationStageArn` and `Event$destinationSessionId` "applicable only if
        // the event name is REPLICATION_STARTED or REPLICATION_STOPPED", so this event
        // carries the destination the stopped replication named. ...
        let mut event = crate::state::ParticipantEvent::new(
            crate::state::EVENT_REPLICATION_STOPPED,
            &stopped.participant_id,
        );
        event.destination_stage_arn = Some(stopped.destination_stage_arn.clone());
        event.destination_session_id = Some(stopped.destination_session_id.clone());
        crate::state::record_participant_event(
            &mut store,
            &stopped.source_stage_arn,
            &stopped.source_session_id,
            event,
        );
```

It is the ONLY writer of that name in the crate (grep for `EVENT_REPLICATION_STOPPED` matches only `src/state.rs:2334` — the constant — and `src/ops/stop_participant_replication.rs:246`).

`DeleteStage` routes both directions through the helper — `src/ops/delete_stage.rs:167-174`:

```
        // So does every replication the stage is an end of: ... so both halves happen in the
        // one place that ends a stage's replications (`crate::state::end_replications_of_stage`).
        crate::state::end_replications_of_stage(&mut store, &input.arn);
```

Test coverage: `tests/all/delete_stage_ends_replications.rs:595-663`
(`deleting_a_destination_stage_leaves_the_publisher_in_its_own_stage`) is the only test for this direction, and it asserts only the publisher's `state`, the stage's `activeSessionId` and the remaining `replicas`; it never reads the publisher's occurrences, so nothing pins the missing event. The sibling behaviour IS pinned, in `tests/all/participant_replication_stop.rs:596-608`:

```
        let mut expected_order: Vec<String> =
            vec!["REPLICATION_STARTED".to_string(); 1 + row.other_destinations];
        expected_order.push("REPLICATION_STOPPED".to_string());
        assert_eq!(
            ordered, expected_order,
            "case {case:?}: a stop is recorded after the starts it ends, \
             got {publisher_events:?}"
        );
```

## Steps to Reproduce

1. CreateStage twice: stage A (source) and stage B (destination).
2. CreateParticipantToken on stage A (default capabilities include PUBLISH); note the returned participantId P.
3. StartParticipantReplication with sourceStageArn=A, destinationStageArn=B, participantId=P — 200.
4. GetStage on A and note its activeSessionId S.
5. ListParticipantEvents(stageArn=A, sessionId=S, participantId=P): the occurrences include REPLICATION_STARTED naming B.
6. DeleteStage with arn=B — 200.
7. ListParticipantEvents(stageArn=A, sessionId=S, participantId=P) again: still only REPLICATION_STARTED — no REPLICATION_STOPPED was recorded, and ListParticipants(A,S) now reports P with replicationType NONE.
8. Compare the supported path: repeat steps 1-5 and call StopParticipantReplication(A,B,P) instead — the same ListParticipantEvents call now reports REPLICATION_STOPPED carrying destinationStageArn=B and destinationSessionId.
