Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-pst-traj-subnet-id-is-region-scoped
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-pst-traj-subnet-id-is-region-scoped ec2-usg-traj-group-id-is-region-scoped ec2-usg-traj-groupname-lookup-resolves-only-in-default-vpc ec2-pip-traj-eni-filter-matches-secondary-address ec2-pip-traj-same-private-ip-in-two-vpcs ec2-sshp-traj-eip-association-needs-an-attached-igw
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeSubnets|4xx->2xx|$.Response
Issue-Key: ec2|DescribeSubnets|4xx->2xx|status
Issue-Key: ec2|DescribeSecurityGroups|4xx->2xx|$.Response
Issue-Key: ec2|DescribeSecurityGroups|4xx->2xx|status
Issue-Key: ec2|DescribeSecurityGroups|2xx->2xx|$.DescribeSecurityGroupsResponse.securityGroupInfo
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item
Issue-Commit: 10d8f420d9792c6a92213c81068536eef787b058

# EC2 reads resolve resources outside the request's region and scope

> _Note: this issue was identified against the code base at commit `10d8f420d9792c6a92213c81068536eef787b058`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSubnets

## Description
Five net-new causes sharing one fault: the resource store is not partitioned by region and the describe filters are matched too loosely, so a read answers from resources the request's region/filter cannot legally see.

Cause 1 of 5 — DescribeSubnets resolves a subnet id created in another region.
AWS: a subnet id is region-scoped. Describing it against a different regional endpoint refuses 400 with Code 'InvalidSubnetID.NotFound' and Message "The subnet ID '<subnet-id>' does not exist" — fixed text with the id from the request in single quotes.
Emulator: answers 200 and returns the subnet created in the other region.
Why: a client's region isolation assumption breaks; code that probes an id to decide which region owns a resource gets the wrong answer everywhere.
Fix: the DescribeSubnets lookup must be scoped by the request's region in state.
Traces: ec2-pst-traj-subnet-id-is-region-scoped#task#4.

Cause 2 of 5 — DescribeSecurityGroups resolves a group id created in another region.
AWS: refuses 400 with Code 'InvalidGroup.NotFound' and Message "The security group '<sg-id>' does not exist" — fixed text with the id from the request.
Emulator: answers 200.
Why: same region-isolation break on the security-group read path.
Fix: the DescribeSecurityGroups lookup must be region-scoped.
Traces: ec2-usg-traj-group-id-is-region-scoped#task#3 (cluster also covers ec2-usg-traj-groupname-lookup-resolves-only-in-default-vpc#task#4).

Cause 3 of 5 — DescribeSecurityGroups in a second region lists the first region's default group.
AWS: the call against a region with no matching group returns an EMPTY securityGroupInfo.
Emulator: returns the default group of the other region, and even renders its ARN with the queried region's name while the group belongs to the other one.
Why: a client enumerating per-region default groups double-counts one group and derives an ARN that resolves to nothing.
Fix: the DescribeSecurityGroups listing/region scoping and the ARN composition in its projection.
Traces: ec2-usg-traj-group-id-is-region-scoped#task#4.

Cause 4 of 5 — a DescribeNetworkInterfaces address filter matches interfaces it should not.
AWS: the recorded filtered call returns an EMPTY networkInterfaceSet (no ENI satisfies the filter as AWS evaluates it).
Emulator: returns the ENI, matching the filter against the whole private-address set instead of the member AWS filters on.
Why: the filter is the client's only selection mechanism; a false positive makes it act on the wrong interface.
Fix: the DescribeNetworkInterfaces filter evaluation.
Traces: ec2-pip-traj-eni-filter-matches-secondary-address#task#5 (cluster also covers ec2-pip-traj-same-private-ip-in-two-vpcs#task#8 and ec2-sshp-traj-eip-association-needs-an-attached-igw#task#11).

Cause 5 of 5 — DescribeNetworkInterfaces returns one ENI where two different VPCs each hold the same private IPv4 address.
AWS: the same private address is legal in two VPCs, so the filtered read returns BOTH ENIs (one per VPC) as a two-element item list.
Emulator: returns a single ENI.
Why: a client reconciling duplicate private addresses across VPCs sees only one and concludes the address is unique.
Fix: the DescribeNetworkInterfaces lookup/filter must not key private addresses globally.
Traces: ec2-pip-traj-same-private-ip-in-two-vpcs#task#7.

## Evidence
Cause 1: status expected 400, got 200; expected $.Response.Errors.Error = {Code: 'InvalidSubnetID.NotFound', Message: "The subnet ID 'subnet-...' does not exist"}, got no Response.
Cause 2: status expected 400, got 200; expected {Code: 'InvalidGroup.NotFound', Message: "The security group 'sg-...' does not exist"}.
Cause 3: $.DescribeSecurityGroupsResponse.securityGroupInfo type_mismatch (expected '', got the other region's default group, groupName 'default', securityGroupArn rendered with the queried region).
Cause 4: $.DescribeNetworkInterfacesResponse.networkInterfaceSet type_mismatch (expected '', got one full ENI item).
Cause 5: $.DescribeNetworkInterfacesResponse.networkInterfaceSet.item type_mismatch (expected a 2-element list of ENIs in vpc A and vpc B sharing privateIpAddress 10.241.1.10, got a single ENI).

## Steps to Reproduce
Replay the wire corpus (simparity-wire-aws-v1) and inspect ec2-pst-traj-subnet-id-is-region-scoped#task#4, ec2-usg-traj-group-id-is-region-scoped#task#3 and #task#4, ec2-pip-traj-eni-filter-matches-secondary-address#task#5, ec2-pip-traj-same-private-ip-in-two-vpcs#task#7. Each fixture creates the resource in one region/VPC and then reads it from the other region or through the recorded filter.
