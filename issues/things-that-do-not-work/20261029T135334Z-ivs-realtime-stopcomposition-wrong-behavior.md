Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 02c390dd8e861c3d98b16a550da31d4601fa546f

# StopComposition advances only the composition clock, so it stops (200) a composition the clock already owes an empty-stage stop whose state a read at the same instant reports differently

_Filed: 2026-10-29T13:53:34Z_
_Operation: StopComposition | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `02c390dd8e861c3d98b16a550da31d4601fa546f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StopComposition` writes down what the clock owes the compositions and then decides, over that store, whether the composition is still running (409 `ConflictException` if not, 404 if its ephemeral resource was reclaimed). It never writes down what the clock owes the REPLICATIONS, which is the step `GetComposition` and `ListCompositions` perform FIRST because a replication ending is one of the ways the composited stage comes to hold nobody and so earns the composition its own stop deadline.

Consequence: in a store holding a replication whose reconnect deadline is already past but unmaterialised, the composited destination stage still looks occupied, so no empty-stage deadline is ever written and the composition stays `ACTIVE` indefinitely — `StopComposition` keeps answering 200 and keeps re-stamping a caller's end time. The same sequence with one `GetComposition` call inserted ends with the composition `STOPPING`/`STOPPED` and finally reclaimed, so `StopComposition` answers 409 and then 404. The handler's own comment states the opposite ("the stop acts on the state the clock has already reached: one whose ephemeral resource it has reclaimed is not found here rather than stopped a second time"), and the composition's end instant is then recorded from this call rather than from the deadline the clock had already reached.

## Evidence

`StopComposition` advances only one of the two clocks — `src/ops/stop_composition.rs:156-168`:
```
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        // A composition's state moves on the clock, so the stop acts on the state the
        // clock has already reached: one whose ephemeral resource it has reclaimed is not
        // found here rather than stopped a second time.
        store.materialize_due_compositions(now);

        let Some(composition) = store.composition_in_mut(&region, &input.arn) else {
```

The sibling readers of the same records run the replication clock first, and say why — `src/ops/get_composition.rs:160-165`:
```
        // What the clock owes a replication FIRST: a replication whose publisher stayed
        // away for its whole reconnect window ends with its replica leaving the
        // destination stage, which is one of the ways the stage a composition composites
        // comes to hold nobody and so earns that composition its own deadline.
        crate::state::materialize_due_replications(&mut store, ::std::time::SystemTime::now());
        store.materialize_due_compositions(::std::time::SystemTime::now());
```
and `StartComposition` — the other mutation over the same records — runs both as well, `src/ops/start_composition.rs:301-313`:
```
        // What the clock owes a replication is written down FIRST of all: a replication
        // whose publisher stayed away for its whole reconnect window ends with its replica
        // leaving the destination stage, so whether the stage this start composites holds
        // anybody is decided over the participants a caller reading at this instant sees.
        crate::state::materialize_due_replications(&mut store, now);
        ...
        store.materialize_due_compositions(now);
```

The empty-stage deadline a stop decision turns on is written only from the replication materialisation — `src/state.rs:3139-3141` (`materialize_due_replications` -> `release_ended_replica`), `src/state.rs:3024-3029` (`release_ended_replica` -> `leave_stage_session`) and `src/state.rs:2955`:
```
    state.observe_stage_occupancy(stage_arn, SystemTime::now());
```
with `src/state.rs:1735-1737` the only writer of the deadline:
```
        } else if self.empty_stage_stop_at.is_none() {
            self.empty_stage_stop_at = Some(now + COMPOSITION_EMPTY_STAGE_TIMEOUT);
        }
```

Test coverage: `tests/all/composition_stop.rs` stops compositions of stages that were never emptied by a clock-due replication, and `tests/all/composition_stage_emptied_by_replication.rs` always empties the stage through an explicit `StopParticipantReplication`/`DeleteStage` (both of which call `materialize_due_replications`) — no test exercises `StopComposition` over a store with an unmaterialised due replication.

## Steps to Reproduce

1. CreateEncoderConfiguration; CreateStage twice (source S, destination D).
2. Seed the data plane: `state::join_stage_session(&mut store, S, JoinedParticipant { participant_id: P, published: true, .. })`.
3. StartParticipantReplication sourceStageArn=S, destinationStageArn=D, participantId=P, no `reconnectWindowSeconds` (default 0) — D now holds the replica.
4. StartComposition with stageArn=D and a channel destination naming the encoder configuration; expect 200.
5. Take the publisher out of S over the data plane: `state::leave_stage_session(&mut store, S, <S's session>, P)` — the replication's deadline is now past but unmaterialised.
6. Wait longer than `state::COMPOSITION_EMPTY_STAGE_TIMEOUT + 2 * state::COMPOSITION_STOP_SPAN`, then call StopComposition with arn=<composition arn>: it answers 200 and stamps `endTime` at this call.
7. Repeat the scenario, but call GetComposition once right after step 5 and then wait the same span: GetComposition now answers 404 (the resource was reclaimed) and StopComposition answers 404 instead of 200 — the stop's answer depended on whether an unrelated read ran.
