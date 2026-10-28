Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3d1fad8f4871dd362a3fc79086918715269856ce

# connectivityInfo route server peer IDs are stored and echoed without being resolved against the account's EC2 store

_Filed: 2026-10-27T23:38:09Z_
_Operation: CreateEnvironment | Class: validation_

> _Note: this issue was identified against the code base at commit `3d1fad8f4871dd362a3fc79086918715269856ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateEnvironment` resolves every member of its request that names an EXISTING Amazon EC2 resource of the caller's account against Amazon EC2's own store before anything is minted, and the handler's module doc enumerates them: `vpcId` and `serviceAccessSubnetId` (rule 7), each `hosts[i].placementGroupId` / `dedicatedHostId` (rule 9), and each `serviceAccessSecurityGroups.securityGroups` entry (rule 13). The same doc names the ONE such member it deliberately does not resolve — `initialVlans.hcxNetworkAclId` — and gives the reason: the account's EC2 store holds no network ACL records at all, so the question is unanswerable.

`connectivityInfo.privateRouteServerPeerings` is a sixth member of this request that names existing EC2 resources ("The unique IDs for private route server peers"; "Amazon EVS requires that you specify two route server peer IDs. During environment creation, the route server endpoints peer with the NSX uplink VLAN for connectivity to the NSX overlay network"), and the reason given for the hcxNetworkAclId exception does not hold for it: the EC2 crate this one already depends on (and already reads for VPCs, subnets, security groups, placement groups, Dedicated Hosts and Elastic IP allocations) holds route server peers in a public map keyed by `RouteServerPeerId`, so the store CAN report whether an id names one.

The handler instead stores the member verbatim and echoes it back in `Environment$connectivityInfo`; nothing else in the crate ever reads it (the only other mentions of `connectivity_info` in `src/` are the stored field and `to_wire`). So a create for an installed VCF version whose `privateRouteServerPeerings` name two peers the account holds no route server peer for is accepted with HTTP 200, and `GetEnvironment` then reports a connectivity configuration pointing at peers that do not exist — while the SAME request naming a non-existent `placementGroupId` or security group is refused with a `ValidationException` clause naming the id it gave. The refusal belongs on the installed-VCF path, the scope this handler already judges the other install-only rules at (rules 10 and 12 are "Judged only where Amazon EVS INSTALLS VCF"), because that is the mode in which the peering described above happens at all (`connectivityInfo` is "Not supported when vcfVersion is SELF_DEPLOYED").

## Evidence

Spec, `ConnectivityInfo` (smithy_query query="shape" shape="ConnectivityInfo"): member `privateRouteServerPeerings` — required, `@length(min:2,max:2)` — doc "<p>The unique IDs for private route server peers.</p>"; shape doc "The connectivity configuration for the environment. Amazon EVS requires that you specify two route server peer IDs. During environment creation, the route server endpoints peer with the NSX uplink VLAN for connectivity to the NSX overlay network." plus "Not supported when <code>vcfVersion</code> is <code>SELF_DEPLOYED</code>."

The handler's own enumeration of the references it DOES resolve — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/create_environment.rs:51-55 and :61-64:
```
//! 7. The `vpcId` names a VPC the account holds, and the `serviceAccessSubnetId` a
//!    subnet of THAT VPC: an environment is deployed into the caller's existing
//!    network, so a request naming a VPC or subnet that does not exist is refused with
//!    a clause naming the id it gave ([`crate::state::ec2_resources`], which reads
//!    Amazon EC2's own store).
...
//! 9. A `placementGroupId` or `dedicatedHostId` an entry of `hosts` names resolves in
//!    that same EC2 store: a host is PLACED on those resources and Amazon EVS creates
//!    neither, so they are the caller's own, judged exactly as rule 7 judges the
//!    network ([`crate::state::ec2_resources::host_placement_failures`]).
```
and the ONE exception it states, with its reason — create_environment.rs:93-97:
```
//! `initialVlans.hcxNetworkAclId` is NOT among them: a network access control list is an
//! EC2 resource too, but the account's EC2 store holds no network ACL records at all, so
//! that store can report neither that the id names one nor that it does not — and an
//! unanswerable question refuses nothing, exactly as an unreadable store does
```
The store CAN answer it for route server peers — services/service-com-amazonaws-ec2-amazonec2/src/state.rs:3279-3281:
```
    /// Every route server peer the account holds, by `RouteServerPeerId`, as the modelled
    /// [`RouteServerPeer`] shape itself, keyed and ordered like [`Self::route_servers`].
    pub route_server_peers: BTreeMap<String, RouteServerPeer>,
```
read exactly the way this crate already reads that store's other maps — services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/state/ec2_resources.rs:291-306 (`store.placement_groups.contains_key(..)`, `store.dedicated_hosts.contains_key(..)`) and :136-142 (`store.vpcs.contains_key(..)`, `store.subnets.get(..)`).

All the handler does with the member — create_environment.rs:746:
```
            connectivity_info: input.connectivity_info,
```
and the only other uses in the crate, src/state.rs:1273 (`pub connectivity_info: Option<ConnectivityInfo>,`) and src/state.rs:1866 (`connectivity_info: self.connectivity_info.clone(),`): stored and echoed, never resolved.

Test coverage: the only rows that send the member use placeholder ids `["rsp-1", "rsp-2"]` — tests/all/environment_lifecycle.rs:669 with the accepted row at :714-724 ("an installed version accepts those parameters too", expected 200) and tests/all/create_environment_validation.rs:155 with the SELF_DEPLOYED row at :194-207 ("Only the terms: carrying those five parameters is no failure at all"). Their stated reason (environment_lifecycle.rs:681-683) is the MODE rule — "The five parameters `SELF_DEPLOYED` 'does not support' are ACCEPTED: the mode does nothing with them" — which is about whether carrying the parameter is a failure, not about whether its value resolves; the SELF_DEPLOYED row stays green under an installs_vcf-scoped check, and the installed row needs its fixture to register the two peers in the EC2 store, the way the suite already registers the VPC and the service access subnet.

## Steps to Reproduce

1. Register a VPC and a service access subnet in the EC2 store (as tests/all/create_environment_validation.rs does), and register NO route server peer.
2. Call CreateEnvironment with vcfVersion=VCF-5.2.2, termsAccepted=true, the ten initialVlans, four hosts, licenseInfo, vcfHostnames and connectivityInfo={"privateRouteServerPeerings":["rsp-1","rsp-2"]} — two ids the account's EC2 store holds no route server peer for.
3. Observe HTTP 200: the environment is minted and GetEnvironment reports environment.connectivityInfo.privateRouteServerPeerings = ["rsp-1","rsp-2"], peers that do not exist in the account.
4. For contrast, repeat the same request with hosts[0].placementGroupId="pg-0123456789abcdef0" (a placement group the EC2 store does not hold): it is refused with 400 ValidationException whose fieldList names hosts[0].placementGroupId — the rule rule 9 of the handler doc states for the same class of reference.
