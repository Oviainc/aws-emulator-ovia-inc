Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 743fc0a4792edb6d196f368afdefaae152d33ff0

# Associating an Elastic IP with the HCX VLAN never records the association in the account's EC2 Elastic IP store

_Filed: 2026-10-24T02:21:27Z_
_Operation: AssociateEipToVlan | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `743fc0a4792edb6d196f368afdefaae152d33ff0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AssociateEipToVlan "Associates the specified Elastic IP address with the specified VLAN subnet" — the `allocationId` names an Amazon EC2 Elastic IP allocation the account holds, and the `eipassoc-…` id the call reports is an EC2 association id. In real AWS the association is a change to the EC2 resource: DescribeAddresses afterwards reports the allocation as associated (association id, and the network interface / private address it is bound to), and a second association of the same allocation elsewhere is refused while it is in use. DisassociateEipFromVlan releases it again.

Expected: a successful AssociateEipToVlan leaves the EC2 Address record for that allocation associated (its `associationId` set to the id EVS reports), and DisassociateEipFromVlan clears it — so the allocation is observably consumed by the VLAN and cannot simultaneously be associated with another VLAN or another EC2 resource.

Actual: the handler reads EC2 only to copy the public IP into the response (`ec2_resources::elastic_ip_address`) and writes nothing back. The association lives solely inside this service's `StoredVlan::eip_associations`, so EC2's own Address record stays unassociated, and because `already_associated` is checked only against the VLAN being written, the SAME allocationId can be associated with the HCX VLAN of several different environments at once, each call minting a different `eipassoc-` id for one allocation. This is a distinct gap from the open AssociateEipToVlan/validation issue (which is about an allocation that does not exist at all): here the allocation exists and the cross-service state is simply never updated.

## Evidence

Spec: AssociateEipToVlan / DisassociateEipFromVlan docs and `allocationId` (`^eipalloc-[a-zA-Z0-9_-]+$`, "the allocation ID of the Elastic IP address") / `associationId` (`^eipassoc-[a-zA-Z0-9_-]+$`) — both are EC2 identifiers, and single-use association of an Elastic IP is well-established EC2 behaviour (Resource.AlreadyAssociated). Handler: src/ops/associate_eip_to_vlan.rs lines 199-202 (EC2 read used only for `ip_address`) and lines 290-324 (the association is pushed onto `vlan.eip_associations` and nothing else); src/ops/disassociate_eip_from_vlan.rs lines 254-261 (the entry is retained/removed in this service's store only); src/state/ec2_resources.rs lines 96-106 exposes a read-only accessor and no writer. The EC2 emulator does model the association: services/service-com-amazonaws-ec2-amazonec2/src/state.rs carries `addresses` with an `association_id` member it clears on disassociate (line 11349). Tests: tests/all/hcx_eip_associations.rs covers the HCX/public-VLAN rules and the idempotent repeat on one VLAN; nothing asserts the EC2 side or refuses a second environment reusing the allocation.

## Steps to Reproduce

1. Allocate an Elastic IP in the EC2 emulator so its allocationId resolves to a public address.
2. Create two environments (each with initialVlans.isHcxPublic=true and an hcxNetworkAclId) and wait for both to reach CREATED.
3. Call AssociateEipToVlan(environmentId=<first>, vlanName="hcx", allocationId=<the same eipalloc>) — 200 with a new eipassoc id.
4. Call EC2 DescribeAddresses for that allocation — it is reported as unassociated, with no associationId.
5. Call AssociateEipToVlan again for the SECOND environment with the same allocationId — it is accepted and mints a second association for one Elastic IP instead of being refused as already associated.
6. Call DisassociateEipFromVlan for the first environment and observe that the EC2 Address record is again unchanged.
