Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4507d6d6dd116d6bbaed085269ed053454966346

# A composition whose stage has no participants never stops or is reclaimed

_Filed: 2026-10-27T14:24:02Z_
_Operation: StartComposition | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4507d6d6dd116d6bbaed085269ed053454966346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's `StartComposition` documentation enumerates the conditions on which "Composition stops and the resource is deleted". The crate implements exactly one of them — "When StopComposition is called" — plus the stage-deletion case, and never records a deadline for the two clock-driven ones:

* "After a 1-minute timeout, when all participants are disconnected from the stage."
* "After a 1-minute timeout, if there are no participants in the stage when StartComposition is called."

`CompositionRecord` carries only `active_at` (the `STARTING` -> `ACTIVE` deadline) and `ended_at` (written exclusively by `begin_stop`, which only `StopComposition` calls). `state_at` therefore knows just two transitions, and `is_reclaimed` can never fire for a composition nobody explicitly stopped. Consequences a client observes:

1. `StartComposition` against a stage with no participants answers 200 and the composition reports `ACTIVE` forever, instead of stopping and being deleted after the documented timeout.
2. `DisconnectParticipant` (and `DeleteIngestConfiguration` with `force: true`) can empty the stage completely; the handler walks the sessions and the ingest configurations but never looks at `store.compositions`, so the composition keeps running.
3. Because the composition never reaches a finished stop, `ComAmazonawsIvsrealtime…State::composition_using` keeps returning it, so `DeleteEncoderConfiguration` answers `ConflictException` (409) forever for every encoder configuration that composition's destinations name — the resource becomes permanently undeletable.

The crate already owns the machinery this needs (a stored deadline materialised by `CompositionRecord::materialize_due` / `materialize_due_compositions`, which every composition reader already calls), and `DeleteStage` already reads the very same documentation clause, so this is a rule the crate applies in one place and skips in the others, not a mechanism it lacks. No comment, doc or test in the crate states a reason for the divergence.

## Evidence

Smithy, `smithy_query(query="doc", shape="StartComposition")`:
> "A Composition is an ephemeral resource that exists after this operation returns successfully. Composition stops and the resource is deleted: … After a 1-minute timeout, when all participants are disconnected from the stage. After a 1-minute timeout, if there are no participants in the stage when StartComposition is called."

The same sentences are carried verbatim in the handler's own header — `src/ops/start_composition.rs:46-47`:
```
//! called. After a 1-minute timeout, when all participants are disconnected from the stage.
//! After a 1-minute timeout, if there are no participants in the stage when
```

But the record holds no deadline for them — `src/state.rs:1412-1415`:
```
    /// The instant the clock owes this composition the `STARTING` -> `ACTIVE`
    /// transition: its start plus [`COMPOSITION_ACTIVATION_SPAN`]. Stored, never
    /// slept on.
    pub active_at: SystemTime,
```
and `state_at` knows only two transitions — `src/state.rs:1465-1477`:
```
    pub fn state_at(&self, now: SystemTime) -> &str {
        match self.state.as_str() {
            COMPOSITION_STATE_STARTING if now >= self.active_at => COMPOSITION_STATE_ACTIVE,
            COMPOSITION_STATE_STOPPING
                if self
                    .ended_at
                    .is_some_and(|ended_at| now >= ended_at + COMPOSITION_STOP_SPAN) =>
```
while reclamation needs `ended_at`, which only `StopComposition`'s `begin_stop` ever sets — `src/state.rs:1635-1640`:
```
    pub fn is_reclaimed(&self, now: SystemTime) -> bool {
        self.state_at(now) == COMPOSITION_STATE_STOPPED
            && self
                .ended_at
                .is_some_and(|ended_at| now >= ended_at + COMPOSITION_STOP_SPAN * 2)
```
`src/state.rs:1619-1621` (`begin_stop`, the only writer of `ended_at`) is reached only from `src/ops/stop_composition.rs:188` (`composition.begin_stop(now);`).

The crate reads the same clause and applies it in `DeleteStage` only — `src/ops/delete_stage.rs:175-183`:
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
whereas `DisconnectParticipant`, which disconnects the last participant of a stage, touches no composition at all — `src/ops/disconnect_participant.rs:227-245` writes only through `leave_stage_session` and `clear_stage_association`, and the handler's closing comment is `src/ops/disconnect_participant.rs:247-249`:
```
        // `DisconnectParticipantResponse` models no member at all, so the operation's
        // whole answer is its modelled 200 plus the state written above.
```

Blocked delete follows from `src/ops/delete_encoder_configuration.rs:172-182`:
```
        if let Some(composition) = store.composition_using(&input.arn) {
```
with `src/state.rs:3376-3381`:
```
    pub fn composition_using(&self, arn: &str) -> Option<&CompositionRecord> {
        self.compositions
            .values()
            .filter(|composition| composition.is_using_resources() && composition.references(arn))
```

Test coverage: `tests/all/compositions.rs:14-17` pins only the `STARTING` -> `ACTIVE` clock transition and `tests/all/composition_stop.rs` only the `StopComposition` path; `tests/all/compositions.rs:33-34` pins "goes away with the stage it composites". No test or comment in the crate states a reason for leaving the two participant-driven timeouts unimplemented (`grep -n '1-minute|all participants are disconnected|no participants in the stage'` matches only `src/ops/start_composition.rs:46-47` and the model JSON).

## Steps to Reproduce

1. CreateStage (no participantTokenConfigurations), so the stage has no session and no participants.
2. CreateEncoderConfiguration, note its `arn`.
3. StartComposition with `stageArn` = the stage and one `destinations` entry whose `channel.encoderConfigurationArn` is that encoder configuration. Observe HTTP 200 and `composition.state == "STARTING"`.
4. Wait well past the documented 1-minute timeout and call GetComposition with the composition ARN. Expected per the model: the composition has stopped and the resource is deleted (404). Actual: HTTP 200 with `state == "ACTIVE"` and no `endTime`.
5. Call DeleteEncoderConfiguration with the encoder configuration ARN. Expected: it succeeds, because the composition AWS documents as deleted is gone. Actual: HTTP 409 `ConflictException`, permanently, because `composition_using` still reports the never-reclaimed composition.
6. Variant for the second clause: CreateStage, join a participant over the data plane, StartComposition, then DisconnectParticipant for that participant so the stage is empty. GetComposition still answers `ACTIVE` indefinitely.
