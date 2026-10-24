Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# AssociateEipToVlan accepts an allocationId the account holds no Elastic IP for

_Filed: 2026-10-24T00:04:35Z_
_Operation: AssociateEipToVlan | Class: validation_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`allocationId` is 'The allocation ID of the Elastic IP address' the caller already holds in Amazon EC2 (pattern `^eipalloc-[a-zA-Z0-9_-]+$`), and the operation models ResourceNotFoundException and ValidationException. Expected: an allocation id that names no Elastic IP allocation of the account is refused (not-found / validation) and no association is recorded. Actual: the handler asks EC2 only for the public address behind the allocation (`ec2_resources::elastic_ip_address`) and treats an absent allocation exactly like an unreadable store: it mints an `eipassoc-…` association anyway and reports the VLAN with an EipAssociation whose `ipAddress` is absent. The association is persisted, reported by ListEnvironmentVlans, and can be disassociated — i.e. the emulator attaches an Elastic IP that does not exist, and the only observable difference from a real association is a missing `ipAddress` member.

## Evidence

Spec: AssociateEipToVlan models ResourceNotFoundException (404) / ValidationException (400); `allocationId` names an existing EC2 Elastic IP allocation. Handler: src/ops/associate_eip_to_vlan.rs lines 199-202 and 290-320 — the EC2 lookup is used only to fill `ip_address`, and `src/state/ec2_resources.rs:96` documents that `None` covers both 'the account holds no such allocation' and 'the store cannot be read', with callers treating None as 'no failure to report'. The same module DOES refuse an unknown vpcId/subnetId in CreateEnvironment, so the EC2 store is consultable for existence. Tests: tests/all/hcx_eip_associations.rs exercises the HCX/public-VLAN rules and the idempotent repeat, but no test asserts a refusal for an unallocated eipalloc id.

## Steps to Reproduce

1. CreateEnvironment with initialVlans.isHcxPublic=true and an hcxNetworkAclId; wait for CREATED.
2. Without allocating any Elastic IP in EC2, call AssociateEipToVlan(environmentId=<id>, vlanName="hcx", allocationId="eipalloc-0123456789abcdef0").
3. Observe HTTP 200 with a newly minted eipassoc association (and no ipAddress) instead of a not-found/validation refusal.
4. ListEnvironmentVlans shows the phantom association on the hcx VLAN.
