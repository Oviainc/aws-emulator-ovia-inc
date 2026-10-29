Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4e290f8d6b034b90409323db1641d26f5c9d1b54

# applianceFqdn is compared byte-exactly, so re-sending the stored FQDN in another case restarts the connector

_Filed: 2026-10-27T19:59:46Z_
_Operation: UpdateEnvironmentConnector | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4e290f8d6b034b90409323db1641d26f5c9d1b54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateEnvironmentConnector decides whether the request changed anything by comparing the requested `applianceFqdn` to the stored one as raw bytes. Everywhere else this crate compares an appliance FQDN it treats it as a DNS name and compares case-insensitively, with that rationale written into the code and pinned by a test. So a request that names the connector's own appliance in a different case (`VCENTER.EVS.EXAMPLE.COM` for a connector carrying `vcenter.evs.example.com`) is treated as a reconfiguration: the handler sets `state` to `UPDATING`, moves `connector.modifiedAt`, notes a pending transition back to `ACTIVE` and re-stamps the environment through `note_child_change` — exactly the three things the comment directly above it promises a no-op update will not do. While the connector sits in `UPDATING` its `decided_result()` is `None`, so `Connector$status` drops to `UNKNOWN` and `Connector$checks` disappears for the whole transition delay, and `Environment$checks`'s `CONNECTOR_HEALTH` aggregate loses this connector's `PASSED` — all for a request that asked for the value the connector already carries.

The same missing normalisation sits in the sibling create path: `CreateEnvironmentConnector`'s `@idempotent` repeat only reports the stored record when `existing.appliance_fqdn == input.appliance_fqdn` byte-for-byte, so repeating the create with the same appliance spelled in another case is refused with `ValidationException` ("only one connector of each type can be created per environment") instead of handing back the connector that already targets that appliance. One fix — comparing the FQDN the way `entitlements_sharing_vcenter` already does — covers both sites.

## Evidence

The handler's own rule, and the exact comparison that breaks it — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/update_environment_connector.rs:284-292:
```
        // The reconfiguration itself. A value the connector already carries is NOT a
        // change: the record keeps its state and its modifiedAt, so repeating the call
        // neither restarts the connector nor moves a stamp.
        let changed = match requested {
            Requested::ApplianceFqdn(appliance_fqdn) => {
                let changed = connector.appliance_fqdn != appliance_fqdn;
                connector.appliance_fqdn = appliance_fqdn.to_string();
                changed
            }
```
and the state it then writes for `changed == true` (update_environment_connector.rs:304-326): `connector.state = crate::state::CONNECTOR_UPDATING.to_string(); connector.modified_at_ms = modified_at_ms; ... environment.note_child_change(modified_at_ms);`.

The crate's own rule for comparing an appliance FQDN — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state.rs:1566-1570 and 1581-1585:
```
    /// The vCenter is the APPLIANCE the connector targets, not the connector record, so
    /// every connector of this environment reaching the same `applianceFqdn` counts —
    /// an entitlement created through a connector that was replaced by a new one for
    /// the same appliance is associated with that same vCenter. The comparison is
    /// case-insensitive because a DNS name is.
...
                other
                    .appliance_fqdn
                    .eq_ignore_ascii_case(&connector.appliance_fqdn)
```

That rule is pinned by this crate's own test — services/service-com-amazonaws-evs-amazonelasticvmwareservice/tests/all/connector_deletion.rs:590-601:
```
    // The replacement: the same type, for the SAME appliance, named by the other form of
    // its secret so nothing but the appliance ties the two records together.
    let replacement = create_connector(
        &router,
        case,
        &environment,
        VCENTER,
        // DNS names are case-insensitive, so the uppercase spelling is the same vCenter.
        &appliance_fqdn.to_uppercase(),
```

The sibling create path, same byte comparison — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/create_environment_connector.rs:281-300:
```
            if existing.is_live()
                && existing.appliance_fqdn == input.appliance_fqdn
                && existing.secret_arn == secret_arn
            {
                return Ok(CreateEnvironmentConnectorOutput {
...
            return Err(CreateEnvironmentConnectorErrorModeled::ValidationException(
```

Test coverage of the no-op rule exercises only the identical spelling, so nothing pins the current behaviour: tests/all/connectors.rs:524-543 `an_update_that_asks_for_the_stored_value_changes_nothing`, whose rows are `("the stored FQDN again", "applianceFqdn", FQDN)` and `("the stored secret again", "secretIdentifier", SECRET_ARN)`.

## Steps to Reproduce

1. CreateEnvironment (any installed VCF version) and wait past the transition so the environment is CREATED.
2. CreateEnvironmentConnector with `type` VCENTER and `applianceFqdn` "vcenter.evs.example.com"; wait past the transition so the connector reports `state` ACTIVE, `status` PASSED and one VCENTER_REACHABILITY check.
3. Call UpdateEnvironmentConnector with the same environmentId/connectorId and `applianceFqdn` "VCENTER.EVS.EXAMPLE.COM" — the same DNS name the connector already targets (the crate treats that spelling as the same vCenter at src/state.rs:1584 and in tests/all/connector_deletion.rs:597).
4. Observe 200 with `state` UPDATING, `status` UNKNOWN, `checks` absent and `modifiedAt` moved, and GetEnvironment reporting a moved environment `modifiedAt` — although the handler's comment at src/ops/update_environment_connector.rs:284-286 states that an update to a value the connector already carries 'neither restarts the connector nor moves a stamp'.
5. Repeat the ORIGINAL CreateEnvironmentConnector with `applianceFqdn` "VCENTER.EVS.EXAMPLE.COM" instead: the @idempotent repeat is refused with 400 ValidationException naming the type conflict, instead of reporting the stored connector for that same appliance.
