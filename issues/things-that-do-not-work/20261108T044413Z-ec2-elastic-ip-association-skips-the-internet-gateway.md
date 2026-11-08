Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-sshp-traj-eip-association-needs-an-attached-igw
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-sshp-traj-eip-association-needs-an-attached-igw
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AssociateAddress|4xx->2xx|$.Response
Issue-Key: ec2|AssociateAddress|4xx->2xx|status
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.association.natEnabled
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.association.publicDnsName
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.privateIpAddressesSet.item.association.natEnabled
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.privateIpAddressesSet.item.association.publicDnsName
Issue-Related: 20261030T000411Z-ec2-mutations-are-not-observable-reads-after-modify.md
Issue-Commit: 3778ba40189b37afffee3a954bf30487a96c0bc7

# EC2 Elastic IP association skips the internet-gateway precondition and misreports the association

> _Note: this issue was identified against the code base at commit `3778ba40189b37afffee3a954bf30487a96c0bc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateAddress

## Description
Two net-new causes in the Elastic-IP association path, both replayed from the same wire fixture. The emulator lets an association happen that AWS refuses, and then describes the resulting association with members AWS does not publish and without members AWS always publishes.

Cause 1 of 2: AssociateAddress does not refuse an association into a VPC with no internet gateway attached (fixture ec2-sshp-traj-eip-association-needs-an-attached-igw, step task#6).
AWS: Smithy: AssociateAddress takes AllocationId / InstanceId / NetworkInterfaceId / PrivateIpAddress / AllowReassociation / DryRun and returns only AssociationId (ec2Query models no operation errors, errors: []), so the refusal itself is recorded behavior only. The recording shows AWS answering HTTP 400 with the ec2-query error envelope $.Response.Errors.Error.Code = 'Gateway.NotAttached' and $.Response.Errors.Error.Message = "Network <vpc-id> is not attached to any internet gateway". Message template: the fixed text is "Network " + " is not attached to any internet gateway"; the only variable part is the VPC id of the target instance/network interface, read from the emulator's state (the recorded run happens to carry vpc-00000000000000034 — that value is not part of the requirement).
Emulator: answers HTTP 200 with an AssociateAddressResponse carrying an associationId and no $.Response error element at all; the association is created.
Why: the IGW-before-EIP ordering rule is a standard bootstrap precondition. A client relying on AWS sees the association fail until it attaches an internet gateway; against the emulator it succeeds, so the address looks associated but is unreachable and every catch/retry path keyed on Gateway.NotAttached is dead code. Downstream asserts then run against an association AWS would never have created.
Fix: AssociateAddress's validation, ahead of the state mutation in its handler — resolve the target interface/instance's VPC, refuse when no internet gateway is attached to it, and serialize the ec2-query error envelope with code Gateway.NotAttached at status 400.
Traces: ec2-sshp-traj-eip-association-needs-an-attached-igw#task#6.

Cause 2 of 2: DescribeNetworkInterfaces misrepresents the EIP association (same fixture, step task#9; the cluster also covers task#11).
AWS: Smithy: NetworkInterfaceAssociation declares publicDnsName (xmlName publicDnsName) alongside allocationId / associationId / ipOwnerId / publicIp / customerOwnedIp / carrierIp; it declares no natEnabled member, so natEnabled is recorded behavior only. The recording shows AWS emitting publicDnsName as an EMPTY element — both on networkInterfaceSet.item and on each association (interface-level and inside every privateIpAddressesSet item) — and emitting natEnabled = 'true' on each association. An EIP association in a VPC that publishes no public DNS name for the interface carries an empty publicDnsName, not a synthesized hostname.
Emulator: omits networkInterfaceSet.item.publicDnsName entirely; fills association.publicDnsName (and privateIpAddressesSet.item.association.publicDnsName) with a hostname synthesized from the public IP in the form ec2-<dashed-ip>.<region>.compute.amazonaws.com; and omits natEnabled on both associations.
Why: a client that reads publicDnsName gets a name AWS never published for this interface and that resolves nowhere, and any logic gated on natEnabled never sees the flag — the association reads as a plain EIP binding rather than a NAT-enabled one.
Fix: the DescribeNetworkInterfaces response projection/serializer for NetworkInterfaceAssociation (and the interface-level publicDnsName) — emit publicDnsName as an empty element unless the interface actually has a published public DNS name, emit it at the interface level too, and emit natEnabled for an EIP association.
Traces: ec2-sshp-traj-eip-association-needs-an-attached-igw#task#9, ec2-sshp-traj-eip-association-needs-an-attached-igw#task#11.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — fixture ec2-sshp-traj-eip-association-needs-an-attached-igw, step task#6, operation AssociateAddress:
  expected status 400, got 200
  expected $.Response = {'Errors': {'Error': {'Code': 'Gateway.NotAttached', 'Message': 'Network vpc-00000000000000034 is not attached to any internet gateway'}}, ...}; got None (no Response element).

Cause 2 — same fixture, steps task#9 and task#11, operation DescribeNetworkInterfaces (status 200 both sides):
  $.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.publicDnsName: missing (expected '', got None)
  $.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.association.publicDnsName: value_mismatch (expected '', got 'ec2-54-0-0-2.us-west-2.compute.amazonaws.com')
  $.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.association.natEnabled: missing (expected 'true', got None)
  $.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.privateIpAddressesSet.item.association.publicDnsName: value_mismatch (expected '', got 'ec2-54-0-0-2.us-west-2.compute.amazonaws.com')
  $.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.privateIpAddressesSet.item.association.natEnabled: missing (expected 'true', got None)

Model evidence: smithy_query operation=AssociateAddress (output = AssociationId only, errors: []) and shape=NetworkInterfaceAssociation (publicDnsName declared; no natEnabled member) on services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json.

No open issue carries these replay keys (the replay reports owned_by = [] and no stub for both clusters).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire fixture ec2-sshp-traj-eip-association-needs-an-attached-igw.
3. Step task#6: AssociateAddress binds an allocated Elastic IP to an instance/ENI whose VPC has no internet gateway attached. Expect HTTP 400 with Response.Errors.Error.Code 'Gateway.NotAttached' and the message "Network <vpc-id> is not attached to any internet gateway"; observe HTTP 200 with an associationId.
4. Steps task#9 and task#11: DescribeNetworkInterfaces on that interface. Expect publicDnsName present-but-empty at the interface level and on every association, and natEnabled 'true' on each association; observe a synthesized ec2-<dashed-ip>.<region>.compute.amazonaws.com publicDnsName, no interface-level publicDnsName and no natEnabled.
