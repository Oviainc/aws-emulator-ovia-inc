Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4db25e6bf87f4906c725859fd3714c6300682a7e

# initialVlans.hcxNetworkAclId is the one EC2 reference accepted unresolved, on a premise the EC2 crate contradicts

_Filed: 2026-10-28T15:03:31Z_
_Operation: CreateEnvironment | Class: validation_

> _Note: this issue was identified against the code base at commit `4db25e6bf87f4906c725859fd3714c6300682a7e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateEnvironment resolves every Amazon EC2 resource a request names against the account's own EC2 store and refuses a member that names nothing there: `vpcId` and `serviceAccessSubnetId` (rule 7), each `hosts[].placementGroupId`/`dedicatedHostId` (rule 9), each `serviceAccessSecurityGroups.securityGroups` entry (rule 13) and each `connectivityInfo.privateRouteServerPeerings` entry (rule 14) all go through `crate::state::ec2_resources::*_failures`.

`initialVlans.hcxNetworkAclId` is the single exception. The only thing the handler checks about it is PRESENCE when `isHcxPublic` is true; the value itself is then copied onto the HCX `PlannedVlan`, stored on the VLAN record and reported back as `Vlan$networkAclId`. So a create naming a network ACL the account does not hold answers HTTP 200 and the environment's HCX VLAN reports a `networkAclId` that Amazon EC2's own `DescribeNetworkAcls` cannot report — the exact defect that was filed and fixed for `connectivityInfo.privateRouteServerPeerings` (issue 20261027T233809Z-evs-createenvironment-validation--r4, now fixed).

The handler doc states the reason for the exemption, and that reason is false in this repository: it claims "the account's EC2 store holds no network ACL records at all". The EC2 crate this service already depends on holds `pub network_acls: BTreeMap<String, NetworkAcl>`, `insert_vpc` writes a default ACL for every VPC it stores, `CreateNetworkAcl` inserts caller-created ones and `DescribeNetworkAcls` reads that same map. The store can therefore answer the question the comment calls unanswerable, so the stated justification for skipping rule 7 for this member does not hold — and for every request that can be served (the `vpcId` has to resolve) the account holds at least the VPC's own default ACL.

## Evidence

The exemption and its stated premise — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/create_environment.rs:103-107:
```
//! `initialVlans.hcxNetworkAclId` is NOT among them: a network access control list is an
//! EC2 resource too, but the account's EC2 store holds no network ACL records at all, so
//! that store can report neither that the id names one nor that it does not — and an
//! unanswerable question refuses nothing, exactly as an unreadable store does
//! ([`crate::state::ec2_resources`]).
```

The premise is contradicted by the EC2 crate this service depends on — services/service-com-amazonaws-ec2-amazonec2/src/state.rs:3018-3028:
```
    /// Every network ACL the account holds, by `NetworkAclId`, as the modelled
    /// [`crate::ops::types::NetworkAcl`] shape itself ...
    /// A fresh account's default VPC already has one: every VPC comes with a default
    /// network ACL, so [`Self::insert_vpc`] writes it and [`Self::remove_vpc`] takes it
    /// away. See the `Network ACLs` section at the end of this module.
    pub network_acls: BTreeMap<String, NetworkAcl>,
```
and services/service-com-amazonaws-ec2-amazonec2/src/state.rs:4389-4394:
```
        // The default network ACL the VPC comes with, written HERE rather than by each
        // writer of a VPC ...
        self.insert_default_network_acl(&vpc_id);
```
That store is written by an implemented operation (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_network_acl.rs:268 `let Some(network_acl_id) = store.insert_network_acl(state::NetworkAclSpec {`) and read by another (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_network_acls.rs:298 `for network_acl in store.network_acls.values() {`), so it is neither absent nor unreadable.

The rule the member is exempted from, as this handler's own doc states it — src/ops/create_environment.rs:51-55 and 85-91:
```
//! 7. The `vpcId` names a VPC the account holds, and the `serviceAccessSubnetId` a
//!    subnet of THAT VPC: an environment is deployed into the caller's existing
//!    network, so a request naming a VPC or subnet that does not exist is refused with
//!    a clause naming the id it gave ([`crate::state::ec2_resources`], which reads
//!    Amazon EC2's own store).
...
//! 13. Each `serviceAccessSecurityGroups.securityGroups` entry names a security group
//!    the account holds IN that same VPC ... and Amazon EVS creates none of them
```

The only check the code applies to the member — src/ops/create_environment.rs:1198-1204:
```
    // "Required when isHcxPublic is set to true" (`hcxNetworkAclId` doc).
    if initial.is_hcx_public && initial.hcx_network_acl_id.is_none() {
        failures.push(crate::state::field_failure(
            "initialVlans.hcxNetworkAclId",
            "is required when isHcxPublic is true",
        ));
    }
```
and the value being stored and reported unchecked — src/ops/create_environment.rs:1164-1169 (`network_acl_id: initial.hcx_network_acl_id.as_deref()`), src/ops/create_environment.rs:768 (`network_acl_id: planned.network_acl_id.map(str::to_string)`) and src/state.rs:1253 (`network_acl_id: self.network_acl_id.clone()` in `StoredVlan::to_wire`).

Model text for the member (smithy_query doc, InitialVlans$hcxNetworkAclId): "A unique ID for a network access control list that the HCX VLAN uses. Required when isHcxPublic is set to true."

Test coverage: no test pins the non-resolution with a reason — the suites simply pass a fixed made-up id while testing other rules (tests/all/public_vlan_netmask.rs:43 `const HCX_NETWORK_ACL_ID: &str = "acl-0123456789abcdef0";`, tests/all/environment_lifecycle.rs:707, tests/all/hcx_eip_associations.rs:75, tests/all/eip_ec2_propagation.rs:127, tests/all/vlan_subnet_propagation.rs:267). The crate already has the harness pattern for registering such a reference: tests/all/environment_lifecycle.rs:168 `pub(crate) fn register_route_server_peers(engine: &engine::InMemoryEngine, peer_ids: &[&str])`, added when the route-server-peer gap was fixed.

## Steps to Reproduce

1. Register a VPC and a service access subnet in the account's EC2 store (as tests/all/environment_lifecycle.rs:128 `register_network` does), so the create's network references resolve.
2. Call CreateEnvironment with `initialVlans.isHcxPublic` true, `initialVlans.hcx.cidr` a /28 inside the VPC, and `initialVlans.hcxNetworkAclId` set to `acl-0123456789abcdef0` — an id the account's EC2 network ACL store (services/service-com-amazonaws-ec2-amazonec2/src/state.rs:3028) holds no record for.
3. Observe HTTP 200: the environment is created and ListEnvironmentVlans reports the HCX VLAN with `networkAclId` `acl-0123456789abcdef0`.
4. Call Amazon EC2 `DescribeNetworkAcls` with that id: the account holds no such ACL, so the VLAN names a resource no operation can resolve.
5. Compare with the sibling members: the same create with `serviceAccessSecurityGroups.securityGroups` or `connectivityInfo.privateRouteServerPeerings` naming an id the EC2 store does not hold is refused with a ValidationException clause naming the member (src/state/ec2_resources.rs:197 `security_group_failures`, :357 `route_server_peer_failures`).
