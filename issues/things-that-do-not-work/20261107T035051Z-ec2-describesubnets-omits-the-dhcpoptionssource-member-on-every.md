Issue-Source: trace-sweep-divergence
Issue-Trace: autoscaling-asgssh-traj-vpczoneidentifier-comma-string-to-private-subnets
Issue-Step: task#13
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: autoscaling-asgssh-traj-vpczoneidentifier-comma-string-to-private-subnets
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-00000000000000033"].dhcpOptionsSource
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-00000000000000035"].dhcpOptionsSource
Issue-Commit: 69fbb491696def14b5ee7dd45913958907d046a9

# EC2 DescribeSubnets omits the dhcpOptionsSource member on every subnet

> _Note: this issue was identified against the code base at commit `69fbb491696def14b5ee7dd45913958907d046a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSubnets

## Description
ONE net-new cause on the EC2 (ec2Query) subnet read surface. A fixer must close this one cause before submitting.

Cause 1 of 1 - DescribeSubnets does not project dhcpOptionsSource.

AWS: a DescribeSubnets response is HTTP 200 whose DescribeSubnetsResponse.subnetSet contains one `item` per matched subnet, and EVERY item carries a `dhcpOptionsSource` element stating where that subnet's DHCP options come from. For a subnet that has no subnet-level DHCP option set of its own and therefore inherits the VPC's, the value is the fixed token `vpc`. This is recorded behavior only: the on-disk Smithy model's shape `Subnet` (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) declares subnetId, vpcId, state, cidrBlock, availabilityZone/availabilityZoneId, availableIpAddressCount, defaultForAz, mapPublicIpOnLaunch, tagSet, subnetArn, ownerId and friends, but does NOT declare dhcpOptionsSource at all - the model is behind the wire for this member, so the recording is the authority. DescribeSubnets models no client errors here; the call succeeds in both AWS and the emulator, so the divergence is purely the missing member, not a status or error-code difference.

Emulator: the element is absent from every described subnet - the comparison reports `dhcpOptionsSource: missing (expected 'vpc', got None)` for each item in subnetSet, while the surrounding 200 response and all other subnet members match.

Why: a client that reads a subnet's DHCP-options source to decide whether it must create/associate a subnet-level DHCP option set, or to report whether a subnet's resolver configuration is inherited or overridden, reads null. It cannot distinguish 'inherits the VPC's options' from 'has its own', so it either re-applies options that are already in effect or reports the subnet as unconfigured. Any SDK that deserializes the member into a non-optional enum/string also loses it silently.

Fix: the DescribeSubnets serializer's Subnet projection must emit the `dhcpOptionsSource` element for every subnet item, backed by the subnet's DHCP-options association in EC2 state, defaulting to the VPC-inherited value (`vpc`) for a subnet that has no subnet-level association. The member is not in the on-disk model, so the projection (and the state field it reads) must carry it explicitly.

Related open issue: 20261102T114505Z-ec2-launch-template-read-validation-and-subnet-projection.md describes the same omission as its Cause 4, but its recorded replay keys name the subnet ids of an earlier run (subnet-0000000000000002b / ...2d) rather than the ids this replay produced (subnet-00000000000000033 / ...035), so that issue's pointer does not bind to this divergence and the replay reports it unowned. Fixing dhcpOptionsSource once closes both; this issue carries the binding that the current corpus actually exercises.

Traces: autoscaling-asgssh-traj-vpczoneidentifier-comma-string-to-private-subnets#task#13.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator booted from this checkout (emulator_up=true, /__admin/health answered 200). Divergence cluster verbatim:

  service: ec2
  operation: DescribeSubnets
  verdict: mismatch
  expected_status: 200, actual_status: 200
  fixture_id: autoscaling-asgssh-traj-vpczoneidentifier-comma-string-to-private-subnets
  step_key: task#13
  owned_by: [] (no open issue carries this replay key)
  stub_path: "" (the operation is implemented; it answers 200)
  detail:
    $.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-00000000000000033"].dhcpOptionsSource: missing (expected 'vpc', got None)
    $.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-00000000000000035"].dhcpOptionsSource: missing (expected 'vpc', got None)

Requirement vs per-run values: the FIXED part of the requirement is that each subnetSet item carries a dhcpOptionsSource element, and that its value is the token 'vpc' when the subnet inherits the VPC's DHCP option set. The subnet ids (subnet-00000000000000033, subnet-00000000000000035) and the response requestId are per-run values from this environment and are NOT part of the requirement - the member must appear for whatever subnets the request matches.

Smithy grounding: shape com.amazonaws.ec2#Subnet in services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json has no dhcpOptionsSource member (verified members: AvailabilityZone, AvailabilityZoneId, AvailableIpAddressCount, AssignIpv6AddressOnCreation, BlockPublicAccessStates, CidrBlock, CustomerOwnedIpv4Pool, DefaultForAz, EnableDns64, EnableLniAtDeviceIndex, Ipv6CidrBlockAssociationSet, Ipv6Native, MapCustomerOwnedIpOnLaunch, MapPublicIpOnLaunch, OutpostArn, OwnerId, PrivateDnsNameOptionsOnLaunch, State, SubnetArn, SubnetId, Tags, Type, VpcId). Hence 'recorded behavior only' for this member.

## Steps to Reproduce
1. Clone the repo and boot the emulator from this checkout.
2. Replay the wire fixture autoscaling-asgssh-traj-vpczoneidentifier-comma-string-to-private-subnets from simparity-wire-aws-v1 up to and including step task#13.
3. task#13 issues the ec2Query request Action=DescribeSubnets for the two private subnets the fixture's VPC setup created.
4. Inspect the 200 response body: each DescribeSubnetsResponse.subnetSet.item is returned, but no item contains a dhcpOptionsSource element.
5. Expected: every subnetSet item contains <dhcpOptionsSource>vpc</dhcpOptionsSource>, because these subnets have no subnet-level DHCP option set and inherit the VPC's.
6. After the fix, re-replay autoscaling-asgssh-traj-vpczoneidentifier-comma-string-to-private-subnets#task#13 and confirm the member is present for every described subnet.
