Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5c9a4c43e4da79c1a8ad9e3e8eedc72bdd65daf0

# An accepted update writes the new configuration before the snapshot phase it documents as applying it later

_Filed: 2026-10-25T20:40:25Z_
_Operation: UpdateEnvironment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5c9a4c43e4da79c1a8ad9e3e8eedc72bdd65daf0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateEnvironment` builds the merged spec, validates it, and then stores it on the record immediately (`stored.spec = merged;`) before calling `begin_update`, which moves the environment into `CREATING_SNAPSHOT` and queues the `UPDATING` and `AVAILABLE` phases.

That contradicts the rule the crate states for those phases: `begin_update`'s own doc says the snapshot happens BEFORE the change is applied and that "the change itself is applied in the second" phase. As written, the change is applied in neither phase — it is applied at request time, one whole `TRANSITION_SPAN` before the update settles.

The observable effect is a single self-contradicting response: a `GetEnvironment` issued while the update is in flight reports `Status: CREATING_SNAPSHOT` and `LastUpdate.Status: PENDING` — i.e. "this update has not landed" — while every member the patch carried already reads back with its new value. The mechanism for landing a change at a later phase already exists on this record: `PendingTransition` carries a per-phase effect (`last_update_status`) that `EnvironmentRecord::settle` applies when the phase comes due, and this path bypasses it for the spec.

## Evidence

The write happens before the phases — services/service-com-amazonaws-mwaa-amazonmwaa/src/ops/update_environment.rs:374-381:
```
        merged.validate().map_err(validation)?;

        stored.spec = merged;
        stored.begin_update(
            now,
            crate::state::UPDATE_SOURCE_API,
            input.worker_replacement_strategy,
        );
```

The rule that code breaks, in this crate — src/state.rs:865-870 (`EnvironmentRecord::begin_update`'s doc): "Amazon MWAA snapshots the environment's database cluster BEFORE it applies the change (so it can restore the environment if the update fails), which is what the first of those statuses reports; the change itself is applied in the second." The same sentence is repeated in the test suite's own narration — tests/all/environment_lifecycle.rs:929-932: "The update is in flight: Amazon MWAA snapshots the environment's database before it applies the change, so the first status an accepted update reports is CREATING_SNAPSHOT".

The deferral mechanism the path skips — src/state.rs:722-729 `PendingTransition { to, last_update_status, due_at }` and src/state.rs:779-791 `EnvironmentRecord::settle`, which applies the queued per-phase effect (`last_update.status = status`) when the phase comes due; `begin_update` (src/state.rs:884-898) queues `UPDATING` and then `AVAILABLE`/`SUCCESS`, but carries no spec to apply.

Test coverage: tests/all/environment_lifecycle.rs:933-944 reads the environment during the snapshot phase and asserts ONLY `Status == CREATING_SNAPSHOT` and `LastUpdate.Status == PENDING`; the patched members are asserted only after the walk settles (lines 947-1009). tests/all/environment_update_status_sequence.rs:296-311 likewise reads the changed member back only once the environment is `AVAILABLE`. So no test pins the merged members being visible during the in-flight phases.

## Steps to Reproduce

1. Boot the emulator.
2. `PUT /environments/PhaseEnv` with the required members and `AirflowVersion: 2.10.1`; wait past `TRANSITION_SPAN` so it settles to `AVAILABLE`.
3. `PATCH /environments/PhaseEnv` with `{"AirflowVersion": "2.11.0"}` -> 200.
4. Immediately (within the snapshot phase) `GET /environments/PhaseEnv`.
5. Expected per the documented phase semantics: `Status: CREATING_SNAPSHOT`, `LastUpdate.Status: PENDING`, and `AirflowVersion` still `2.10.1` — the change lands when the `UPDATING`/`AVAILABLE` phase comes due. Observed: `Status: CREATING_SNAPSHOT` and `LastUpdate.Status: PENDING` alongside `AirflowVersion: 2.11.0`, so one response reports the update as both pending and already applied.
