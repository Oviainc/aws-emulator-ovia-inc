Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 9644ddf994ccf47d3ab59a1dffbb24b4a89d5fe4

# StartParticipantReplication answers 404 for a replica participant the same stage reports CONNECTED and publishing

_Filed: 2026-10-29T13:32:51Z_
_Operation: StartParticipantReplication | Class: error-response_

> _Note: this issue was identified against the code base at commit `9644ddf994ccf47d3ab59a1dffbb24b4a89d5fe4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler resolves the publisher to replicate through `crate::state::stage_participant`, which answers only "what identity did this service ISSUE against this stage" — the stage's own participant-token records and the ingest configurations associated with it. A REPLICA participant (the record a replication puts into its destination stage) carries an id issued against the SOURCE stage, so `stage_participant` never finds it and a replication whose `sourceStageArn` is the stage that holds the replica is refused with `ResourceNotFoundException` (HTTP 404) — even though that same stage's own readers (`ListParticipants`, `GetParticipant`) report the participant as `CONNECTED` with `published: true` and `replicationType: REPLICA`.

Three places in this crate state the opposite rule:

1. `stage_participant`'s own doc says this is NOT the question to ask about "is this participant in this stage" and names the helper that is (`participant_attached_to_stage`).
2. `participant_attached_to_stage` declares itself the ONE answer to that question precisely so that an operation acting on a participant of a stage and the readers reporting that stage's participants cannot disagree — and the sibling handler `DisconnectParticipant` resolves its participant through it (the identical 404-for-a-replica defect was already fixed there).
3. `materialize_due_replications` is written for exactly the state this handler can never produce: its loop exists because "that replica can itself be the publisher of a FURTHER replication sourced from that stage".

So the clock walk guards a state the start path refuses to create, and one handler calls a participant of a stage non-existent while the stage's readers (and its sibling mutator) call it present. Everything the start needs for a replica is already on the record the destination session holds (its `user_id` and `attributes`, and `published`/`state`), so no new data is required.

## Evidence

The handler asks the issued-identity question and 404s on a miss — services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/src/ops/start_participant_replication.rs:247-258:
```
// The participant has to be one of the SOURCE stage's, and it has to be a
// publisher: this operation replicates "a publishing participant" (the model's
// own doc for it) ...
let Some(participant) =
    crate::state::stage_participant(&store, &input.source_stage_arn, &input.participant_id)
else {
    return Err(crate::state::resource_not_found(
        StartParticipantReplicationErrorModeled::ResourceNotFoundException,
        &input.participant_id,
    ));
};
```

That helper's own doc says it is the wrong question for "is this participant in this stage" — src/state.rs:4286-4288:
```
/// This is NOT the question "is this participant in this stage": a stage holds whatever
/// participant records its sessions hold, including one whose identity was issued against
/// another stage, which is [`participant_attached_to_stage`].
```

And the helper it points at declares itself the single answer, so readers and mutators cannot disagree — src/state.rs:4339-4342:
```
/// The ONE answer to "does this stage have this participant", so an operation that acts on
/// a participant of a stage and the readers that report that stage's participants
/// (`ListParticipants`, `GetParticipant`) cannot disagree about which participants the
/// stage has.
```
which checks the session records first — src/state.rs:4349-4357 (`let recorded = state.stage_sessions.get(stage_arn).is_some_and(...)` … `recorded || stage_participant(...).is_some()`). The sibling mutator uses it: src/ops/disconnect_participant.rs:209-218 (`if !crate::state::participant_attached_to_stage(&store, &input.stage_arn, &input.participant_id) { return Err(resource_not_found(..)) }`).

The crate's clock walk is written for the state this handler cannot create — src/state.rs:3107-3111:
```
// Ending one replication takes its replica out of the destination stage, and that
// replica can itself be the publisher of a FURTHER replication sourced from that stage,
// whose own deadline is then recorded and can already be due — so the walk repeats
// until this store holds no due replication.
```

The replica really is recorded as a publishing participant of the destination stage — src/state.rs:3391-3395 joins it with `published: true`, and the suite pins that view: tests/all/replicated_participant_views.rs:675-685:
```
assert_eq!(text(replica, "state"), CONNECTED,
    "case {case:?}: a replica is in the destination stage, got {listed}");
assert_eq!(replica["published"], json!(true),
    "case {case:?}: a replica carries the publisher's media into the destination stage, got {listed}");
```

No existing test pins the refusal this issue reports: the only 404 case the replication suite fixes is a token identity of ANOTHER stage that the source stage holds no record of — tests/all/participant_replication.rs:798-808 (`case: "a participant of the destination stage rather than the source"`, `participantId: elsewhere`, status 404).

## Steps to Reproduce

1. CreateStage three times with one region's credentials: stage A, stage B, stage C.
2. CreateParticipantToken with stageArn = A and no `capabilities` (the documented default grants PUBLISH); note the returned `participantToken.participantId` as P.
3. StartParticipantReplication with sourceStageArn = A, destinationStageArn = B, participantId = P → HTTP 200.
4. GetStage with arn = B and note the non-empty `stage.activeSessionId` as S_B; then ListParticipants with stageArn = B, sessionId = S_B → P is reported with state CONNECTED, published true, replicationType REPLICA (GetParticipant on the same triple answers 200 with the same view).
5. StartParticipantReplication with sourceStageArn = B, destinationStageArn = C, participantId = P → HTTP 404 ResourceNotFoundException "Resource: P not found", contradicting the CONNECTED, publishing participant B's own readers just reported, and leaving the state `materialize_due_replications` is written for (a replica sourcing a further replication) unreachable.
