Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d25a8df08abf64a2b119d49e5e317e50f9894e06

# CreateEnvironmentConnector answers 200 with a DELETING connector and mints nothing

_Filed: 2026-10-26T01:22:38Z_
_Operation: CreateEnvironmentConnector | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d25a8df08abf64a2b119d49e5e317e50f9894e06`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateEnvironmentConnector resolves the "one connector of each type per environment" rule through `crate::state::live_connector_of_type`, which deliberately counts a connector that is still `DELETING` as occupying the type. The handler then branches on the stored record's metadata: if the request names the same `applianceFqdn` and the same resolved `secretArn`, it returns `Ok` with that stored record as the "idempotent" answer and mints nothing.

That makes a delete-then-recreate of the same connector (same type, same appliance, same secret) answer HTTP 200 while creating nothing, as long as the previous connector has not yet settled from `DELETING` to `DELETED` (`crate::state::TRANSITION_DELAY`). The caller is handed `connectorId` of the record that is on its way out, with `state: DELETING`; a moment later that id is `DELETED`, so every follow-up — `UpdateEnvironmentConnector`, `CreateEntitlement`, `ListVmEntitlements` — answers `ResourceNotFoundException` for the connector the create reported as created. The connector the caller asked for never exists, and nothing in the response says so.

The crate itself states the opposite rule in two places: a `DELETING` connector is one that "can no longer be used", and `CreateEntitlement`/`DeleteEntitlement`/`ListVmEntitlements` all resolve a connector id through `live_connector`, which refuses a `DELETING` record as not found. The same `if` block already does the right thing for a request that names DIFFERENT metadata — it refuses with the modelled `ValidationException` saying the type is already taken — so the type-is-not-free decision and the idempotency decision disagree inside one branch: the only input for which a `DELETING` connector yields a success is the one where the success is indistinguishable from a create that worked.

(Distinct from the already-fixed `CreateEnvironmentConnector` wrong-behavior issue, which was about `Connector$secretArn` echoing a bare secret name.)

## Evidence

The handler's idempotent branch — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/create_environment_connector.rs:266-275:
```
        if let Some(existing) = crate::state::live_connector_of_type(environment, &input.r#type) {
            if existing.appliance_fqdn == input.appliance_fqdn
                && existing.secret_arn == secret_arn
            {
                // `@idempotent`: the same create again reports the stored record and
                // mints nothing.
                return Ok(CreateEnvironmentConnectorOutput {
                    connector: Some(existing.to_wire(&environment_id)),
                });
            }
```

The lookup it uses admits a `DELETING` record — src/state.rs:2523-2533:
```
/// A connector that has reached `DELETED` does not count, exactly as a deleted
/// environment does not hold its name: the type is free for a new connector again. One
/// that is still `DELETING` does count — its connection to the appliance is still being
/// torn down, so the type is not free yet.
pub fn live_connector_of_type<'a>(
...
        connector.connector_type == connector_type && connector.state != CONNECTOR_DELETED
```

The crate's own rule about what a `DELETING` connector is — src/state.rs:2539-2550:
```
/// ... a connector the service has started deleting resolves to nothing: a
/// `DELETING` connector's connection to the appliance is being torn down and a
/// `DELETED` one is gone, so neither is a connector new work can be tracked through
...
/// [`live_connector_of_type`] draws the line one state later on purpose: a type whose
/// connector is still being deleted is not free for a new connector yet, while the
/// connector itself can no longer be used.
```

The sibling path that applies that rule — src/ops/create_entitlement.rs:235-243:
```
        if crate::state::live_connector(environment, &input.connector_id)
            .filter(|connector| connector.connector_type == crate::state::CONNECTOR_TYPE_VCENTER)
            .is_none()
        {
            return Err(CreateEntitlementErrorModeled::ResourceNotFoundException(
                crate::state::connector_not_found(&input.connector_id),
```

Test coverage: tests/all/connector_deletion.rs:398-441 pins the intended answer one state later — it calls `settle(&router)`, asserts the old connector is `DELETED` ("The type the deleted connector held is free again: 'You can create only one connector of each type per environment' counts the LIVE ones."), then re-creates the same type with the SAME `appliance_fqdn` and SAME `secret_identifier` and asserts `assert_ne!(replacement["connectorId"], json!(connector_id), ...)` and `assert_member(case, &replacement, "state", &json!("CREATING"))`. No test in tests/all/ creates a connector while the previous one of that type is still `DELETING` (tests/all/connectors.rs:741-789 only repeats a create against a LIVE connector), so refusing or minting in that window leaves every existing assertion intact.

## Steps to Reproduce

1. Create an environment and let its CREATING -> CREATED transition settle.
2. Call CreateEnvironmentConnector with {type: "VCENTER", applianceFqdn: "vcenter.evs.example.com", secretIdentifier: <arn>} and note the returned connectorId C1.
3. Call DeleteEnvironmentConnector for C1 — it answers 200 with state DELETING (the move to DELETED is owed ~500 ms later).
4. Immediately (inside that window) call CreateEnvironmentConnector again with exactly the same type, applianceFqdn and secretIdentifier.
5. It answers HTTP 200 reporting connectorId C1 with state DELETING, and stores nothing new.
6. Wait for the transition to settle and call ListEnvironmentConnectors: C1 is DELETED and the environment holds no live connector; UpdateEnvironmentConnector or CreateEntitlement for C1 now answers ResourceNotFoundException, so the connector the second create reported never existed.
