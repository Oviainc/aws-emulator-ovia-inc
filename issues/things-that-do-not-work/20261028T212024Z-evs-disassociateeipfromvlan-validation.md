Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: e94a5165da5e1a7dc4e65daf2da688ae995eb229

# DisassociateEipFromVlan answers 200 for an associationId the addressed VLAN never held — including one another environment's VLAN still holds

_Filed: 2026-10-28T21:20:24Z_
_Operation: DisassociateEipFromVlan | Class: validation_

> _Note: this issue was identified against the code base at commit `e94a5165da5e1a7dc4e65daf2da688ae995eb229`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DisassociateEipFromVlan resolves the environment and its public HCX VLAN, then removes associations by matching `associationId` against the list that VLAN carries. An id that matches NOTHING is not refused: `retain` keeps every entry, `removed` is false, nothing is re-stamped, nothing is released on EC2 — and the call answers HTTP 200 with the VLAN. So a caller that passes a typo'd id, an id that was never minted, or the `eipassoc-` id of an association ANOTHER environment's HCX VLAN is still holding, is told the disassociation succeeded while both this service's store and the account's EC2 `Address` records are untouched.

Three places in this same crate draw the opposite line for a handle that resolves to no record OF THE ADDRESSED PARENT:

* `DeleteEnvironmentHost` — an `@idempotent` delete addressed by a handle — answers `ResourceNotFoundException` for a host record the environment never held, and says so explicitly: a record that has already reached DELETED is what the idempotent repeat reports, while one the environment never held is the missing resource.
* `DeleteEnvironmentConnector` does the same for a `connectorId`, and `crate::state::live_connector` states that the id is scoped to the environment, so a connector id ANOTHER environment holds resolves to nothing (this crate's own `delete_entitlement` suite enumerates exactly that row).
* The paired writer on this very resource, `AssociateEipToVlan`, REFUSES an `allocationId` that another environment's HCX VLAN is using, with a ValidationException naming the holder and its association id.

So the associate half refuses a cross-environment Elastic IP handle and the disassociate half silently succeeds on one. The handler's stated justification covers only the repeat case ("a repeat of a call that already removed it is not a failure"); it says nothing about an id that never named an association of this VLAN, which is the case the sibling deletes single out as the missing resource. The operation models `ResourceNotFoundException` ("A service resource associated with the request could not be found"), and nothing else in the response distinguishes the two outcomes, because the answered `vlan` looks identical either way.

## Evidence

The unconditional success — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/disassociate_eip_from_vlan.rs:257-291:
```
        // `@idempotent`: the end state is what the call asks for — the VLAN no longer
        // carries this association — so a repeat of a call that already removed it is
        // not a failure. Only a call that really removed something re-stamps the record.
...
        let before = vlan.eip_associations.len();
        let mut released: Vec<(String, String)> = Vec::new();
        vlan.eip_associations.retain(|association| {
            let named =
                association.association_id.as_deref() == Some(input.association_id.as_str());
...
        let removed = vlan.eip_associations.len() != before;
        if removed {
            vlan.modified_at_ms = modified_at_ms;
        }
        let vlan = vlan.to_wire();
...
        Ok(DisassociateEipFromVlanOutput { vlan: Some(vlan) })
```
(there is no branch on `removed` other than the stamp and the EC2 release, and no error path for an unmatched id).

The sibling rule for a handle the addressed parent never held — src/ops/delete_environment_host.rs:241-248:
```
        // A host record this environment never held is the missing resource, whatever
        // state the records it does hold are in: a record that has already reached
        // DELETED is what the idempotent repeat below reports.
        let host = environment.hosts.get_mut(&input.host_name).ok_or_else(|| {
            DeleteEnvironmentHostErrorModeled::ResourceNotFoundException(
                crate::state::host_not_found(&input.host_name),
            )
        })?;
```
and src/ops/delete_environment_connector.rs:249-259:
```
        // A connector record this environment never held is the missing resource,
        // whatever state the records it does hold are in: a record that has already
        // reached DELETED is what the idempotent repeat below reports.
        let connector = environment
            .connectors
            .get_mut(&input.connector_id)
            .ok_or_else(|| {
                DeleteEnvironmentConnectorErrorModeled::ResourceNotFoundException(
                    crate::state::connector_not_found(&input.connector_id),
                )
            })?;
```
The crate's statement that such a handle is environment-scoped — src/state.rs:2837-2842:
```
/// The live connector `connector_id` names in this environment, if any: the record a
/// request that wants to USE a connector resolves to.
///
/// The id is scoped to the environment, exactly as `UpdateEnvironmentConnector` reads
/// it, and a connector the service has started deleting resolves to nothing
```
and the row this crate's own suite asserts for it — tests/all/delete_entitlement.rs:556-561: "the rows differ along which handle fails to resolve — a well-formed environment id no environment holds ..., a never-minted connector id, and a connector id another environment holds."

The paired writer refusing the cross-environment Elastic IP handle — src/ops/associate_eip_to_vlan.rs:320-340:
```
        // One Elastic IP address carries ONE association at a time: the `allocationId` is
        // an allocation of the caller's EC2 account, so associating it with this VLAN
        // CONSUMES it, and an allocation some other VLAN is already using cannot be
        // associated again
...
            if let Some((holder, held_by)) = state.eip_allocation_holder(&input.allocation_id) {
                return Err(AssociateEipToVlanErrorModeled::ValidationException(
                    crate::state::field_validation_error(
                        "allocationId",
                        &format!(
                            "names an Elastic IP address that is already associated with the \
                             {} VLAN of environment {holder} through association {held_by}; \
                             disassociate it first",
```

Test coverage: no test exercises an `associationId` that names a live association of another environment. The only place an unmatched id reaches this handler is incidental — tests/all/hcx_eip_associations.rs:365-366 and :420-439 run the row ("the public hcx VLAN", public_hcx, "hcx", "") through BOTH operations, passing `"associationId": json!("eipassoc-0123456789abcdef0")` (an id nothing minted) and asserting HTTP 200; that row's stated subject is the `vlanName` rule ("the error code the rule answers with — `\"\"` for a row that is accepted"), not the association handle, so nothing states a reason for accepting an id the VLAN never held.

## Steps to Reproduce

1. Create environment A with `initialVlans.isHcxPublic` true (plus `hcxNetworkAclId`) and let it settle to CREATED; register an Elastic IP allocation in the account's EC2 store.
2. AssociateEipToVlan(environmentId=A, vlanName="hcx", allocationId=<that allocation>) → 200; note the `eipassoc-…` id reported in `vlan.eipAssociations[0].associationId`.
3. Create a second environment B the same way (also public HCX) and let it settle.
4. Call DisassociateEipFromVlan(environmentId=B, vlanName="hcx", associationId=<A's eipassoc- id>).
5. Observe HTTP 200 with B's `vlan` — no error — while ListEnvironmentVlans on A still reports the association and EC2's DescribeAddresses still reports the allocation associated. The same request shape against environment A with a never-minted id (`eipassoc-0123456789abcdef0`) is also answered 200.
6. Compare the paired writer: AssociateEipToVlan(environmentId=B, vlanName="hcx", allocationId=<the allocation A holds>) is refused 400 ValidationException naming A as the holder (src/ops/associate_eip_to_vlan.rs:327-340), and DeleteEnvironmentConnector/DeleteEnvironmentHost answer ResourceNotFoundException for a handle the addressed environment never held.
