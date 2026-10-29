Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f7cb83054c40d7ded49f98a292169f4a09dea4c9

# A clock-due replication end is timestamped at the call that happens to materialise it, not at the deadline it fell due — so eventTime, the session endTime and the composition's empty-stage deadline all depend on when somebody called

_Filed: 2026-10-29T14:40:33Z_
_Operation: ListParticipantEvents | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f7cb83054c40d7ded49f98a292169f4a09dea4c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`materialize_due_replications` writes down the end of every replication whose publisher stayed away for its whole reconnect window. It takes the instant to decide against (`now: SystemTime`) and uses it ONLY to test the deadline; every record the end then writes is stamped with a fresh `SystemTime::now()` read at the call site instead of with the deadline the end fell due at.

Observable consequences for a replication owed its end at t0 that nobody materialises until t1:

1. `ListParticipantEvents` on the SOURCE session reports the `REPLICATION_STOPPED` occurrence with `eventTime` = t1, although the occurrence happened at t0 — and `record_participant_event`'s own doc says "the instant is the one it occurred at".
2. `GetStageSession` / `ListStageSessions` on the DESTINATION stage report `endTime` = t1 for the session the departing replica emptied, although the session "ends after the last participant leaves the stage", which was t0 — plus a `PUBLISH_STOPPED`/`LEFT` occurrence stamped t1.
3. A composition of the destination stage has its empty-stage timeout measured from t1, so it keeps composing and is then given a fresh `COMPOSITION_EMPTY_STAGE_TIMEOUT` — while `CompositionRecord::empty_stage_stop_at`'s doc states that "what the timeout is measured from is the instant the stage was actually left empty".

The same crate's composition clock does the opposite and says why: `CompositionRecord::materialize_due` passes the owed instant into `begin_stop(due)` precisely so "a caller that reads late sees exactly what one reading on time saw". The replication clock's own doc claims that same invariant while the code under it breaks it: the reported instants are a function of when a caller happened to call.

This is distinct from the already-filed StopComposition/DeleteEncoderConfiguration issues, which are about handlers that do not run the replication clock at all; here the clock runs but stamps the wrong instants.

## Evidence

The replication clock receives the instant, uses it only as a predicate, and hands nothing down — `src/state.rs:3103-3106` and `src/state.rs:3113-3141`:
```
pub fn materialize_due_replications(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    now: SystemTime,
) {
...
            let owed = replication.is_active()
                && replication
                    .reconnect_deadline
                    .is_some_and(|deadline| deadline <= now);
...
        for ended in &due {
            record_replication_stopped(state, ended);
        }
        for ended in &due {
            release_ended_replica(state, ended);
        }
```
Neither `record_replication_stopped` nor `release_ended_replica` takes an instant, and the writes they reach read the wall clock afresh — `src/state.rs:2453`:
```
        event_time: SystemTime::now(),
```
under a doc that says otherwise, `src/state.rs:2428-2430`:
```
/// The ONE place an event record is created, so every seam that observes an occurrence
/// writes the same representation of it: the instant is the one it occurred at and the
/// position is the session's next occurrence position, ...
```
and `src/state.rs:2906`, `src/state.rs:2950`, `src/state.rs:2955`:
```
            session.end_time = Some(SystemTime::now());
...
    observe_replication_publisher(state, stage_arn, session_id, participant_id, SystemTime::now());
...
    state.observe_stage_occupancy(stage_arn, SystemTime::now());
```

The claim the code breaks, on the clock itself — `src/state.rs:3098-3102`:
```
/// Each of those ends was owed at that deadline whether or not anybody called, so this
/// changes nothing a caller reading at the same instant would have seen differently — which
/// is why a reporting operation may call it, exactly as it may materialise what the clock
/// owes a composition
```
and on the member the third symptom lands on — `src/state.rs:1427-1431`:
```
    /// Written only by [`Self::observe_stage_occupancy`], through the store-wide
    /// `ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::observe_stage_occupancy`
    /// that every change to a stage's participants goes through, so what the timeout is
    /// measured from is the instant the stage was actually left empty. Stored, never
    /// slept on.
```

The sibling clock in the same crate anchors to the owed instant and states that rule — `src/state.rs:1454-1461`:
```
        // The stop the clock owes a composition whose stage has been empty for a whole
        // [`COMPOSITION_EMPTY_STAGE_TIMEOUT`] begins AT the instant it fell due, not at
        // this call: everything the stop owes afterwards is owed from there, so a caller
        // that reads late sees exactly what one reading on time saw.
        if self.is_running() {
            if let Some(due) = self.empty_stage_stop_at.filter(|due| now >= *due) {
                self.begin_stop(due);
            }
        }
```

Test coverage: no test exercises a LATE materialisation. `tests/all/replication_follows_publisher_connection.rs:341-360` overwrites every recorded deadline with `Some(SystemTime::now())` immediately before triggering the clock, so the owed instant and the call instant always coincide there; `tests/all/replication_end_recorded_in_source_session.rs` asserts nothing about `eventTime` (no `eventTime`/`endTime` match in the file).

## Steps to Reproduce

1. CreateStage twice: a source stage S and a destination stage D.
2. Seed the data plane: `state::join_stage_session(&mut store, S, JoinedParticipant { participant_id: P, published: true, .. })` and note S's session id.
3. StartParticipantReplication sourceStageArn=S, destinationStageArn=D, participantId=P, omitting `reconnectWindowSeconds` (documented default 0). D's session now holds the replica.
4. Take the publisher out of S over the data plane at t0: `state::leave_stage_session(&mut store, S, <S's session>, P)`. The replication's `reconnect_deadline` is now t0 and the end is owed, but unmaterialised.
5. Wait (or advance the stored deadline into the past and wait) so that a long span t1 - t0 elapses with NO API call in between.
6. Call ListParticipantEvents(stageArn=S, sessionId=<S's session>, participantId=P): the `REPLICATION_STOPPED` event carries `eventTime` ≈ t1, not t0.
7. Call GetStageSession(stageArn=D, sessionId=<D's session>): `endTime` ≈ t1, although the replica — the session's only participant — left when the end fell due at t0.
8. Repeat the scenario but call GetStage once right after step 4: the same two reads now report instants ≈ t0, so the values reported depend on when an unrelated call ran.
