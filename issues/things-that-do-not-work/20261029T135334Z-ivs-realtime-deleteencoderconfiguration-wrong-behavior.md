Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 02c390dd8e861c3d98b16a550da31d4601fa546f

# The in-use refusal advances only the composition clock, not the replication clock the composition's own stop depends on, so a configuration held by a composition of an emptied stage can never be deleted

_Filed: 2026-10-29T13:53:34Z_
_Operation: DeleteEncoderConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `02c390dd8e861c3d98b16a550da31d4601fa546f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteEncoderConfiguration` writes down what the clock owes the COMPOSITIONS (`store.materialize_due_compositions(...)`) and then refuses over `store.composition_using(&input.arn)`. It does NOT write down what the clock owes the REPLICATIONS (`crate::state::materialize_due_replications`), which is the step `GetComposition` and `ListCompositions` run FIRST, for exactly this reason: ending a replication takes its replica out of the destination stage, which is one of the ways the stage a composition composites comes to hold nobody and so earns that composition the empty-stage stop that eventually reclaims it.

Because the empty-stage deadline is written ONLY from `leave_stage_session` -> `state.observe_stage_occupancy(..)` (state.rs:2955), and the only thing that can perform that departure for a replica is `materialize_due_replications` -> `release_ended_replica` -> `leave_stage_session` (state.rs:3139-3141, 3024-3029), a store in which a replication's reconnect deadline is already past but unmaterialised keeps the destination stage looking occupied forever. In that store this handler answers `ConflictException` (409) on every call, for as long as the caller never issues some OTHER operation that happens to materialise replications. Insert a single `ListCompositions`/`GetComposition` call and the same sequence ends with the composition reclaimed and the delete answering 200. The verdict of a delete therefore depends on whether an unrelated read was made, which is precisely what the handler's own comment and `composition_using`'s own doc promise it does not.

The identical copy of this gap is `src/ops/delete_storage_configuration.rs:153-158` + `:181` (same comment, same single `materialize_due_compositions` call, same `composition_using` refusal) and needs the same fix: materialise the replications before the compositions, in the order `list_compositions.rs:201-207` already uses.

## Evidence

The handler claims parity with a read at the same instant but advances only one of the two clocks — `src/ops/delete_encoder_configuration.rs:150-155`:
```
        let mut store = state.lock();
        // The transitions the clock has already made due are written down FIRST, so the
        // refusal below decides against the same store a read at this instant would
        // report: a composition whose stop the clock has already finished is gone, and
        // holds nothing back.
        store.materialize_due_compositions(::std::time::SystemTime::now());
```
and then refuses over it — `src/ops/delete_encoder_configuration.rs:172`:
```
        if let Some(composition) = store.composition_using(&input.arn) {
```

The lookup it refuses over states the parity that is broken — `src/state.rs:3848-3852`:
```
    /// The ONE lookup for "is a composition using this resource", so what a delete of an
    /// encoder or storage configuration refuses over is exactly what the compositions
    /// `GetComposition` reports say. Call it after
    /// [`Self::materialize_due_compositions`], so a composition whose stop the clock has
    /// already finished holds nothing back.
```

But `GetComposition` runs BOTH clocks, replications first — `src/ops/get_composition.rs:160-165`:
```
        // What the clock owes a replication FIRST: a replication whose publisher stayed
        // away for its whole reconnect window ends with its replica leaving the
        // destination stage, which is one of the ways the stage a composition composites
        // comes to hold nobody and so earns that composition its own deadline.
        crate::state::materialize_due_replications(&mut store, ::std::time::SystemTime::now());
        store.materialize_due_compositions(::std::time::SystemTime::now());
```
and so does `src/ops/list_compositions.rs:202-207` (same comment, same two calls).

The empty-stage deadline is reachable only through that replication materialisation — `src/state.rs:3139-3141`:
```
        for ended in &due {
            release_ended_replica(state, ended);
        }
```
`src/state.rs:3024-3029` (`release_ended_replica` -> `leave_stage_session`) and `src/state.rs:2951-2955`:
```
    // A departure can leave the stage with no participant in it at all, which is when the
    // timeout the model's `StartComposition` doc ends a composition on starts running for
    // every composition of that stage ("After a 1-minute timeout, when all participants
    // are disconnected from the stage").
    state.observe_stage_occupancy(stage_arn, SystemTime::now());
```

The sibling delete carries the identical gap — `src/ops/delete_storage_configuration.rs:153-158`:
```
        let mut store = state.lock();
        // The transitions the clock has already made due are written down FIRST, so the
        // refusal below decides against the same store a read at this instant would
        // report: a composition whose stop the clock has already finished is gone, and
        // holds nothing back.
        store.materialize_due_compositions(::std::time::SystemTime::now());
```

Test coverage: `tests/all/composition_stage_emptied_by_replication.rs` only ever empties the stage through an explicit `StopParticipantReplication`/`DeleteStage` (which do materialise) and always reads the composition before deleting the encoder configuration (`tests/all/composition_stage_emptied_by_replication.rs:543-544, 623`), so no test pins the delete-only path; `tests/all/encoder_configuration_in_use.rs` covers only a composition of an occupied stage.

## Steps to Reproduce

1. CreateEncoderConfiguration; note `encoderConfiguration.arn`.
2. CreateStage twice: a source stage S and a destination stage D.
3. Seed the data plane the way this crate's own suites do: `state::join_stage_session(&mut store, S, JoinedParticipant { participant_id: P, published: true, .. })`.
4. StartParticipantReplication with sourceStageArn=S, destinationStageArn=D, participantId=P and no `reconnectWindowSeconds` (documented default 0), so D now holds the replica.
5. StartComposition with stageArn=D and a destination naming the encoder configuration; expect 200 (the stage is occupied by the replica).
6. Take the publisher out of S over the data plane: `state::leave_stage_session(&mut store, S, <S's session>, P)`. The replication's reconnect deadline is now already past, but nothing has materialised it.
7. Call DeleteEncoderConfiguration with arn=<encoder arn> repeatedly, waiting well past `state::COMPOSITION_EMPTY_STAGE_TIMEOUT + 2 * state::COMPOSITION_STOP_SPAN`: every call answers 409 ConflictException, because the composition's stage still looks occupied and the composition is never reclaimed.
8. Now call ListCompositions (or GetComposition) once, wait `COMPOSITION_EMPTY_STAGE_TIMEOUT + 2 * COMPOSITION_STOP_SPAN`, and call DeleteEncoderConfiguration again: it answers 200. The delete's verdict depended on an unrelated read.
9. Repeat steps 1-8 with DeleteStorageConfiguration and an S3 destination for the identical result.
