Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a

# DeleteEnvironmentConnector reports an environmentSummary whose environmentStatus was computed before the connector was torn down

_Filed: 2026-10-28T11:51:44Z_
_Operation: DeleteEnvironmentConnector | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0c9cda5ef496fef3a9fe3e02c7d9493d80d1134a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteEnvironmentConnector answers with an `environmentSummary`, and `EnvironmentSummary$environmentStatus` is the roll-up of the environment's stored `checks` — one of which, `CONNECTOR_HEALTH`, is the aggregate over exactly the connectors this handler just changed. The handler recomputes the checks only at the TOP of the call (inside `state.materialise_due_transitions()`), then moves the connector to `DELETING` and builds the summary WITHOUT recomputing, so the `environmentStatus` it reports is the one that held while the connector was still live.

Every sibling handler of this crate that mutates a check input and then reports an environment recomputes first, and says so in a comment: `DeleteEnvironmentHost` and `CreateEnvironmentHost` call `state.recompute_environment_checks()` before building their `environmentSummary`, and `DeleteEnvironment` calls it before building its `Environment`. This one path skips it.

Consequence: the summary the delete hands back can disagree with the summary a read taken immediately afterwards reports. A connector whose record has decided `FAILED` (`CONNECTOR_CREATE_FAILED` / `CONNECTOR_UPDATE_FAILED`, the states `StoredConnector::decided_result` maps to `CHECK_FAILED`) makes `CONNECTOR_HEALTH` `FAILED`; once the delete moves it to `DELETING` the connector decides nothing, so a later `ListEnvironments`/`GetEnvironment` rolls the environment up to `PASSED`/`UNKNOWN` while the delete's own response still says `FAILED`. The crate's own test asserts those two reads agree, so the handler is one check-input change away from contradicting it. The stored `checks` also stay stale in the store until the next reporting call recomputes them.

## Evidence

The missing recomputation — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/delete_environment_connector.rs:289-313 (connector state changed, then the summary built with no recompute):
```
            connector.state = crate::state::CONNECTOR_DELETING.to_string();
...
        let deleted = connector.to_wire(&environment_id);
        if let Some(due_at) = due_at {
...
            environment.purge_finished_connectors();
            state.note_transition_due(due_at);
        }
        // The environment the connector was deleted FROM, as it stands after the delete.
        let environment_summary = state
            .environment_in_region(&region, &input.environment_id)
            .map(|environment| environment.to_summary())
```

The sibling rule, stated and applied — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/delete_environment_host.rs:280-286:
```
        // The environment's health checks read its hosts — the host count, and the cores
        // a license has to cover — so they are recomputed before the summary is built.
        state.recompute_environment_checks();
        // The environment the host was deleted FROM, as it stands after the delete.
        let environment_summary = state
            .environment_in_region(&region, &input.environment_id)
            .map(|environment| environment.to_summary())
```
and services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/delete_environment.rs:288-291:
```
        // The deletion just changed the environment's state, and its health checks are
        // derived from that state, so they are recomputed before it is reported: an
        // environment that is going away reports no check.
        state.recompute_environment_checks();
```

That the connectors ARE a check input — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:1796-1806:
```
        // "Aggregate health across all connectors in the environment": the roll-up of what
        // each connector the environment holds has decided about its own connectivity
        // ([`StoredConnector::decided_result`]). One impaired connector impairs the
        // aggregate; ...
        let connector_health = roll_up_results(
            self.connectors
                .values()
                .filter_map(|connector| connector.decided_result()),
        );
```
and that a `DELETING` connector decides nothing while `CREATE_FAILED`/`UPDATE_FAILED` decide `FAILED` — state.rs:788-793:
```
    pub fn decided_result(&self) -> Option<&'static str> {
        match self.state.as_str() {
            CONNECTOR_ACTIVE => Some(CHECK_PASSED),
            CONNECTOR_CREATE_FAILED | CONNECTOR_UPDATE_FAILED => Some(CHECK_FAILED),
            _ => None,
```
and that `environmentStatus` is read off the stored `checks` — state.rs:1664-1666, 1874-1880 (`environment_status: Some(self.environment_status().to_string())` in `to_summary`).

Test coverage of the invariant this can break — services/service-com-amazonaws-evs-amazonelasticvmwareservice/tests/all/connector_deletion.rs:153-155 and 366-370:
```
/// The summary `ListEnvironments` reports for one environment — what the delete's own
/// `environmentSummary` is checked against, so the two reads of one record agree.
...
        assert_eq!(
            body["environmentSummary"],
            listed_summary(&router, case, &environment).await,
            "case {case:?}: the summary must be the environment's own, got {body}"
        );
```
The rows it runs only reach connectors in `ACTIVE`/`CREATING`, where `HOST_COUNT` already decides the roll-up, so nothing in the suite exercises the divergent case (`tests/all/environment_connector_health.rs:230-232` is where a `CREATE_FAILED` connector is driven into the store).

## Steps to Reproduce

1. Create an environment and wait past the transition delay so it is CREATED with four hosts (so HOST_COUNT is PASSED).
2. CreateEnvironmentConnector with type VCENTER, then put that connector's stored record into CONNECTOR_CREATE_FAILED — the state `StoredConnector::decided_result` maps to CHECK_FAILED (the drive `tests/all/environment_connector_health.rs:230-232` uses).
3. GetEnvironment: `environmentStatus` is FAILED, because CONNECTOR_HEALTH rolls up the failed connector.
4. Call DeleteEnvironmentConnector for that connector and read `environmentSummary.environmentStatus` from the response: it still reports FAILED, computed before the connector moved to DELETING.
5. Call ListEnvironments (or GetEnvironment) immediately afterwards: the same environment now reports PASSED, because the DELETING connector decides nothing — the two reads of one record disagree, which `tests/all/connector_deletion.rs:366-370` asserts must not happen.
