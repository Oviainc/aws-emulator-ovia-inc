Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-ec2mr-byvpc-vpc-create-round-trips-isdefault-and-cidr-associations
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-ec2mr-byvpc-vpc-create-round-trips-isdefault-and-cidr-associations ec2-madl-traj-vpc-subnet-discovery-filters ec2-ibp-traj-launch-template-from-failed-ami-poll ec2-pip-traj-instance-private-ip-reverse-lookup ec2-pip-traj-eni-filter-matches-secondary-address ec2-sshp-traj-eip-association-needs-an-attached-igw ec2-vfd-describe-filter-name-and-token-mistakes ec2-vfd-log-format-field-name-refusals ec2-vfd-resource-and-flow-log-id-mistakes-are-atomic ec2-vfd-s3-destination-mistakes-then-recovery ec2-vfd-s3-reference-route-and-destination-filters ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeVpcs|2xx->2xx|$.DescribeVpcsResponse.vpcSet.item.ipv6CidrBlockAssociationSet
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-0000000000000002b"].dhcpOptionsId
Issue-Key: ec2|DescribeSubnets|2xx->2xx|$.DescribeSubnetsResponse.subnetSet.item[subnetId="subnet-0000000000000002d"].dhcpOptionsId
Issue-Key: ec2|DescribeLaunchTemplates|2xx->2xx|$.DescribeLaunchTemplatesResponse.launchTemplates.item.createdBy
Issue-Key: ec2|DescribeLaunchTemplates|2xx->2xx|$.DescribeLaunchTemplatesResponse.launchTemplates.item.launchTemplateId
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.networkInterfaceSet.item.privateIpAddressesSet.item
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.privateIpAddressesSet.item[*].privateDnsName
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.association
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.privateIpAddressesSet.item.association
Issue-Key: ec2|DescribeFlowLogs|2xx->2xx|$.DescribeFlowLogsResponse.flowLogSet.item.destinationOptions
Issue-Key: ec2|DescribeFlowLogs|2xx->2xx|$.DescribeFlowLogsResponse.nextToken
Issue-Key: ec2|DescribeAddresses|2xx->2xx|$.DescribeAddressesResponse.addressesSet.item.instanceId
Issue-Related: 20261023T102725Z-ec2-response-bodies-omit-modeled-members-and-report.md
Issue-Related: 20261023T155030Z-ec2-createplacementgroup-and-describeaddresses-drop-output-members.md
Issue-Related: 20261030T000411Z-ec2-mutations-are-not-observable-reads-after-modify.md
Issue-Related: 20261030T124526Z-ec2-describelaunchtemplates-omits-the-modeled-operator-member-and.md
Issue-Related: 20261031T181451Z-ec2-instance-volume-snapshot-and-iam-profile-projections.md
Issue-Related: 20261102T114505Z-ec2-launch-template-read-validation-and-subnet-projection.md
Issue-Related: 20261103T071133Z-ec2-describeinstances-and-launch-template-projections-omit-modeled.md
Issue-Commit: 10d8f420d9792c6a92213c81068536eef787b058

# EC2 Describe projections omit modeled response members

> _Note: this issue was identified against the code base at commit `10d8f420d9792c6a92213c81068536eef787b058`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
Nine net-new causes with one shape: the read path serializes a hand-written subset of each resource projection, so members AWS always renders are absent (and in one case a member AWS omits is rendered as an empty element). Every cause is a 200/200 body mismatch, so no status or error code is involved.

Cause 1 of 9 — DescribeVpcs omits dhcpOptionsSource and emits an empty ipv6CidrBlockAssociationSet.
AWS: recorded behavior only — the bundled Smithy shape com.amazonaws.ec2#Vpc models vpcId/state/cidrBlock/cidrBlockAssociationSet/ipv6CidrBlockAssociationSet/dhcpOptionsId/instanceTenancy/isDefault/ownerId/tagSet but has no DhcpOptionsSource member, while the recording shows the live API rendering <dhcpOptionsSource>vpc</dhcpOptionsSource> on every VPC and OMITTING ipv6CidrBlockAssociationSet entirely for a VPC with no IPv6 association. The value is the source of the VPC's DHCP option set (fixed token 'vpc' for an account-level/VPC-level association).
Emulator: dhcpOptionsSource is absent and ipv6CidrBlockAssociationSet is present as an empty element.
Why: a client that reads vpc.dhcpOptionsSource to decide whether DHCP options are inherited sees nothing, and an SDK that treats a present-but-empty list element as 'IPv6 association list exists' mis-renders a VPC that has none.
Fix: the DescribeVpcs serializer / Vpc projection builder.
Traces: ec2-ec2mr-byvpc-vpc-create-round-trips-isdefault-and-cidr-associations#task#2.

Cause 2 of 9 — DescribeSubnets omits dhcpOptionsId and dhcpOptionsSource.
AWS: recorded behavior only — the bundled com.amazonaws.ec2#Subnet shape has neither member, and the recording shows the live API rendering both on every subnet in the response (dhcpOptionsId carrying the DHCP option-set id inherited from the subnet's VPC, dhcpOptionsSource carrying the fixed token 'vpc'). The option-set id is a value from state, not a constant.
Emulator: both members are missing from each item of subnetSet.
Why: a caller resolving a subnet's effective DHCP options from DescribeSubnets alone cannot, and must fall back to a VPC lookup that AWS does not require.
Fix: the DescribeSubnets serializer / Subnet projection builder.
Traces: ec2-madl-traj-vpc-subnet-discovery-filters#task#8 (both subnets in the set).

Cause 3 of 9 — DescribeLaunchTemplates reports the wrong creating principal, never advances latestVersionNumber, and omits operator.
AWS: Smithy com.amazonaws.ec2#LaunchTemplate models CreatedBy ('The principal that created the launch template'), LatestVersionNumber ('The version number of the latest version of the launch template') and Operator -> OperatorResponse ('The entity that manages the launch template'). After a second CreateLaunchTemplateVersion the latest version is 2, createdBy is the ARN of the calling principal, and operator renders with managed=false for a customer-managed template.
Emulator: createdBy is the account root ARN instead of the calling IAM principal's ARN, latestVersionNumber stays 1 after a second version exists, and operator is absent.
Why: tooling that picks $Latest from latestVersionNumber launches the stale version, and audit tooling attributes the template to the account root rather than the creating principal.
Fix: the DescribeLaunchTemplates projection plus the launch-template version-counter update in state.
Traces: ec2-ibp-traj-launch-template-from-failed-ami-poll#task#10.

Cause 4 of 9 — DescribeInstances omits instance members and renders only the primary private address.
AWS: the instancesSet item carries productCodes (empty element when none), clientToken (empty element when the launch supplied none), metadataOptions.httpProtocolIpv4 ('enabled' by default), currentInstanceBootMode reflecting the AMI's boot mode ('uefi' for the recorded AMI), and networkInterfaceSet.item.privateIpAddressesSet listing EVERY assigned private address with primary=true/false, where the secondary entries carry an empty privateDnsName.
Emulator: productCodes, clientToken and metadataOptions.httpProtocolIpv4 are absent; currentInstanceBootMode is reported as 'legacy-bios'; privateIpAddressesSet holds only the primary address and fills privateDnsName for it.
Why: a client enumerating an instance's secondary IPs from DescribeInstances sees one address, and boot-mode-sensitive logic picks the wrong launch path.
Fix: the DescribeInstances serializer / instance projection and the ENI-to-instance address join in state.
Traces: ec2-pip-traj-instance-private-ip-reverse-lookup#task#5.

Cause 5 of 9 — DescribeNetworkInterfaces omits requesterId and publicDnsName and invents privateDnsName for secondary addresses.
AWS: the networkInterfaceSet item carries publicDnsName (empty element when the ENI has no public name) and requesterId, and inside privateIpAddressesSet only the PRIMARY entry carries a resolved privateDnsName — secondary entries carry an empty privateDnsName.
Emulator: requesterId and publicDnsName are absent, and secondary entries carry a synthesized ip-<a-b-c-d>.<region>.compute.internal name.
Why: a caller that distinguishes primary from secondary addresses by the presence of a private DNS name mis-classifies every secondary address.
Fix: the DescribeNetworkInterfaces serializer / NetworkInterfacePrivateIpAddress projection.
Traces: ec2-pip-traj-eni-filter-matches-secondary-address#task#4 (cluster also covers task#6 and task#8 of the same fixture).

Cause 6 of 9 — DescribeNetworkInterfaces omits the EIP association block after AssociateAddress.
AWS: once an Elastic IP is associated with an ENI, the ENI projection carries association {publicIp, publicDnsName, ipOwnerId, allocationId, associationId, natEnabled} at the interface level AND on the matching privateIpAddressesSet entry, plus publicDnsName at the interface level.
Emulator: association is absent at both levels and publicDnsName is absent, even though the associate call succeeded.
Why: a client that reads the public IP of an instance's ENI from DescribeNetworkInterfaces concludes the EIP was never attached.
Fix: the DescribeNetworkInterfaces projection plus the address-association link in state.
Traces: ec2-sshp-traj-eip-association-needs-an-attached-igw#task#9.

Cause 7 of 9 — DescribeFlowLogs omits destinationOptions.
AWS: Smithy com.amazonaws.ec2#FlowLog models DestinationOptions -> DestinationOptionsResponse, and every flow log renders it; for a flow log created without explicit destination options the values are the defaults fileFormat=plain-text, hiveCompatiblePartitions=false, perHourPartition=false.
Emulator: destinationOptions is absent from flowLogSet items.
Why: a caller reading the Parquet/partitioning configuration of an existing flow log gets nothing and cannot tell plain-text from parquet delivery.
Fix: the DescribeFlowLogs serializer / FlowLog projection.
Traces: ec2-vfd-describe-filter-name-and-token-mistakes#task#6 (cluster of 10 steps across the ec2-vfd-* fixtures).

Cause 8 of 9 — DescribeFlowLogs omits nextToken when the page is capped.
AWS: when the request supplies MaxResults and more flow logs remain, the response carries nextToken alongside flowLogSet.
Emulator: nextToken is absent, so the response looks like the last page.
Why: a paginating client stops after the first page and silently misses flow logs.
Fix: the DescribeFlowLogs handler's pagination/token emission.
Traces: ec2-vfd-s3-reference-route-and-destination-filters#task#11.

Cause 9 of 9 — DescribeAddresses omits the association members of an associated Elastic IP.
AWS: an Elastic IP associated with an instance renders instanceId, associationId, networkInterfaceId, networkInterfaceOwnerId and privateIpAddress inside the addressesSet item.
Emulator: all five members are absent, so the address reads as unassociated.
Why: a client that locates the instance behind an EIP (or releases only unassociated EIPs) acts on wrong data.
Fix: the DescribeAddresses projection plus the address-to-ENI/instance link in state.
Traces: ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap#task#25.

## Evidence
All nine causes are 200 vs 200 body mismatches reported by the wire replay.
Cause 1: $.DescribeVpcsResponse.vpcSet.item.dhcpOptionsSource missing (expected 'vpc', got None); $.DescribeVpcsResponse.vpcSet.item.ipv6CidrBlockAssociationSet unexpected_present (expected None, got '').
Cause 2: $.DescribeSubnetsResponse.subnetSet.item[*].dhcpOptionsId missing (expected the VPC's dopt- id, got None); .dhcpOptionsSource missing (expected 'vpc', got None) for both subnets.
Cause 3: .createdBy value_mismatch (expected 'arn:aws:iam::<acct>:user/<principal>', got 'arn:aws:iam::<acct>:root'); .latestVersionNumber value_mismatch (expected '2', got '1'); .operator missing (expected {'managed':'false'}, got None).
Cause 4: .productCodes missing (expected '', got None); .clientToken missing (expected '', got None); .metadataOptions.httpProtocolIpv4 missing (expected 'enabled', got None); .currentInstanceBootMode value_mismatch (expected 'uefi', got 'legacy-bios'); .networkInterfaceSet.item.privateIpAddressesSet.item type_mismatch (expected a 2-element list with primary=true/false, got a single primary entry).
Cause 5: .requesterId missing; .publicDnsName missing (expected ''); .privateIpAddressesSet.item[1]/[2].privateDnsName value_mismatch (expected '', got a synthesized ip-...compute.internal name).
Cause 6: .association missing at interface and per-address level; .publicDnsName missing (expected '').
Cause 7: $.DescribeFlowLogsResponse.flowLogSet.item.destinationOptions missing (expected {'fileFormat':'plain-text','hiveCompatiblePartitions':'false','perHourPartition':'false'}, got None).
Cause 8: $.DescribeFlowLogsResponse.nextToken missing (expected a token, got None).
Cause 9: $.DescribeAddressesResponse.addressesSet.item.{instanceId,associationId,networkInterfaceId,networkInterfaceOwnerId,privateIpAddress} all missing.

## Steps to Reproduce
Replay the wire corpus (trace set simparity-wire-aws-v1) against the emulator and inspect the listed fixture steps: ec2-ec2mr-byvpc-vpc-create-round-trips-isdefault-and-cidr-associations#task#2, ec2-madl-traj-vpc-subnet-discovery-filters#task#8, ec2-ibp-traj-launch-template-from-failed-ami-poll#task#10, ec2-pip-traj-instance-private-ip-reverse-lookup#task#5, ec2-pip-traj-eni-filter-matches-secondary-address#task#4, ec2-sshp-traj-eip-association-needs-an-attached-igw#task#9, ec2-vfd-describe-filter-name-and-token-mistakes#task#6, ec2-vfd-s3-reference-route-and-destination-filters#task#11, ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap#task#25. Each step issues the named Describe call after the fixture's setup and compares the rendered projection against the recorded AWS response.
