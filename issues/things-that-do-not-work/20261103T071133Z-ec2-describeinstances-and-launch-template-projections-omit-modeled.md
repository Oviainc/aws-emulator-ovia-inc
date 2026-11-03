Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-byvpc-traj-one-call-for-several-vpcs-filter-semantics
Issue-Step: task#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-byvpc-traj-one-call-for-several-vpcs-filter-semantics ec2-trajwire-deb-deb-launch-then-attach-device-mistakes ec2-ecc-traj-os-classification-platform-vs-platform-details ec2-byvpc-traj-stopped-instance-stays-in-its-vpc ec2-byvpc-traj-terminated-instance-loses-vpc-attribution ec2-xrg-traj-instance-id-is-region-scoped ec2-byvpc-traj-batched-ids-hit-per-call-limits ec2-pip-traj-instance-private-ip-reverse-lookup ec2-sshr-traj-instances-in-the-open-group-split-on-public-ip ec2-w22n-traj-ec2-describe-instances-route ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch ec2-asgssh-launch-template-iam-profile-reference-accepted-unvalidated-with-warning ec2-asgssh-launch-template-without-key-name-omits-it-from-version-data ec2-ibp-traj-launch-template-from-failed-ami-poll ec2-asghc-cluster-sg-self-rules-round-trip-as-group-pairs-with-no-address-source ec2-asgssh-nacl-entries-round-trip-deny-22-before-allow-all
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.clientToken
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item[*]
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.reason
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.metadataOptions.state
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.privateIpAddress
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.imageId
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.instanceState.code
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.instanceState.name
Issue-Key: ec2|DescribeInstances|2xx->4xx|$.DescribeInstancesResponse
Issue-Key: ec2|DescribeInstances|2xx->4xx|status
Issue-Key: ec2|CreateLaunchTemplate|2xx->2xx|$.CreateLaunchTemplateResponse.launchTemplate.createdBy
Issue-Key: ec2|CreateLaunchTemplate|2xx->2xx|$.CreateLaunchTemplateResponse.warning
Issue-Key: ec2|DescribeSecurityGroupRules|2xx->2xx|$.DescribeSecurityGroupRulesResponse.securityGroupRuleSet.item[*]
Issue-Key: ec2|DescribeNetworkAcls|2xx->2xx|$.DescribeNetworkAclsResponse.networkAclSet.item.entrySet.item[*]
Issue-Related: 20261023T102725Z-ec2-response-bodies-omit-modeled-members-and-report.md
Issue-Related: 20261031T181451Z-ec2-instance-volume-snapshot-and-iam-profile-projections.md
Issue-Related: 20261101T141251Z-ec2-peering-id-format-prefix-list-and-capacity.md
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# EC2 DescribeInstances and launch-template projections omit modeled members and misreport terminal state

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances

## Description
Ten 200-response projections differ from the recordings: members AWS always emits are absent, empty structures are serialized as empty elements instead of being omitted, a terminated instance keeps its placement attributes, and one read that AWS answers with an empty set is refused. These are read-path/serializer defects in the instance, launch-template, security-group-rule and network-ACL projections.

Cause 1 of 10: DescribeInstances omits always-present instance members.
AWS: recorded behavior only — every instance carries productCodes (empty element), clientToken (empty element), metadataOptions.httpProtocolIpv4 'enabled' and currentInstanceBootMode 'uefi' for the recorded AMI.
Emulator: productCodes, clientToken and metadataOptions.httpProtocolIpv4 are absent and currentInstanceBootMode is 'legacy-bios'.
Why: SDK consumers that read metadataOptions.httpProtocolIpv4 or boot mode get nothing/the wrong value; the boot mode is a property of the AMI, not a constant.
Fix: the instance projection/serializer behind services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_instances.rs.
Traces: ec2-byvpc-traj-one-call-for-several-vpcs-filter-semantics#task#13 (cluster also covers ec2-trajwire-deb-deb-launch-then-attach-device-mistakes#task#14).

Cause 2 of 10: DescribeInstances reservation items differ wholesale.
AWS: recorded behavior only — each reservationSet.item carries reservationId, ownerId, empty groupSet and an instancesSet item with operator.managed/hiddenByDefault, placement.availabilityZoneId, monitoring, blockDeviceMapping.ebs.ebsCardIndex, networkInterfaceSet with attachment.networkCardIndex and the request's tagSet.
Emulator: the item does not match the recorded structure (members missing/extra).
Why: a client that walks reservations cannot find the members AWS guarantees.
Fix: as cause 1.
Traces: ec2-byvpc-traj-one-call-for-several-vpcs-filter-semantics#task#8 (cluster also covers task#12 and ec2-ecc-traj-os-classification-platform-vs-platform-details#task#5).

Cause 3 of 10: a STOPPED instance's projection.
AWS: recorded behavior only — reason is present (AWS's form 'User initiated (YYYY-MM-DD HH:MM:SS GMT)'), productCodes and clientToken are present-but-empty, metadataOptions.httpProtocolIpv4 'enabled', currentInstanceBootMode 'uefi'.
Emulator: same omissions as cause 1 and reason rendered as an ISO-8601 timestamp ('User initiated (2026-11-03T06:18:58.688599Z)') rather than AWS's 'YYYY-MM-DD HH:MM:SS GMT' form. (The instant itself is per-run and is not the finding; the format is.)
Why: consumers that parse reason's timestamp fail.
Fix: as cause 1, plus the reason formatter in the stop path.
Traces: ec2-byvpc-traj-stopped-instance-stays-in-its-vpc#task#9 (cluster also covers task#10).

Cause 4 of 10: a TERMINATED instance keeps placement and storage attributes.
AWS: recorded behavior only — after termination privateDnsName is empty, subnetId and privateIpAddress are ABSENT, blockDeviceMapping is empty, and metadataOptions.state is 'pending'.
Emulator: privateDnsName still 'ip-10-224-1-4.us-west-2.compute.internal', subnetId and privateIpAddress still present, blockDeviceMapping still lists the root volume, metadataOptions.state 'applied'.
Why: a terminated instance still looks attributable to a subnet/VPC and still owns storage, so cleanup verification and VPC-deletion preconditions behave differently from AWS.
Fix: the terminate transition in the instance state store plus the projection in describe_instances.rs.
Traces: ec2-byvpc-traj-terminated-instance-loses-vpc-attribution#task#7 (cluster also covers task#10).

Cause 5 of 10: a launched instance never reaches the recorded running projection.
AWS: recorded behavior only — DescribeInstances settles on instanceState code 16 / 'running' with the request's imageId, productCodes/clientToken present-but-empty, reason empty and metadataOptions.httpProtocolIpv4 'enabled'.
Emulator: the poll is capped at 60s without ever matching (imageId, instanceState, reason and the members from cause 1 all diverge).
Why: any trajectory that waits for 'running' burns its whole budget and then fails.
Fix: as causes 1 and 4.
Traces: ec2-xrg-traj-instance-id-is-region-scoped#task#8.

Cause 6 of 10: a filter-only DescribeInstances that matches nothing is refused.
AWS: recorded behavior only — 200 with DescribeInstancesResponse.reservationSet serialized as empty.
Emulator: 400 (no response body).
Why: 'no instances match' is a normal answer; refusing it turns an empty result into an error for 13 steps across six fixtures.
Fix: filter resolution in describe_instances.rs must answer an empty reservationSet rather than refusing.
Traces: ec2-byvpc-traj-batched-ids-hit-per-call-limits#task#2 (cluster also covers ec2-ecc-traj-os-classification-platform-vs-platform-details#task#6..#10, ec2-pip-traj-instance-private-ip-reverse-lookup#task#7, ec2-sshr-traj-instances-in-the-open-group-split-on-public-ip#task#13..#16, ec2-w22n-traj-ec2-describe-instances-route#task#1/#2).

Cause 7 of 10: CreateLaunchTemplate response members and warning.
AWS: recorded behavior only — launchTemplate.createdBy is the ARN of the CALLING principal (an IAM user ARN of the form arn:aws:iam::<account>:user/<name>), launchTemplate.operator carries managed 'false', and a template that specifies a network interface without security groups returns a warning.errorSet.item with code 'InvalidParameterCombination' and message 'If you specify a network interface, you must specify all security groups as part of the network interface.'
Emulator: createdBy is the account root ARN (arn:aws:iam::<account>:root), operator is absent and warning is absent entirely.
Why: the caller is never told its template is internally inconsistent (AWS warns at create and refuses at launch), and createdBy misattributes authorship.
Fix: create_launch_template.rs (principal resolution, operator projection, warning construction).
Traces: ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch#task#5 (cluster also covers ec2-asgssh-launch-template-iam-profile-reference-accepted-unvalidated-with-warning#task#2, ec2-asgssh-launch-template-without-key-name-omits-it-from-version-data#task#5).

Cause 8 of 10: CreateLaunchTemplate createdBy/operator on templates with no warning.
AWS: as cause 7 — createdBy = calling-user ARN, operator.managed 'false'.
Emulator: root ARN, operator absent.
Why/Fix: as cause 7 (same two members, separate cluster).
Traces: ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch#task#7 (cluster also covers task#14 and ec2-ibp-traj-launch-template-from-failed-ami-poll#task#4).

Cause 9 of 10: DescribeSecurityGroupRules misrepresents all-traffic self rules.
AWS: recorded behavior only — an all-traffic self rule is described with ipProtocol '-1', fromPort '-1', toPort '-1', referencedGroupInfo naming the group, and NO cidrIpv4/cidrIpv6/prefixListId.
Emulator: returns TCP/988-scoped rules in those positions (ipProtocol 'tcp', fromPort/toPort '988') and adds securityGroupRuleArn, so the ingress and egress all-traffic rules AWS reports are absent.
Why: a caller auditing which rules allow all traffic from the group's own members gets the wrong answer.
Fix: the rule projection in describe_security_group_rules.rs and the rule representation stored by the authorize path.
Traces: ec2-asghc-cluster-sg-self-rules-round-trip-as-group-pairs-with-no-address-source#task#8.

Cause 10 of 10: DescribeNetworkAcls serializes absent sub-structures as empty elements.
AWS: recorded behavior only — an entry with no ICMP type/code and no port range OMITS icmpTypeCode and portRange from the entry.
Emulator: emits icmpTypeCode '' and portRange '' on every such entry.
Why: XML-to-object mapping yields an empty structure where AWS yields null, so consumers that test for presence mis-read every ACL entry.
Fix: the ec2Query serializer for the network-ACL entry projection behind describe_network_acls.rs.
Traces: ec2-asgssh-nacl-entries-round-trip-deny-22-before-allow-all#task#7.

## Evidence
Replayed wire corpus simparity-wire-aws-v1. Representative: ec2-byvpc-traj-one-call-for-several-vpcs-filter-semantics task#13 — expected productCodes '', clientToken '', metadataOptions.httpProtocolIpv4 'enabled', currentInstanceBootMode 'uefi'; actual the first three missing and bootMode 'legacy-bios' (status 200 both). Further clusters: reservation item shape; stopped-instance projection; terminated instance keeping subnetId/privateIpAddress/blockDeviceMapping and metadataOptions.state 'applied' vs 'pending'; a running-state poll capped at 60s; a filter-only DescribeInstances answered 400 where the recording has 200 with an empty reservationSet (13 steps); CreateLaunchTemplate createdBy 'arn:aws:iam::000000000000:root' vs 'arn:aws:iam::000000000000:user/emulator' plus missing operator and missing warning.errorSet; DescribeSecurityGroupRules self-rule items; DescribeNetworkAcls icmpTypeCode/portRange emitted as ''.

## Steps to Reproduce
1. Launch an instance and call DescribeInstances (fixture ec2-byvpc-traj-one-call-for-several-vpcs-filter-semantics, step task#13); diff the instancesSet item against the recording.
2. Terminate it and re-describe (ec2-byvpc-traj-terminated-instance-loses-vpc-attribution, task#7): observe subnetId/privateIpAddress/blockDeviceMapping still present.
3. Call DescribeInstances with a filter that matches nothing (ec2-byvpc-traj-batched-ids-hit-per-call-limits, task#2): observe 400 instead of an empty reservationSet.
4. CreateLaunchTemplate with a network interface and no security groups (ec2-asgeni-traj-launch-template-ni-accepted-then-refused-at-launch, task#5): observe missing warning/operator and the root createdBy ARN.
5. Replay the remaining fixture#step tokens in trace_ids.
