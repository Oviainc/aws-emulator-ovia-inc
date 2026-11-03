Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-albos-eni-lookup-id-and-filter-mistakes
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-albos-eni-lookup-id-and-filter-mistakes ec2-asgeni-traj-attach-device-index-and-limit ec2-asgeni-traj-attach-placement-ids-and-teardown-order ec2-t12-eni-attach-and-private-ip ec2-pip-traj-ipv6-on-ipv4-only-subnet ec2-pip-traj-same-private-ip-in-two-vpcs ec2-sshp-traj-subnet-nacl-is-always-explicitly-associated ec2-usg-traj-default-group-unused-undeletable-dies-with-vpc ec2-usg-traj-groupname-lookup-resolves-only-in-default-vpc ec2-sshp-traj-prefix-list-source-is-opaque-to-cidr-filters ec2-sshp-traj-ssh-ingress-rule-authoring-mistakes ec2-ipam-trajwire-child-pool-create-mistakes ec2-vvd-traj-vpc-tag-spec-mistakes-and-filter-lookup
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DetachNetworkInterface|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AssignIpv6Addresses|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateNetworkInterface|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteNetworkAcl|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteSecurityGroup|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AuthorizeSecurityGroupIngress|4xx->4xx|$.Response.Errors.Error.Code
Issue-Related: 20261029T200817Z-ec2-already-exists-and-malformed-id-conflict-messages.md
Issue-Related: 20261103T071133Z-ec2-confuses-malformed-and-not-found-ids-and.md
Issue-Related: 20261103T071133Z-ec2-invalid-enum-and-attribute-values-are-refused.md
Issue-Related: 20261103T071133Z-ec2-missing-required-parameters-are-refused-as-malformedinput.md
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# EC2 refusal messages are reworded across network, security-group and VPC create paths

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeNetworkInterfaces

## Description
Ten refusals return the right status (400) and — except for cause 8 — the right error code, but the message is emulator-authored rather than AWS's. An error message is part of the contract: tests, runbooks and support tooling match on it. Each cause below gives AWS's sentence, naming which parts are fixed text and which are taken from the request or state.

Cause 1 of 10: DescribeNetworkInterfaces with both NetworkInterfaceIds and MaxResults.
AWS: recorded behavior only — 'The parameter NetworkInterfaceIds cannot be used with the parameter MaxResults' (fixed text; the two parameter names are AWS's, with the ids parameter named FIRST and in plural form).
Emulator: 'The parameter MaxResults cannot be used with the parameter NetworkInterfaceId' — operands swapped and the singular name used.
Why: tooling that parses the offending parameter out of the sentence removes the wrong one.
Fix: the conflict check in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_network_interfaces.rs.
Traces: ec2-albos-eni-lookup-id-and-filter-mistakes#task#5.

Cause 2 of 10: AttachNetworkInterface at an occupied device index.
AWS: recorded behavior only — "Instance 'i-00000000000000001' already has an interface attached at device index '0'." (fixed text with the instance id and device index from the request/state).
Emulator: "Value (0) for parameter deviceIndex is invalid. Instance 'i-00000000000000001' already has an interface attached at device index '0'" — prefixes AWS's sentence with a parameter-value clause and drops the trailing period.
Fix: the duplicate-index refusal in attach_network_interface.rs.
Why: message mismatch as above.
Traces: ec2-asgeni-traj-attach-device-index-and-limit#task#7 (cluster also covers task#9).

Cause 3 of 10: DetachNetworkInterface on the primary interface.
AWS: recorded behavior only — 'The network interface at device index 0 and networkCard index 0 cannot be detached.' (note AWS's camelCase 'networkCard').
Emulator: '... and networkcard index 0 cannot be detached.' (lower-cased).
Fix: refusal text in detach_network_interface.rs.
Why: byte-exact message mismatch.
Traces: ec2-asgeni-traj-attach-placement-ids-and-teardown-order#task#16 (cluster also covers ec2-t12-eni-attach-and-private-ip#task#15).

Cause 4 of 10: AssignIpv6Addresses on an IPv4-only subnet.
AWS: recorded behavior only — 'Subnet does not contain any IPv6 CIDR block ranges' (fixed text, no id substituted).
Emulator: "The subnet 'subnet-0000000000000002b' does not have an associated IPv6 CIDR block".
Fix: refusal text in assign_ipv6_addresses.rs.
Why: message mismatch; the emulator also injects an id AWS does not include.
Traces: ec2-pip-traj-ipv6-on-ipv4-only-subnet#task#5 (cluster also covers task#6).

Cause 5 of 10: CreateNetworkInterface with a private IP already in use.
AWS: recorded behavior only — 'The specified address is already in use.' (fixed text, no address substituted).
Emulator: 'Address 10.241.1.10 is in use'.
Fix: refusal text in create_network_interface.rs.
Why: message mismatch.
Traces: ec2-pip-traj-same-private-ip-in-two-vpcs#task#9.

Cause 6 of 10: DeleteNetworkAcl on a VPC's default ACL.
AWS: recorded behavior only — 'cannot delete default network ACL acl-00000000000000027' (lowercase start, the ACL id from the request appended bare).
Emulator: "The network acl 'acl-00000000000000027' is the default network acl and cannot be deleted".
Fix: refusal text in delete_network_acl.rs.
Why: message mismatch.
Traces: ec2-sshp-traj-subnet-nacl-is-always-explicitly-associated#task#12.

Cause 7 of 10: DeleteSecurityGroup on a VPC's default group.
AWS: recorded behavior only — 'the specified group: "sg-00000000000000028" name: "default" cannot be deleted by a user' (lowercase start; the group id and group name are taken from state and double-quoted).
Emulator: "The security group 'sg-00000000000000028' is the default group of its VPC and cannot be deleted".
Fix: refusal text in delete_security_group.rs.
Why: message mismatch.
Traces: ec2-usg-traj-default-group-unused-undeletable-dies-with-vpc#task#5 (cluster also covers ec2-usg-traj-groupname-lookup-resolves-only-in-default-vpc#task#6).

Cause 8 of 10: AuthorizeSecurityGroupIngress naming an unknown prefix list — wrong code AND message.
AWS: recorded behavior only — 400 Code 'InvalidPrefixListID.NotFound', Message "The prefix list ID 'pl-corpvpn' does not exist".
Emulator: 400 Code 'InvalidParameterValue', Message "Invalid prefix list id 'pl-corpvpn' for ipPermissions.1".
Why: the dedicated NotFound code is lost, so a caller cannot distinguish an absent prefix list from a malformed rule.
Fix: prefix-list resolution in authorize_security_group_ingress.rs.
Traces: ec2-sshp-traj-prefix-list-source-is-opaque-to-cidr-filters#task#6 (cluster also covers ec2-sshp-traj-ssh-ingress-rule-authoring-mistakes#task#5).

Cause 9 of 10: CreateIpamPool with an upper-cased Locale.
AWS: Smithy: CreateIpamPool.Locale is a String (no enum) — the invalid value is refused 400 with Message 'The locale US-WEST-2 is not valid.' (fixed text 'The locale ' + the request's value + ' is not valid.').
Emulator: "Invalid value 'US-WEST-2' for Locale. It must be an Amazon Web Services Region or the network border group of a supported Local Zone".
Fix: locale validation text in create_ipam_pool.rs.
Why: message mismatch.
Traces: ec2-ipam-trajwire-child-pool-create-mistakes#task#6.

Cause 10 of 10: CreateVpc with neither CidrBlock nor Ipv4IpamPoolId.
AWS: recorded behavior only — "Either 'cidrBlock' or 'ipv4IpamPoolId' should be provided." (fixed text, wire member names single-quoted, 'should be provided').
Emulator: "Either 'cidrBlock' or 'ipv4IpamPoolId' must be specified".
Fix: refusal text in create_vpc.rs.
Why: message mismatch.
Traces: ec2-vvd-traj-vpc-tag-spec-mistakes-and-filter-lookup#task#1.

## Evidence
Replayed wire corpus simparity-wire-aws-v1. Representative: ec2-albos-eni-lookup-id-and-filter-mistakes task#5 — expected Message 'The parameter NetworkInterfaceIds cannot be used with the parameter MaxResults'; actual 'The parameter MaxResults cannot be used with the parameter NetworkInterfaceId' (status 400, code matched). Nine further clusters show the same class on AttachNetworkInterface, DetachNetworkInterface, AssignIpv6Addresses, CreateNetworkInterface, DeleteNetworkAcl, DeleteSecurityGroup, AuthorizeSecurityGroupIngress (code InvalidPrefixListID.NotFound lost), CreateIpamPool and CreateVpc.

## Steps to Reproduce
1. Call DescribeNetworkInterfaces with both NetworkInterfaceId.1 and MaxResults (fixture ec2-albos-eni-lookup-id-and-filter-mistakes, task#5) and compare the message word for word.
2. Replay each fixture#step listed in trace_ids and diff Response.Errors.Error.Message (and, for the prefix-list cause, the Code) against the recording.
