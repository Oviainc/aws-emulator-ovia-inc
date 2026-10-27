Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9ce68f8efb447c5711e42ec7e2cb128c2507b7ba

# A composition outlives the emptying of its stage: DisconnectParticipant never ends the composition DeleteStage ends for the same reason

_Filed: 2026-10-27T08:23:49Z_
_Operation: DisconnectParticipant | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9ce68f8efb447c5711e42ec7e2cb128c2507b7ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model fixes the end conditions of a Composition on `StartComposition`: among them, "After a 1-minute timeout, when all participants are disconnected from the stage."

This crate implements that condition in exactly one place — `DeleteStage`, whose handler drops every composition of the stage it deletes and whose comment quotes that very sentence as the justification. The operation that actually disconnects participants does not: `DisconnectParticipant` moves the named participant to `DISCONNECTED` through `crate::state::leave_stage_session`, which ends the session and clears the stage's `activeSessionId` when the departure empties it, and then returns — neither the handler nor the helper reads or writes `store.compositions` (the handler never even calls `materialize_due_compositions`, which every other composition-aware handler does).

So after disconnecting every participant of a stage, the stage reports no active session and every participant reports `DISCONNECTED`, yet `GetComposition` / `ListCompositions` keep reporting the composition of that stage as `STARTING`/`ACTIVE` indefinitely, its destinations `ACTIVE`, and `DeleteEncoderConfiguration` keeps refusing with `ConflictException` because `composition_using` still finds it. Deleting the stage instead reclaims the composition at once. One documented rule, two different answers depending on which operation emptied the stage.

The same omission reaches the two other callers of `leave_stage_session` in this crate: `DeleteIngestConfiguration` with `force=true` (which disconnects the configuration's publishers) and `StopParticipantReplication` (which takes the replica out of the destination stage's session). All three can empty a stage without the composition of that stage ever ending.

## Evidence

Smithy, `smithy_query(query="doc", shape="StartComposition")` — the end conditions of the ephemeral resource:

```
A Composition is an ephemeral resource that exists after this operation returns successfully. Composition stops and the resource is deleted:
  - When StopComposition is called.
  - After a 1-minute timeout, when all participants are disconnected from the stage.
  - After a 1-minute timeout, if there are no participants in the stage when StartComposition is called.
```

The sibling that applies that rule — src/ops/delete_stage.rs:175-183:
```
        // So does every composition of this stage: a composition composites one stage,
        // and this operation shuts the stage down "disconnecting all participants", which
        // is one of the conditions the model's own `StartComposition` doc ends a
        // composition on ("Composition stops and the resource is deleted … when all
        // participants are disconnected from the stage"). A composition cannot outlive
        // the only stage it has to composite.
        store
            .compositions
            .retain(|_, composition| composition.stage_arn != input.arn);
```

The handler that skips it — src/ops/disconnect_participant.rs:222-249 is the whole remainder of `handle`, and `store.compositions` appears nowhere in it:
```
        // "Disconnects a specified participant from a specified stage": the participant
        // leaves the stage through the one place a participant leaves, so it is reported
        // `DISCONNECTED` and a session it emptied ends — the same departure a client
        // dropping off the data plane records. ...
        for session_id in &connected_sessions {
            crate::state::leave_stage_session(
                &mut store,
                &input.stage_arn,
                session_id,
                &input.participant_id,
            );
        }
```

The shared helper does not do it either — src/state.rs:2646-2690 (`leave_stage_session`) writes only the participant's state, the session's `end_time` and the stage's `active_session_id`:
```
        leaving.state = PARTICIPANT_STATE_DISCONNECTED.to_string();
        let emptied = session
            .participants
            .iter()
            .all(|participant| participant.state == PARTICIPANT_STATE_DISCONNECTED);
        if emptied {
            session.end_time = Some(SystemTime::now());
        }
...
    if session_ended {
        if let Some(stage) = state.stages.get_mut(stage_arn) {
            if stage.active_session_id.as_deref() == Some(session_id) {
                stage.active_session_id = None;
            }
        }
    }
```

`materialize_due_compositions` is called by get_composition.rs:160, list_compositions.rs:202, start_composition.rs:308, stop_composition.rs:161 and delete_encoder_configuration.rs:155 — and by no participant-departure path (grep over the crate).

Test coverage: the DeleteStage half IS pinned, tests/all/compositions.rs:1168-1194:
```
    // Deleting the stage disconnects every participant, which is one of the conditions
    // the model's own StartComposition doc ends a composition on: nothing composites a
    // stage that no longer exists.
...
    assert_eq!(
        status, 404,
        "a composition must not outlive the stage it composites, got {status} {response}"
    );
```
The disconnect half is unpinned: tests/all/disconnect_participant.rs states its three rules at lines 10-26 and never mentions a composition, and grep for `composition` in tests/all/disconnect_participant.rs finds nothing.

## Steps to Reproduce

1. CreateStage, and note the stage ARN.
2. Bring one participant into the stage over the data-plane seam the crate's own suites use (`state::join_stage_session`, exactly as tests/all/disconnect_participant.rs:167-175 does), then GetStage to read `activeSessionId`.
3. CreateEncoderConfiguration, then StartComposition with that stageArn and a single `channel` destination carrying that `encoderConfigurationArn`.
4. GetComposition on the returned ARN: 200, state STARTING/ACTIVE.
5. DisconnectParticipant for that stage and participant — 200. GetStage now reports `activeSessionId: ""` and ListParticipants reports the participant `DISCONNECTED`, i.e. all participants are disconnected from the stage.
6. Expected (the rule DeleteStage applies from the same Smithy sentence): the composition ends and is reclaimed, so GetComposition answers 404 and DeleteEncoderConfiguration succeeds.
7. Actual: GetComposition still answers 200 with the composition ACTIVE and its destinations ACTIVE, indefinitely, and DeleteEncoderConfiguration still answers 409 ConflictException because `state::composition_using` finds it. Replacing the DisconnectParticipant call with DeleteStage reclaims the composition at once (tests/all/compositions.rs:1168-1194), so the two paths disagree about one documented rule.
