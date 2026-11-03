Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-sshp-traj-eip-association-needs-an-attached-igw
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-sshp-traj-eip-association-needs-an-attached-igw
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeAddresses|2xx->2xx|$.DescribeAddressesResponse.addressesSet
Issue-Related: 20261030T000411Z-ec2-mutations-are-not-observable-reads-after-modify.md
Issue-Related: 20261103T123451Z-ec2-describe-projections-omit-modeled-response-members.md
Issue-Commit: 10d8f420d9792c6a92213c81068536eef787b058

# EC2 Elastic IP allocations are invisible to DescribeAddresses after association

> _Note: this issue was identified against the code base at commit `10d8f420d9792c6a92213c81068536eef787b058`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAddresses

## Description
Two net-new causes: an allocated-and-associated Elastic IP is not represented in the address store, so the read paths that depend on it return nothing.

Cause 1 of 2 — DescribeAddresses returns an empty addressesSet after AllocateAddress + AssociateAddress succeeded.
AWS: the filtered read returns the address item with publicIp, allocationId, domain 'vpc', associationId, networkInterfaceId, networkInterfaceOwnerId, privateIpAddress, publicIpv4Pool 'amazon', networkBorderGroup set to the region, and the tagSet applied at allocation.
Emulator: addressesSet is serialized as an empty element, so the EIP the fixture just allocated and associated does not exist for the read path.
Why: a client cannot discover, re-associate or release the EIP it owns; address-leak cleanup silently finds nothing to clean.
Fix: the DescribeAddresses lookup/filtering plus the address record written by AllocateAddress/AssociateAddress in state.
Traces: ec2-sshp-traj-eip-association-needs-an-attached-igw#task#10.

Cause 2 of 2 — the same missing association is visible from the ENI side.
AWS: the associated ENI renders an association block {publicIp, publicDnsName, ipOwnerId, allocationId, associationId, natEnabled 'true'} at the interface level and on the matching privateIpAddressesSet entry, plus an empty publicDnsName at the interface level.
Emulator: all of them are absent.
Why: neither the address view nor the interface view shows the public address, so no client can determine the instance's public connectivity.
Fix: the address-to-ENI link in state, consumed by both the DescribeAddresses and DescribeNetworkInterfaces projections.
Traces: ec2-sshp-traj-eip-association-needs-an-attached-igw#task#9.

## Evidence
Cause 1: $.DescribeAddressesResponse.addressesSet type_mismatch — expected {'item': {'publicIp': ..., 'allocationId': 'eipalloc-...', 'domain': 'vpc', 'associationId': ..., 'networkInterfaceId': 'eni-...', 'networkInterfaceOwnerId': '<acct>', 'privateIpAddress': ..., 'tagSet': {'item': {'key': 'Name', ...}}, 'publicIpv4Pool': 'amazon', 'networkBorderGroup': '<region>'}}, got ''.
Cause 2: $.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.association missing, .privateIpAddressesSet.item.association missing (both expected the {publicIp, publicDnsName, ipOwnerId, allocationId, associationId, natEnabled 'true'} block), .publicDnsName missing (expected '').

## Steps to Reproduce
Replay the wire corpus (simparity-wire-aws-v1) and inspect ec2-sshp-traj-eip-association-needs-an-attached-igw#task#9 and #task#10, which allocate an Elastic IP, attach an internet gateway, associate the EIP with the ENI and then read both DescribeNetworkInterfaces and DescribeAddresses.
