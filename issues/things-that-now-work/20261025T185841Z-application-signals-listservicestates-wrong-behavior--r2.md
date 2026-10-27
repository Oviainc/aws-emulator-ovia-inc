Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8c118d1d43e9fad6226f410894186130d2b1ac53

# ListServiceStates' "latest change" ignores instrumentation-configuration change records that ListEntityEvents reports

_Filed: 2026-10-25T18:58:41Z_
_Operation: ListServiceStates | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8c118d1d43e9fad6226f410894186130d2b1ac53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate keeps TWO kinds of dated change records against a service entity: the writes of a stored SLO, and the creation of a dynamic instrumentation configuration for a `Service`/`Environment` pair. `ListEntityEvents` reports both (it walks `self.slos` AND `self.instrumentation_configurations`), but `ListServiceStates` computes its single `LatestChangeEvents` entry through `latest_change_in_window`, which walks `self.slos` only.

So for a service that has an SLO and, later, an instrumentation configuration, the two reads disagree about the same entity's most recent change: `ListEntityEvents` reports the `CreateInstrumentationConfiguration` event first (newest first), while `ListServiceStates` reports the older `CreateServiceLevelObjective`/`UpdateServiceLevelObjective` event as "the one most recent change recorded for its service inside the window". The state module's own docs say both reads are built from the same change records and must not disagree, so one of the two is wrong — `ListServiceStates` silently drops a change record the crate itself holds and reports elsewhere.

## Evidence

`entity_change_events` states which records a change event can come from, and reads both kinds:

- `src/state.rs:4155-4158`: "A change event is a DATED CHANGE this emulator has a record of, and the records it holds against an entity are the stored SLOs configured on it (dated by the `CreatedTime` of the create and the `LastUpdatedTime` of an update that followed it) and the instrumentation configurations applied to it (dated by their `CreatedAt`)."
- `src/state.rs:4218-4232`: `for (scope, set) in &self.instrumentation_configurations { let attributes = service_entity_attributes(&scope.service, &scope.environment); ... collect(ChangeRecord { at: stored.created_at, arn: &stored.arn, event_name: INSTRUMENTATION_CREATED_EVENT_NAME }, &attributes); }`

The other read claims to report the latest of those records, but only scans SLOs:

- `src/state.rs:2438-2440` (`observed_service_states` doc): "Each reported entry carries the one most recent change recorded for its service inside the window ([`Self::latest_change_in_window`])"
- `src/state.rs:2425-2430` (`latest_change_in_window`): `self.slos .values() .filter(|slo| names_same_service(entity_of(slo).key_attributes.as_ref(), identity)) .flat_map(slo_change_records) .filter(|record| record.inside(window)) .min_by_key(ChangeRecord::order_key)` — nothing reads `self.instrumentation_configurations`.

The crate also asserts the two reads must agree, and the module doc claims they cannot:

- `src/state.rs:4179-4185`: "`ListServiceStates` reads the same SLO writes ([`Self::latest_change_in_window`]) ... both read the writes of an SLO off [`slo_change_records`] and report one as [`ChangeRecord::event`] builds it"
- `tests/all/change_event_attribution.rs:4`: "report a `ChangeEvent`: `ListEntityEvents` and `ListServiceStates`" and `tests/all/change_event_attribution.rs:304`: `for (read, event) in [("ListEntityEvents", &from_history), ("ListServiceStates", &from_state)]` — the suite asserts both reads report the SAME write, but every seeded case is SLO-only (`create_slo`/`update_slo`, lines 288-302), so the overlap case is untested.

That the instrumentation event exists on the `ListEntityEvents` side is pinned by `tests/all/entity_events.rs:366-368`: `label: "a service an instrumentation configuration is applied to", entity: instrumented.clone(), expected: vec![(INSTRUMENTATION_CREATED, instrumented.clone())]` — but that seeded service (`INSTRUMENTED_SERVICE`) carries no SLO, so it never reaches `ListServiceStates`.

## Steps to Reproduce

1. Create an SLO whose SLI config names `KeyAttributes` `{"Type":"Service","Name":"checkout","Environment":"prod"}` (`POST /slo`).
2. Create a dynamic instrumentation configuration for the same entity: `POST /create-instrumentation-configuration` with `Service=checkout`, `Environment=prod`, `InstrumentationType=BREAKPOINT`, `SignalType=SNAPSHOT` and a `Location`.
3. Call `POST /events` (`ListEntityEvents`) with `Entity={"Type":"Service","Name":"checkout","Environment":"prod"}` over a window covering both writes: the first (newest) `ChangeEvents` entry has `EventName="CreateInstrumentationConfiguration"`.
4. Call `POST /service/states` (`ListServiceStates`) over the same window: the entry for that service reports `LatestChangeEvents[0].EventName="CreateServiceLevelObjective"` with the older timestamp.
5. Expected: both reads agree on the most recent recorded change for the entity; actual: `ListServiceStates` ignores the instrumentation-configuration record entirely.
