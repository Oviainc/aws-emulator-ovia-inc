Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 097d82b3cbc72c7bfc910d8eaa03c5f730d6b648

# The attributes a re-replication sets on the replica are silently dropped when the destination session still holds the participant's record, because join_stage_session's rejoin branch never takes attributes over

_Filed: 2026-10-29T17:01:53Z_
_Operation: StartParticipantReplication | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `097d82b3cbc72c7bfc910d8eaa03c5f730d6b648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartParticipantReplication` merges the request's `attributes` over the publisher's own and hands the merged map to `crate::state::record_replicated_participant`, which declares in its own doc that "the replica carries the ones the replication merged for it" and passes that map into `crate::state::join_stage_session` as the joining participant's `attributes`.

`join_stage_session` has two paths. The FIRST-JOIN path (`new_participant_record`) stores `joined.attributes`. The REJOIN path — taken whenever the destination session already holds a record for that participant id, whatever state it is in — takes over every other member the join supplies (`state`, `published`, `recording_state`, `redundant_ingest`, `ingest_configuration_arn`, `client`, `protocol`) and never touches `attributes`. So the merged map the replication computed is thrown away and the replica goes on reporting the attributes of the PREVIOUS replication.

The path is reachable through the public API alone. A replica only leaves its destination session when its replication ends; if that session still holds another participant it does NOT end, so the departed replica's record stays in it as `DISCONNECTED`. A second `StartParticipantReplication` of the same publisher into the same stage then resolves the same session (`ensure_active_session` returns the stage's still-active session), finds the record not `CONNECTED`, and goes down the join path — which is the rejoin branch, because the session holds the record. `GetParticipant` / `ListParticipants` — the only readers of a replica's attributes, since `ParticipantReplica` models none — keep answering the first replication's values.

The handler's refusal of an oversized `attributes` map and the merge it performs therefore have no effect on anything a caller can read in this case: the operation accepts and acknowledges a documented input member whose value no read ever reports.

## Evidence

The model states the member's effect — `smithy_query(query="doc", shape="StartParticipantReplicationRequest", member="attributes")`:
```
<p>Application-provided attributes to set on the replicated participant in the destination stage. … </p> <p>These attributes are merged with any attributes set for this participant when creating the token. If there is overlap in keys, the values in these attributes are replaced.</p>
```

The handler merges them and stores them on the replication — `src/ops/start_participant_replication.rs:336-341`:
```
        // "These attributes are merged with any attributes set for this participant when
        // creating the token. If there is overlap in keys, the values in these attributes
        // are replaced" — so the source participant's own attributes first, then the
        // request's over them.
        let mut attributes = participant.attributes.clone();
        attributes.extend(requested_attributes);
```
and hands them to the one place a replicated participant is recorded — `src/ops/start_participant_replication.rs:372-374`:
```
        if let Err(refusal) =
            crate::state::record_replicated_participant(&mut store, &record, &participant)
```

That helper states the rule it is the owner of — `src/state.rs:3314-3331`:
```
    // The two ends of one replication: the session each record goes into, the attributes
    // that record carries, and whether the participant there IS the replica. The
    // publisher carries the attributes its own identity was issued with; the replica
    // carries the ones the replication merged for it.
    let ends = [
        (
            &replica.source_stage_arn,
            &replica.source_session_id,
            &identity.attributes,
            false,
        ),
        (
            &replica.destination_stage_arn,
            &replica.destination_session_id,
            &replica.attributes,
            true,
        ),
    ];
```
and passes them through the join — `src/state.rs:3358-3362` and `:3391-3407`:
```
    for (stage_arn, session_id, attributes, is_replica) in ends {
        // Only a participant the session holds as present keeps its record untouched: one
        // whose record has left the session is brought back in by the join below.
        let present = session_participant(state, stage_arn, session_id, &replica.participant_id)
            .is_some_and(|present| present.state == PARTICIPANT_STATE_CONNECTED);
```
```
        let joined = JoinedParticipant {
            participant_id: replica.participant_id.clone(),
            user_id: identity.user_id.clone(),
            attributes: attributes.clone(),
            published: true,
```
```
        if join_stage_session(state, stage_arn, joined).is_none() {
```

But `join_stage_session`'s rejoin branch drops them — `src/state.rs:2725-2753`:
```
    // A participant the session already holds is REJOINING it, so it keeps its id, its
    // first join instant and its page position.
    let rejoined = {
        let session = session_mut(state, stage_arn, &session_id)?;
        if let Some(rejoined) = session
            .participants
            .iter_mut()
            .find(|participant| participant.participant_id == joined.participant_id)
        {
            rejoined.state = PARTICIPANT_STATE_CONNECTED.to_string();
            rejoined.published = rejoined.published || joined.published;
            if let Some(recording_state) = joined.recording_state.clone() {
                rejoined.recording_state = recording_state;
            }
            rejoined.redundant_ingest = joined.redundant_ingest;
            if joined.ingest_configuration_arn.is_some() {
                rejoined.ingest_configuration_arn = joined.ingest_configuration_arn.clone();
            }
            // A rejoin is a fresh connection, so what the data plane observes about it
            // — the client it came from and the protocol it publishes with — is what
            // the participant carries from now on. A member this connection did not
            // observe leaves the earlier observation standing, the rule every other
            // optional member of this block follows
            // ([`ParticipantClient::observe`]).
            rejoined.client.observe(&joined.client);
            if joined.protocol.is_some() {
                rejoined.protocol = joined.protocol.clone();
            }
            Some(rejoined.clone())
        } else {
```
`attributes` is not an optional member of `JoinedParticipant` (it is a bare `HashMap`, `src/state.rs:2659`), so the block's own "a member this connection did not observe leaves the earlier observation standing" rule cannot be what justifies skipping it — the join always supplies it, and the first-join path does store it (`src/state.rs:2825-2828`: `attributes: joined.attributes,`).

The only read of a replica's attributes goes through the untouched record — `src/state.rs:2269`:
```
            attributes: (!self.attributes.is_empty()).then(|| self.attributes.clone()),
```

Test coverage: no test reaches the rejoin branch for a replica. `tests/all/participant_replication_stop.rs:781-826` (`a_publisher_can_be_replicated_again_once_its_replication_stopped`) restarts a replication, but the destination stage held only that one replica, so its session ENDED and the retest asserts a NEW session is opened (`:810-815`, "the destination stage's session ended when the replica left") — which takes the first-join path. `tests/all/participant_replication.rs:325-340` and `tests/all/replicated_participant_views.rs:457-462` only cover attributes on a FIRST replication, and `tests/all/participant_replication.rs:944` offers "different attributes" only to assert the 409 a second RUNNING replication gets.

## Steps to Reproduce

1. CreateStage twice: a source stage S and a destination stage D.
2. CreateParticipantToken against S twice (default capabilities, so both carry PUBLISH): note participantIds Q and P.
3. StartParticipantReplication with sourceStageArn=S, destinationStageArn=D, participantId=Q — this opens S's and D's sessions and puts Q's replica into D.
4. StartParticipantReplication with sourceStageArn=S, destinationStageArn=D, participantId=P, attributes={"seat":"left"}.
5. GetStage(arn=D) and note activeSessionId; GetParticipant(stageArn=D, sessionId=<that>, participantId=P) — attributes is {"seat":"left"}, as the request asked.
6. StopParticipantReplication with sourceStageArn=S, destinationStageArn=D, participantId=P. P's replica leaves D, but D's session survives because Q is still in it (GetStage(D) still reports the same activeSessionId).
7. StartParticipantReplication again with sourceStageArn=S, destinationStageArn=D, participantId=P, attributes={"seat":"right"} — answers 200, and ListParticipantReplicas(sourceStageArn=S, participantId=P) shows the new replica ACTIVE in the SAME destinationSessionId as the stopped one.
8. GetParticipant(stageArn=D, sessionId=<the same session>, participantId=P): attributes is still {"seat":"left"}. The second request's attributes were accepted, merged and stored on the replication record, but no read reports them.
