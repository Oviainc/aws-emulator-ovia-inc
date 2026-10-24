Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# VLAN subnet CIDR blocks are never checked against the VPC they are provisioned into

_Filed: 2026-10-24T01:22:38Z_
_Operation: CreateEnvironment | Class: validation_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The ten `initialVlans` CIDR blocks become real subnets inside the VPC the request names, so the documented rule is scoped to the VPC: "Amazon EVS VLAN subnet CIDR blocks must not overlap with other subnets in the VPC" (`InitialVlans` / `InitialVlanInfo$cidr` doc). A VLAN block must therefore (a) lie inside the named VPC's own CIDR block — a subnet cannot be created outside its VPC's range — and (b) not overlap any subnet the VPC already holds, the `serviceAccessSubnetId` of the very same request included.

Expected: a CreateEnvironment whose `initialVlans` carry a CIDR outside the VPC's CIDR block, or one that overlaps an existing subnet of that VPC (e.g. the service access subnet's own block), is refused with ValidationException whose `fieldList` names the offending `initialVlans.<member>`, and nothing is created.

Actual: `vlan_failures` only compares the ten requested blocks against EACH OTHER (plus the /24../28 size rule and the `hcxNetworkAclId` rule). `crate::state::ec2_resources::network_failures` reads EC2 only to confirm the VPC exists and that the service access subnet belongs to it — it never reads `vpc.cidr_block` or `subnet.cidr_block` — so a request whose ten VLANs sit in 10.0.0.0/24 is accepted against a VPC of 172.31.0.0/16, and a VLAN that exactly overlaps the service access subnet is accepted too. Two further members of the same request are accepted unchecked for the same reason: `serviceAccessSecurityGroups.securityGroups` (ids the account may hold no security group for, or ones belonging to another VPC) and `initialVlans.hcxNetworkAclId` (an `acl-…` id no network ACL exists for), although the EC2 store that would answer all of these is already reachable from this crate.

## Evidence

Spec: `InitialVlans` doc ("Amazon EVS VLAN subnet CIDR blocks must not overlap with other subnets in the VPC"), `InitialVlanInfo$cidr` doc (same sentence), `CreateEnvironmentRequest$vpcId` / `serviceAccessSubnetId` docs (the environment is deployed into the caller's existing network), and the VPC rule that a subnet's CIDR lies inside its VPC's CIDR. Handler: src/ops/create_environment.rs `vlan_failures` lines 583-632 — the only overlap test is `networks.iter().find(|(_, other)| other.overlaps(&network))` over the ten PLANNED VLANs; src/state/ec2_resources.rs `network_failures` lines 68-89 — existence checks only (`store.vpcs.contains_key`, `subnet.vpc_id == vpc_id`), no CIDR is read, even though EC2's store carries `Subnet.cidr_block` / VPC CIDR (services/service-com-amazonaws-ec2-amazonec2/src/state.rs:2561). Handler module doc lines 9-26 enumerates the enforced preconditions and the VPC-scoped CIDR rule is not among them. Tests: tests/all/create_environment_validation.rs covers mutual VLAN overlap, the /24../28 bound and unknown vpc/subnet ids (UNKNOWN_VPC / UNKNOWN_SUBNET rows around line 441); no test registers a VPC CIDR or a second subnet CIDR, so nothing asserts the VPC-scoped rule.

## Steps to Reproduce

1. Register a VPC with CIDR 172.31.0.0/16 and a service access subnet 172.31.0.0/20 in the shared EC2 store.
2. Call CreateEnvironment against that vpcId/serviceAccessSubnetId with termsAccepted=true and ten initialVlans CIDRs taken from 10.0.0.0/24 (entirely outside the VPC's CIDR block).
3. Observe HTTP 200 and an environment whose ten VLAN subnets sit outside the VPC, instead of a ValidationException naming the initialVlans members.
4. Repeat with one VLAN set to 172.31.0.0/24 (overlapping the service access subnet of the same VPC) — it is also accepted.
