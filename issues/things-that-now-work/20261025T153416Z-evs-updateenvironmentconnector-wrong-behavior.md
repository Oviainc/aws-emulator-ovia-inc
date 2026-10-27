Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: efbe6fec395222c954ec73ab018b3346b3354542

# UpdateEnvironmentConnector resurrects a DELETING connector instead of refusing it

_Filed: 2026-10-25T15:34:16Z_
_Operation: UpdateEnvironmentConnector | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `efbe6fec395222c954ec73ab018b3346b3354542`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateEnvironmentConnector` resolves the connector with a raw map lookup that only excludes `DELETED`:

```rust
let connector = environment
    .connectors
    .get_mut(&input.connector_id)
    .filter(|connector| connector.state != crate::state::CONNECTOR_DELETED)
```

A connector whose teardown `DeleteEnvironmentConnector` already started is in state `DELETING` with `pending = PendingConnectorState::starting_now(CONNECTOR_DELETED)`. That record passes this filter, so the update proceeds: it writes the new `applianceFqdn`/`secretArn`, sets `connector.state = CONNECTOR_UPDATING` and OVERWRITES `connector.pending` with a pending move to `CONNECTOR_ACTIVE`. The owed move to `DELETED` is thrown away, so the deletion the caller already accepted silently never happens and the connector comes back as `ACTIVE`.

That is corruption of stored state, not just a status oddity: once the connector is `ACTIVE` again, `live_connector_of_type` sees it, so `CreateEnvironmentConnector` keeps refusing a new connector of that type ("only one connector of each type per environment"), and the connector can never reach `DELETED` unless a second delete is issued. Every sibling handler of this crate that resolves a connector id for work routes it through `crate::state::live_connector`, which draws the line one state earlier on purpose; this handler is the only one that does not.

## Evidence

The crate's own helper states the rule AND names this operation as following it — `services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:2460-2485`:

```
/// The live connector `connector_id` names in this environment, if any: the record a
/// request that wants to USE a connector resolves to.
///
/// The id is scoped to the environment, exactly as `UpdateEnvironmentConnector` reads
/// it, and a connector the service has started deleting resolves to nothing: a
/// `DELETING` connector's connection to the appliance is being torn down and a
/// `DELETED` one is gone, so neither is a connector new work can be tracked through
...
/// [`live_connector_of_type`] draws the line one state later on purpose: a type whose
/// connector is still being deleted is not free for a new connector yet, while the
/// connector itself can no longer be used.
pub fn live_connector<'a>(
    environment: &'a StoredEnvironment,
    connector_id: &str,
) -> Option<&'a StoredConnector> {
    environment
        .connectors
        .get(connector_id)
        .filter(|connector| {
            connector.state != CONNECTOR_DELETING && connector.state != CONNECTOR_DELETED
        })
}
```

The handler does NOT call it — `src/ops/update_environment_connector.rs:241-249`:

```rust
let connector = environment
    .connectors
    .get_mut(&input.connector_id)
    .filter(|connector| connector.state != crate::state::CONNECTOR_DELETED)
    .ok_or_else(|| {
        UpdateEnvironmentConnectorErrorModeled::ResourceNotFoundException(
            crate::state::connector_not_found(&input.connector_id),
        )
    })?;
```

and then discards the owed deletion — `src/ops/update_environment_connector.rs:297-310`:

```rust
let pending =
    crate::state::PendingConnectorState::starting_now(crate::state::CONNECTOR_ACTIVE);
connector.state = crate::state::CONNECTOR_UPDATING.to_string();
connector.modified_at_ms = crate::state::now_ms();
let due_at = pending.due_at;
connector.pending = Some(pending);
```

Sibling handlers that DO use the helper for the same member:
* `src/ops/create_entitlement.rs:235` — `if crate::state::live_connector(environment, &input.connector_id)`
* `src/ops/delete_entitlement.rs:240` — `if crate::state::live_connector(environment, &input.connector_id).is_none()`
* `src/ops/list_vm_entitlements.rs:243` — `if crate::state::live_connector(environment, &input.connector_id).is_none()`

The delete that leaves the record `DELETING` — `src/ops/delete_environment_connector.rs:283-289`:

```rust
let pending =
    crate::state::PendingConnectorState::starting_now(crate::state::CONNECTOR_DELETED);
let due_at = pending.due_at;
connector.state = crate::state::CONNECTOR_DELETING.to_string();
```

Test coverage: `tests/all/connectors.rs:806-868` (`an_environment_that_has_reached_deleted_holds_no_new_or_updated_connector`) only covers an update in a DELETED ENVIRONMENT; `tests/all/connector_deletion.rs:529,546` pins the connector as `DELETING` after the delete but never issues an update against it. No test pins the current behaviour, so nothing in the crate justifies the divergence.

## Steps to Reproduce

1. CreateEnvironment and wait for it to reach CREATED.
2. CreateEnvironmentConnector with type=VCENTER, applianceFqdn=vcenter.evs.example.com — note the returned connectorId.
3. Wait past crate::state::TRANSITION_DELAY so the connector settles to ACTIVE.
4. Call DeleteEnvironmentConnector for that connectorId: the response reports state DELETING (src/ops/delete_environment_connector.rs:286).
5. Immediately (within the 500ms TRANSITION_DELAY window) call UpdateEnvironmentConnector with the same environmentId/connectorId and applianceFqdn=vcenter-2.evs.example.com.
6. Expected: the request is refused (the connector the service has started deleting resolves to nothing, per state.rs:2463-2474). Actual: HTTP 200 with state UPDATING.
7. Wait past TRANSITION_DELAY and call ListEnvironmentConnectors: the connector reports state ACTIVE — the accepted deletion was silently cancelled and the record never reaches DELETED.
8. Call CreateEnvironmentConnector with type=VCENTER again: it is refused with "only one connector of each type can be created per environment", naming the connector that was supposed to be deleted.
