Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# DeleteStage drops a replication's records but leaves the replica participant CONNECTED in the destination stage, where no operation can ever remove it

_Filed: 2026-10-28T11:50:28Z_
_Operation: DeleteStage | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When a stage is the SOURCE of a running participant replication, `DeleteStage` deletes the replication records but never takes the replica participant back out of the DESTINATION stage's session — the step `StopParticipantReplication`, the other way one replication ends, performs through the crate's single departure helper `crate::state::leave_stage_session`.

After `DeleteStage(source)`:

* the destination stage's session still holds the replicated participant as `CONNECTED` (`ListParticipants` / `GetParticipant` report it, now with `replicationType: NONE` because the replication record is gone);
* nothing can ever remove it again. `StopParticipantReplication` answers 404 because the source stage no longer exists, and `DisconnectParticipant(destination, participantId)` also answers 404 because `crate::state::stage_participant` resolves a participant only from the stage's own participant tokens and ingest configurations — the token that minted this participant belonged to the deleted source stage;
* therefore the destination stage's session `endTime` is never set, its `activeSessionId` never clears, and a composition of the destination stage never reaches the empty-stage stop the model documents, because the phantom participant keeps `stage_has_participants` true forever.

The departure helper already exists and is already called from two other handlers, so this is a path that skips a helper the crate has, not a new feature.

## Evidence

The delete removes only the replication RECORDS — `src/ops/delete_stage.rs:167-174`:

```
// So does every replication the stage is an end of: a replica is a participant
// of its source stage replicated into its destination stage, so neither end can
// outlive the stage it names. The replicas sourced from this stage go with its
// key; the ones replicated INTO it are dropped from the stages that source them.
store.participant_replicas.remove(&input.arn);
for replicas in store.participant_replicas.values_mut() {
    replicas.retain(|replica| replica.destination_stage_arn != input.arn);
}
```

The sibling path that ends a replication DOES take the replica out of the destination session — `src/ops/stop_participant_replication.rs:258-282`:

```
// The replica is what the replication put into the destination stage, so stopping
// it takes that participant back out of the destination stage's session, through
// the one place a participant leaves a session
// (`crate::state::leave_stage_session`): the record stays and is reported
// `DISCONNECTED`, and the session ends if it was the last participant in it.
...
if !still_replicated {
    crate::state::leave_stage_session(
        &mut store,
        &stopped.destination_stage_arn,
        &stopped.destination_session_id,
        &stopped.participant_id,
    );
}
```

Why the leftover record is unreachable afterwards — `src/state.rs:3989-4026` (`stage_participant`, the lookup `DisconnectParticipant` gates on):

```
/// The participant `participant_id` of the stage `stage_arn`, or `None` when this
/// service issued no identity for that participant in that stage.
...
/// so the identities this store holds for a stage are
/// exactly its token records and the credentials of the ingest configurations associated
/// with it.
```

and `src/ops/disconnect_participant.rs:192-198`:

```
if crate::state::stage_participant(&store, &input.stage_arn, &input.participant_id).is_none()
{
    return Err(crate::state::resource_not_found(
        DisconnectParticipantErrorModeled::ResourceNotFoundException,
        &input.participant_id,
    ));
}
```

The model sentence the crate is working from (`smithy_query(query="doc", shape="DeleteStage")`): "Shuts down and deletes the specified stage (disconnecting all participants)."

Test coverage: `tests/all/replicated_participant_views.rs:759-808` (`a_participant_reports_only_the_replications_the_store_holds`) covers only deletion of the DESTINATION stage (`emulator.delete_stage(case, &stages[0]).await` where `stages` holds the destinations) and asserts "The participant record itself is untouched by the delete: the publisher is still in its own stage." No test covers deleting the SOURCE stage, so nothing pins the behaviour described here.

## Steps to Reproduce

1. CreateStage twice: stage A (source) and stage B (destination).
2. CreateParticipantToken on stage A (default capabilities include PUBLISH); note the returned participantId P.
3. StartParticipantReplication with sourceStageArn=A, destinationStageArn=B, participantId=P — 200.
4. GetStage on B and note its activeSessionId S; ListParticipants(stageArn=B, sessionId=S) reports P as CONNECTED with replicationType REPLICA.
5. DeleteStage with arn=A — 200.
6. ListParticipants(stageArn=B, sessionId=S) again: P is STILL reported CONNECTED (now replicationType NONE), and GetStageSession(B, S) carries no endTime.
7. Try to clean it up: StopParticipantReplication(A,B,P) answers 404 (stage A is gone) and DisconnectParticipant(stageArn=B, participantId=P) answers 404 (P is not one of B's own token/ingest identities), so the phantom participant can never leave.
8. Compare with the supported path: repeat steps 1-4 and call StopParticipantReplication(A,B,P) instead — P becomes DISCONNECTED in B and B's session ends.
