Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.asgdx-duplicate-health-check-ingress-rejected ec2.asghc-authorize-duplicate-health-check-ingress-rejected ec2.op-contract-authorize-security-group-egress ec2.asgdx-sg-ingress-by-protocol-number-is-stored-as-the-protocol-name ec2.asgeni-eni-with-deleted-sg-rejected ec2.asgeni-eni-with-sg-from-another-vpc-rejected ec2.asgeni-secondary-eni-cross-az-attach-rejected ec2.ec2mr-usg-delete-default-sg-rejected-cannotdelete ec2.default-security-group-delete-fails ec2.ec2mr-usg-duplicate-group-name-in-vpc-rejected ec2.ec2mr-usg-cross-vpc-group-reference-rejected ec2.network-interface-attach-detach-lifecycle ec2.ec2mr-pip-eni-secondary-ip-set-round-trips ec2.asgssh-default-nacl-delete-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.asgdx-duplicate-health-check-ingress-rejected|run#11|AssertionError|assert got == expected, f"re-authorizing the same rule must be {expected!r}, got {got!r}"
Issue-Key: ec2.asghc-authorize-duplicate-health-check-ingress-rejected|run#18|AssertionError|assert msg == dup, f"re-applied fix message: {msg!r}"
Issue-Key: ec2.op-contract-authorize-security-group-egress|run#28|AssertionError|assert message == (
Issue-Key: ec2.asgdx-sg-ingress-by-protocol-number-is-stored-as-the-protocol-name|run#11|AssertionError|assert perms == [("tcp", 80, 80, [_CIDR])], (
Issue-Key: ec2.asgeni-eni-with-deleted-sg-rejected|run#8|AssertionError|assert err == want, (
Issue-Key: ec2.asgeni-eni-with-sg-from-another-vpc-rejected|run#9|AssertionError|assert message == "You have specified two resources that belong to different networks.", (
Issue-Key: ec2.asgeni-secondary-eni-cross-az-attach-rejected|run#20|AssertionError|assert err == want, (
Issue-Key: ec2.ec2mr-usg-delete-default-sg-rejected-cannotdelete|run#15|AssertionError|assert (code, message) == ("CannotDelete", expected_message), (
Issue-Key: ec2.default-security-group-delete-fails|run#15|AssertionError|assert message == f'the specified group: "{default_gid}" name: "default" cannot be deleted by a user', message
Issue-Key: ec2|CreateSecurityGroup|InvalidGroup.Duplicate
Issue-Key: ec2.ec2mr-usg-cross-vpc-group-reference-rejected|run#11|AssertionError|raise AssertionError(
Issue-Key: ec2|DeleteNetworkInterface|InvalidNetworkInterface.InUse
Issue-Key: ec2.ec2mr-pip-eni-secondary-ip-set-round-trips|run#27|AssertionError|assert "PrivateDnsName" not in prim_entry, (
Issue-Key: ec2.asgssh-default-nacl-delete-rejected|run#7|AssertionError|assert (code, message) == (
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# EC2 security-group and network-interface refusals: wrong error codes, rule-id-based duplicate messages, numeric protocol echo, and missing cross-VPC validation (10 root causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AuthorizeSecurityGroupIngress

## Description
Ten net-new root causes in the EC2 security-group / network-interface surface. The EC2 Smithy model is ec2Query and declares no per-operation errors (smithy_query operation=AuthorizeSecurityGroupIngress,AttachNetworkInterface -> "errors": []), so every code/message below is grounded in the replayed recording of real AWS; where that is the only source it is marked "recorded behavior only".

Cause 1 of 10: duplicate security-group rule refusal names the generated rule id instead of the rule.
AWS: recorded behavior only. Re-authorizing an identical ingress/egress rule is refused HTTP 400 InvalidPermission.Duplicate with the message `the specified rule "peer: <cidr>, <PROTOCOL>, from port: <from>, to port: <to>, <ACTION>" already exists`. Fixed text: `the specified rule "` ... `" already exists`; the variable part is the rule tuple rendered from the REQUEST (peer CIDR or group, upper-case protocol, from/to port, ALLOW/DENY) - never a stored id.
Emulator: same code, but the message renders the emulator's own rule id, e.g. `the specified rule "sgr-00000000000000039" already exists`.
Why: a client that parses the duplicate message to learn WHICH rule collided (idempotent reconcilers, CDK/Terraform-style diffing) gets an opaque server-side id that is absent from its own request and cannot be matched to anything it asked for.
Fix: the message template in the AuthorizeSecurityGroupIngress / AuthorizeSecurityGroupEgress handlers' duplicate-rule rejection path.
Traces: ec2.asgdx-duplicate-health-check-ingress-rejected, ec2.asghc-authorize-duplicate-health-check-ingress-rejected, ec2.op-contract-authorize-security-group-egress. (The open issue 20261029T175114Z-ec2-refusal-messages-are-emulator-authored-prose-instead carries the ingress op-contract variant only; these three checks are not on it.)

Cause 2 of 10: DescribeSecurityGroups echoes the numeric IpProtocol instead of the protocol name.
AWS: recorded behavior only. A rule authorized with IpProtocol="6" reads back from DescribeSecurityGroups with IpProtocol="tcp" (AWS normalises the well-known numbers 6/17/1 to tcp/udp/icmp on the read path).
Emulator: reads back IpProtocol="6" - the replay shows `[('6', 80, 80, ['198.51.100.0/24'])]` where `('tcp', 80, 80, ...)` is required.
Why: clients compare the desired rule set to the described one; the un-normalised value makes every such rule look different from the one that was requested, producing endless no-op rewrites.
Fix: the DescribeSecurityGroups serializer / the IpPermission projection in security-group state.
Traces: ec2.asgdx-sg-ingress-by-protocol-number-is-stored-as-the-protocol-name

Cause 3 of 10: CreateNetworkInterface uses the group-name error class for a group ID.
AWS: recorded behavior only. Naming a deleted or foreign-VPC security group BY ID is refused InvalidSecurityGroupID.NotFound with `The securityGroup ID '<sg-id>' does not exist` (fixed text `The securityGroup ID '` / `' does not exist`, id taken from the request).
Emulator: answers InvalidGroup.NotFound with `The security group '<sg-id>' does not exist`.
Why: InvalidGroup.NotFound is AWS's by-NAME class; SDK/retry logic and IaC error mapping keyed on InvalidSecurityGroupID.NotFound never matches, so a legitimate not-found is treated as an unknown failure.
Fix: the group-resolution validation in the CreateNetworkInterface handler (choose the ID error class when the caller supplied ids).
Traces: ec2.asgeni-eni-with-deleted-sg-rejected, ec2.asgeni-eni-with-sg-from-another-vpc-rejected

Cause 4 of 10: AttachNetworkInterface cross-AZ attach uses the wrong error class and message.
AWS: recorded behavior only. Attaching an ENI in one AZ to an instance in another is refused InvalidParameterCombination with the fixed message `You may not attach a network interface to an instance if they are not in the same availability zone` (no variable parts).
Emulator: answers InvalidParameterValue with emulator-authored prose `Value (<eni-id>) for parameter networkInterfaceId is invalid. The network interface is in the Availability Zone <az> and the instance is in <az>`.
Why: the request is a bad COMBINATION of two individually valid parameters; a client branching on InvalidParameterCombination to re-pick a subnet never sees it.
Fix: the AZ-consistency check in the AttachNetworkInterface handler's validation.
Traces: ec2.asgeni-secondary-eni-cross-az-attach-rejected

Cause 5 of 10: deleting a VPC's default security group is refused with the wrong message template.
AWS: recorded behavior only. DeleteSecurityGroup on the default group is refused CannotDelete with `the specified group: "<sg-id>" name: "default" cannot be deleted by a user` - fixed text around two variables: the group id from the request and the group's name from state.
Emulator: answers `The security group '<sg-id>' is the default group of its VPC and cannot be deleted`.
Why: the message is the only place AWS states the reason; operators and log scrapers keyed on AWS's wording see an unrecognised refusal.
Fix: the default-group guard in the DeleteSecurityGroup handler.
Traces: ec2.ec2mr-usg-delete-default-sg-rejected-cannotdelete, ec2.default-security-group-delete-fails

Cause 6 of 10: duplicate security-group name message omits the VPC id.
AWS: recorded behavior only. CreateSecurityGroup with a name already used in the VPC is refused with `The security group '<name>' already exists for VPC '<vpc-id>'` - the trailing `for VPC '<vpc-id>'` is required, with the id taken from the request/state.
Emulator: truncates to `The security group '<name>' already exists for VPC` (the id is missing entirely).
Why: in multi-VPC accounts the message is how a caller learns which VPC already holds the name; without it the refusal is unactionable.
Fix: the duplicate-name rejection in the CreateSecurityGroup handler.
Traces: ec2.ec2mr-usg-duplicate-group-name-in-vpc-rejected

Cause 7 of 10: a security-group rule may reference a group in another VPC.
AWS: recorded behavior only. Authorizing a rule whose UserIdGroupPairs names a group in a DIFFERENT VPC is refused InvalidGroup.NotFound (the peer group is not visible from the rule's VPC).
Emulator: accepts it with HTTP 200 - the replay shows a rule on a group in one VPC referencing a group in another VPC being stored.
Why: the emulator lets a configuration that can never carry traffic on AWS be provisioned, so a reachability or network-segmentation test passes locally and fails in production.
Fix: the peer-group resolution in the AuthorizeSecurityGroupIngress/Egress handler must scope the lookup to the rule's VPC.
Traces: ec2.ec2mr-usg-cross-vpc-group-reference-rejected

Cause 8 of 10: DeleteNetworkInterface on an attached ENI uses the wrong error class.
AWS: recorded behavior only. Deleting an ENI that is still attached is refused InvalidParameterValue.
Emulator: answers InvalidNetworkInterface.InUse.
Why: callers that treat InvalidParameterValue as "fix the request" versus an in-use conflict code branch differently; the detach-then-delete loop keyed on AWS's class never triggers.
Fix: the attachment guard in the DeleteNetworkInterface handler.
Traces: ec2.network-interface-attach-detach-lifecycle

Cause 9 of 10: ENI private-address entries carry PrivateDnsName in a VPC with DNS hostnames disabled.
AWS: recorded behavior only. When the ENI's VPC has enableDnsHostnames=false, each PrivateIpAddresses entry OMITS PrivateDnsName; the member is absent, not empty.
Emulator: emits it anyway, e.g. `{'Primary': True, 'PrivateDnsName': 'ip-10-61-1-4.<region>.compute.internal', 'PrivateIpAddress': '10.61.1.4'}`.
Why: the presence of PrivateDnsName is how a client detects that in-VPC DNS hostnames are on; the emulator makes every VPC look DNS-enabled.
Fix: the DescribeNetworkInterfaces / DescribeInstances address serializer must consult the VPC's enableDnsHostnames attribute.
Traces: ec2.ec2mr-pip-eni-secondary-ip-set-round-trips

Cause 10 of 10: deleting the default network ACL is refused with the wrong code/message.
AWS: recorded behavior only. DeleteNetworkAcl on a VPC's default ACL is refused with AWS's own default-ACL class and wording, not a parameter-value error.
Emulator: answers InvalidParameterValue with `The network acl '<acl-id>' is the default network acl and cannot be deleted`.
Why: a client cannot distinguish "this ACL is structurally undeletable" from "you passed a bad id", so cleanup code retries forever.
Fix: the default-ACL guard in the DeleteNetworkAcl handler.
Traces: ec2.asgssh-default-nacl-delete-rejected

## Evidence
Replayed failing checks (verdict: mismatch) and their asserted-vs-actual values:

1. ec2.asgdx-duplicate-health-check-ingress-rejected -> AssertionError: re-authorizing the same rule must be ('InvalidPermission.Duplicate', 'the specified rule "peer: 172.25.0.0/23, TCP, from port: 4000, to port: 4000, ALLOW" already exists'), got ('InvalidPermission.Duplicate', 'the specified rule "sgr-00000000000000039" already exists')
   ec2.asghc-authorize-duplicate-health-check-ingress-rejected -> AssertionError: re-applied fix message: 'the specified rule "sgr-00000000000000042" already exists'
   ec2.op-contract-authorize-security-group-egress -> AssertionError: the specified rule "sgr-00000000000000039" already exists
2. ec2.asgdx-sg-ingress-by-protocol-number-is-stored-as-the-protocol-name -> AssertionError: DescribeSecurityGroups must report protocol 6 as 'tcp' 80-80 from 198.51.100.0/24, got [('6', 80, 80, ['198.51.100.0/24'])]
3. ec2.asgeni-eni-with-deleted-sg-rejected -> AssertionError: CreateNetworkInterface with deleted group 'sg-0000000000000002f' answered ('InvalidGroup.NotFound', "The security group 'sg-0000000000000002f' does not exist"), expected ('InvalidSecurityGroupID.NotFound', "The securityGroup ID 'sg-0000000000000002f' does not exist")
   ec2.asgeni-eni-with-sg-from-another-vpc-rejected -> AssertionError: unexpected InvalidGroup.NotFound message; got "The security group 'sg-00000000000000035' does not exist"
4. ec2.asgeni-secondary-eni-cross-az-attach-rejected -> AssertionError: attaching 'eni-...' (zone us-west-2b) to 'i-...' (zone us-west-2a) answered ('InvalidParameterValue', 'Value (eni-...) for parameter networkInterfaceId is invalid. The network interface is in the Availability Zone us-west-2b and the instance is in us-west-2a'), expected ('InvalidParameterCombination', 'You may not attach a network interface to an instance if they are not in the same availability zone')
5. ec2.ec2mr-usg-delete-default-sg-rejected-cannotdelete -> AssertionError: deleting the default security group raised ('CannotDelete', "The security group 'sg-...' is the default group of its VPC and cannot be deleted"), expected ('CannotDelete', 'the specified group: "sg-..." name: "default" cannot be deleted by a user')
   ec2.default-security-group-delete-fails -> AssertionError: The security group 'sg-00000000000000028' is the default group of its VPC and cannot be deleted
6. ec2.ec2mr-usg-duplicate-group-name-in-vpc-rejected -> AssertionError: expected the message "The security group '<name>' already exists for VPC 'vpc-00000000000000024'", got "The security group '<name>' already exists for VPC"
7. ec2.ec2mr-usg-cross-vpc-group-reference-rejected -> AssertionError: a rule on 'sg-...' (in 'vpc-00000000000000024') referencing 'sg-...' (in 'vpc-0000000000000002b') was accepted - real AWS refuses a cross-VPC group reference with InvalidGroup.NotFound
8. ec2.network-interface-attach-detach-lifecycle -> AssertionError: expected InvalidParameterValue deleting an attached ENI, got InvalidNetworkInterface.InUse
9. ec2.ec2mr-pip-eni-secondary-ip-set-round-trips -> AssertionError: the primary address entry should carry no PrivateDnsName in a VPC with DNS hostnames disabled; got {'Primary': True, 'PrivateDnsName': 'ip-10-61-1-4.us-west-2.compute.internal', 'PrivateIpAddress': '10.61.1.4'}
10. ec2.asgssh-default-nacl-delete-rejected -> AssertionError: deleting the default ACL 'acl-...' must be refused; got ('InvalidParameterValue', "The network acl 'acl-...' is the default network acl and cannot be deleted")

Model evidence: smithy_query(query="operation", operation="AuthorizeSecurityGroupIngress,AttachNetworkInterface,CreateNetworkAclEntry") on services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json returns "errors": [] for each (ec2Query declares refusals out-of-band), so the codes/messages above are the recorded AWS behavior.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite.
2. Under the `### ec2` heading, locate the ten checks listed in trace_ids; none of them carries an "ALREADY FILED" or "OWNED by" marker.
3. Each reproduces standalone, e.g.:
   - Create a VPC + security group; AuthorizeSecurityGroupIngress with IpPermissions=[{IpProtocol:'tcp',FromPort:4000,ToPort:4000,IpRanges:[{CidrIp:'172.25.0.0/23'}]}] twice. The second call must fail InvalidPermission.Duplicate with the rule rendered as `peer: 172.25.0.0/23, TCP, from port: 4000, to port: 4000, ALLOW`.
   - Authorize with IpProtocol='6', FromPort=80, ToPort=80, then DescribeSecurityGroups: IpProtocol must read back as 'tcp'.
   - DeleteSecurityGroup a group, then CreateNetworkInterface naming it in Groups: expect InvalidSecurityGroupID.NotFound.
   - Create an ENI in AZ b and an instance in AZ a, then AttachNetworkInterface: expect InvalidParameterCombination.
   - DeleteSecurityGroup on the VPC's default group: expect CannotDelete with the `cannot be deleted by a user` wording.
   - CreateSecurityGroup twice with the same GroupName in one VPC: the duplicate message must end `for VPC '<vpc-id>'`.
   - Authorize a rule whose UserIdGroupPairs names a group in a different VPC: expect InvalidGroup.NotFound.
   - Attach an ENI, then DeleteNetworkInterface: expect InvalidParameterValue.
   - In a VPC with enableDnsHostnames=false, DescribeNetworkInterfaces: PrivateIpAddresses entries must omit PrivateDnsName.
   - DeleteNetworkAcl on the VPC default ACL: expect AWS's default-ACL refusal, not InvalidParameterValue.
