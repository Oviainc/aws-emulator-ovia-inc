Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-cluster-network-security-setup
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-cluster-network-security-setup redshift-cluster-subnet-group-invalid-subnet-error redshift-subnet-group-lifecycle redshift-subnet-group-and-security-group-validation
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribeClusterSubnetGroups|2xx->4xx|status
Issue-Key: redshift|DescribeClusterSubnetGroups|2xx->4xx|$.DescribeClusterSubnetGroupsResponse
Issue-Key: redshift|CreateClusterSubnetGroup|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|CreateClusterSubnetGroup|4xx->2xx|status
Issue-Key: redshift|DescribeClusterSubnetGroups|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|DescribeClusterSubnetGroups|4xx->2xx|status
Issue-Key: redshift|DescribeClusterSubnetGroups|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|CreateClusterSubnetGroup|2xx->2xx|$.CreateClusterSubnetGroupResponse.CreateClusterSubnetGroupResult.ClusterSubnetGroup.Subnets.Subnet[*]
Issue-Key: redshift|CreateClusterSubnetGroup|2xx->2xx|$.CreateClusterSubnetGroupResponse.CreateClusterSubnetGroupResult.ClusterSubnetGroup.VpcId
Issue-Key: redshift|DescribeClusterSubnetGroups|2xx->2xx|$.DescribeClusterSubnetGroupsResponse.DescribeClusterSubnetGroupsResult.ClusterSubnetGroups.ClusterSubnetGroup.Subnets.Subnet[*]
Issue-Key: redshift|DescribeClusterSubnetGroups|2xx->2xx|$.DescribeClusterSubnetGroupsResponse.DescribeClusterSubnetGroupsResult.ClusterSubnetGroups.ClusterSubnetGroup.VpcId
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Redshift cluster-subnet-group operations answer the wrong status, skip subnet validation and synthesize VPC identity

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterSubnetGroups

## Description
7 root causes on the cluster-subnet-group surface: successful describes are refused, refusals are granted, and the subnet group's resolved network identity is invented.

Cause 1 of 7: DescribeClusterSubnetGroups on an existing subnet group is refused with 400. AWS: 200 with the ClusterSubnetGroups collection (Smithy: DescribeClusterSubnetGroups output ClusterSubnetGroupMessage; its only modeled client errors are ClusterSubnetGroupNotFoundFault and InvalidTagFault). Emulator: HTTP 400 and no success body at all. Why: every client that reads back a subnet group it just created fails, so subnet-group-dependent setup cannot proceed. Fix: the DescribeClusterSubnetGroups handler must resolve the group from state and serialize the success response. Traces: redshift-cluster-network-security-setup#setup#1.

Cause 2 of 7: the same DescribeClusterSubnetGroups refusal after a ModifyClusterSubnetGroup, where AWS returns 200 with the updated group (Description, Subnets with SubnetIdentifier/SubnetStatus/SubnetAvailabilityZone, SupportedClusterIpAddressTypes, SubnetGroupStatus 'Complete', Tags). Emulator: 400, response body absent. Fix: as cause 1, on the post-modify read path. Traces: redshift-cluster-network-security-setup#task#5.

Cause 3 of 7: CreateClusterSubnetGroup ACCEPTS a SubnetIds entry that is not a real subnet. Smithy: CreateClusterSubnetGroup declares InvalidSubnet (@error("client"), httpError 400, awsQueryError InvalidSubnet); recorded behavior shows AWS refusing with Error.Type 'Sender', Code 'InvalidParameterValue' and the message "Some input subnets in :[<subnet ids>] are invalid." — fixed text "Some input subnets in :[" + the comma-joined offending subnet ids from the request + "] are invalid." The emulator performs no subnet-existence check and answers 200, creating the group. Why: a client that relies on the service to validate subnets creates a subnet group that can never host a cluster, and the failure surfaces much later. Fix: CreateClusterSubnetGroup validation must resolve each SubnetIds entry before creating the group. Traces: redshift-cluster-subnet-group-invalid-subnet-error#task#1.

Cause 4 of 7: DescribeClusterSubnetGroups for a subnet-group name that does not exist answers 200 instead of refusing. AWS: 400 with Error.Type 'Sender', Code 'ClusterSubnetGroupNotFoundFault', Message "Cluster Subnet group '<ClusterSubnetGroupName>' not found." — fixed text "Cluster Subnet group '" (capital S on Subnet) + the requested name + "' not found." Emulator: 200 with no ErrorResponse. Why: a client probing for existence is told the group exists; a delete-then-verify loop never terminates. Fix: DescribeClusterSubnetGroups must refuse an unresolvable ClusterSubnetGroupName. Traces: redshift-cluster-subnet-group-invalid-subnet-error#task#2.

Cause 5 of 7: when DescribeClusterSubnetGroups DOES refuse an unknown group, the message is emulator-authored. AWS: 400 ClusterSubnetGroupNotFoundFault, "Cluster Subnet group '<name>' not found." Emulator: "Cluster subnet group '<name>' does not exist." (lower-case 'subnet', "does not exist" instead of "not found"). Fix: the DescribeClusterSubnetGroups refusal message. Traces: redshift-subnet-group-and-security-group-validation#task#1.

Cause 6 of 7: CreateClusterSubnetGroup invents the group's VpcId and mis-resolves each subnet's availability zone. AWS resolves the supplied SubnetIds against EC2 and returns (a) VpcId = the VPC that actually contains those subnets, (b) one Subnet entry per supplied SubnetIdentifier whose SubnetAvailabilityZone.Name is THAT subnet's availability zone, carrying only the modeled members SubnetIdentifier, SubnetAvailabilityZone and SubnetStatus (Smithy: Subnet has exactly those three members; AvailabilityZone's SupportedPlatforms is not populated on this path), and (c) the entries in the order AWS returns them. The emulator instead derives VpcId from a hash-like synthetic value unrelated to the subnets' VPC, assigns availability zones that do not match the subnets (several subnets all report the same AZ), adds a SupportedPlatforms sub-structure AWS does not send, and orders the Subnets list differently. Why: a client that takes VpcId from the subnet group to create security groups or endpoints targets a VPC that does not exist, and AZ-aware placement decisions are made on wrong data. Fix: CreateClusterSubnetGroup must resolve subnets through the EC2 state rather than synthesizing identity, and the Subnet serializer must emit only the modeled members. Related open issue 20261028T094513Z-redshift-cluster-networking-identity-is-synthesized-not-resolved.md describes the synthesized VpcId; it does not carry this fixture/step and does not cover the AZ mapping, the extra SupportedPlatforms element or the ordering. Traces: redshift-subnet-group-lifecycle#task#6.

Cause 7 of 7: DescribeClusterSubnetGroups reads back the same invented VpcId, wrong per-subnet availability zones, extra SupportedPlatforms element and divergent Subnets order, so the defect is observable on the read path as well as the create path. Fix: as cause 6, in the subnet-group state/read serializer. Traces: redshift-subnet-group-lifecycle#task#7.

## Evidence
Replay of simparity-wire-aws-v1; all seven clusters verdict=mismatch.

DescribeClusterSubnetGroups (redshift-cluster-network-security-setup#setup#1): status: expected 200, got 400.
DescribeClusterSubnetGroups (redshift-cluster-network-security-setup#task#5): status: expected 200, got 400; $.DescribeClusterSubnetGroupsResponse: missing (expected the full ClusterSubnetGroup body with Subnets, SupportedClusterIpAddressTypes 'ipv4', SubnetGroupStatus 'Complete', got None).
CreateClusterSubnetGroup (redshift-cluster-subnet-group-invalid-subnet-error#task#1): status: expected 400, got 200; $.ErrorResponse: missing — expected Error.Type 'Sender', Code 'InvalidParameterValue', Message 'Some input subnets in :[subnet-00000000000000000] are invalid.'
DescribeClusterSubnetGroups (redshift-cluster-subnet-group-invalid-subnet-error#task#2): status: expected 400, got 200; $.ErrorResponse: missing — expected Code 'ClusterSubnetGroupNotFoundFault', Message "Cluster Subnet group 'sim2real-probe-redshift-subnetgrp-bad-74540915' not found."
DescribeClusterSubnetGroups (redshift-subnet-group-and-security-group-validation#task#1), 400 vs 400: expected "Cluster Subnet group 'sim2real-probe-sng-missing' not found.", got "Cluster subnet group 'sim2real-probe-sng-missing' does not exist."
CreateClusterSubnetGroup (redshift-subnet-group-lifecycle#task#6), 200 vs 200:
  ClusterSubnetGroup.VpcId: expected 'vpc-00000000000000022', got 'vpc-8961393bcdacaea32'
  Subnets.Subnet[0]: expected {'SubnetIdentifier': 'subnet-0000000000000002d', 'SubnetStatus': 'Active', 'SubnetAvailabilityZone': {'Name': 'us-west-2c'}}, got {'SubnetIdentifier': 'subnet-00000000000000029', 'SubnetAvailabilityZone': {'Name': 'us-west-2c', 'SupportedPlatforms': {'SupportedPlatform': {'Name': 'VPC'}}}, 'SubnetStatus': 'Active'}
  Subnets.Subnet[1] and [2] likewise: all three emulator entries report AZ 'us-west-2c'/'us-west-2a' against expected 'us-west-2c'/'us-west-2a'/'us-west-2b' per subnet, each carrying the extra SupportedPlatforms element.
DescribeClusterSubnetGroups (redshift-subnet-group-lifecycle#task#7), 200 vs 200: same VpcId, per-subnet AZ, SupportedPlatforms and ordering divergences on the read-back.

The specific vpc-/subnet- ids are the recording's environment; the requirement is that VpcId and each SubnetAvailabilityZone be RESOLVED from the supplied subnets, that only the modeled Subnet members be serialized, and that the status/error classifications above hold.

## Steps to Reproduce
1. Replay fixture redshift-cluster-network-security-setup; inspect setup#1 and task#5 (DescribeClusterSubnetGroups on an existing / just-modified group) and observe HTTP 400 with no success body.
2. Replay fixture redshift-cluster-subnet-group-invalid-subnet-error; inspect task#1 (CreateClusterSubnetGroup with a nonexistent subnet id -> 200 instead of 400 InvalidParameterValue) and task#2 (DescribeClusterSubnetGroups for a nonexistent group -> 200 instead of 400 ClusterSubnetGroupNotFoundFault).
3. Replay redshift-subnet-group-and-security-group-validation step task#1 and compare the not-found Error.Message.
4. Replay fixture redshift-subnet-group-lifecycle; inspect task#6 and task#7 and compare ClusterSubnetGroup.VpcId, each Subnet's SubnetAvailabilityZone.Name, the presence of SupportedPlatforms and the order of the Subnets list.
