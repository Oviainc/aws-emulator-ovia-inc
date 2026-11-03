Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.security-group-duplicate-ingress-rule-fails ec2.tgwc-duplicate-cache-port-ingress-rejected ec2.security-group-duplicate-name-concurrent ec2.security-group-name-notfound-error ec2.placement-group-and-public-ipv4-pool ec2.op-contract-reset-fpga-image-attribute ec2.tgwc-attachment-second-route-table-association-rejected ec2.tgwc-vpc-attachment-two-subnets-in-one-zone-rejected ec2.security-group-default-egress-and-rule-ids ec2.op-contract-modify-reserved-instances-rejected ec2.op-contract-modify-reserved-instances-required-params-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.security-group-duplicate-ingress-rule-fails|run#13|AssertionError|assert message == expected, f"unexpected message: {message!r}"
Issue-Key: ec2.tgwc-duplicate-cache-port-ingress-rejected|run#12|AssertionError|assert message == expected, f"duplicate-grant message was {message!r}, expected {expected!r}"
Issue-Key: ec2.security-group-duplicate-name-concurrent|run#24|AssertionError|assert message == expected, f"expected every loser's message to be {expected!r}, got {message!r}"
Issue-Key: ec2.security-group-name-notfound-error|run#6|AssertionError|assert message is not None and message.startswith(expected_prefix) and message.endswith("'"), message
Issue-Key: ec2.placement-group-and-public-ipv4-pool|run#14|AssertionError|assert dup_message == f"The placement group '{name}' already exists.", dup_message
Issue-Key: ec2.op-contract-reset-fpga-image-attribute|run#5|AssertionError|assert malformed_message == expected_message, f"expected message {expected_message!r}, got {malformed_message!r}"
Issue-Key: ec2.tgwc-attachment-second-route-table-association-rejected|run#15|AssertionError|assert message == expected_message, f"expected {expected_message!r}, got {message!r}"
Issue-Key: ec2.tgwc-vpc-attachment-two-subnets-in-one-zone-rejected|run#11|AssertionError|assert message == expected_message, f"the refusal's message was {message!r}, expected {expected_message!r}"
Issue-Key: ec2.security-group-default-egress-and-rule-ids|run#23|AssertionError|assert _rule_fields(implicit) == want_implicit, (
Issue-Key: ec2.op-contract-modify-reserved-instances-rejected|run#7|AssertionError|assert code == "MissingParameter", f"expected MissingParameter for a scope-less target configuration, got {code}"
Issue-Key: ec2.op-contract-modify-reserved-instances-required-params-rejected|run#5|AssertionError|assert code == "MissingParameter", f"expected MissingParameter, got {code}"
Issue-Commit: a2c0c19d1874b6d8f510c899cc1a51333f7d4b07

# EC2: refusal messages, one refusal code and one rule representation diverge from AWS across security groups, placement groups, transit gateways, FPGA images and reserved instances (9 net-new root causes)

> _Note: this issue was identified against the code base at commit `a2c0c19d1874b6d8f510c899cc1a51333f7d4b07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AuthorizeSecurityGroupIngress

## Description
Nine net-new root causes in the EC2 emulator. Every one is grounded in a failing behavioral check that the replay report marks neither ALREADY FILED nor OWNED. They are the same family as the open issues 20261029T175114Z-ec2-refusal-messages-are-emulator-authored-prose-instead.md and 20261024T081028Z-ec2-error-semantics-5-rejection-message-templates-diverge.md, which do not carry these checks' replay keys. A fixer must close all NINE causes before submitting. In every cause below, the status, the error code and the message text are the requirement; the ids, names and CIDRs shown are the variable parts filled from the request and from emulator state, never literal requirements.

Cause 1 of 9 — the duplicate-permission message names a rule id instead of the permission tuple.
AWS: Smithy: AuthorizeSecurityGroupIngress.IpPermissions (IpPermissionList); EC2 models no error shapes (ec2Query), so recorded behavior only. Re-authorizing an identical permission is refused with error code `InvalidPermission.Duplicate` and the message `the specified rule "peer: <cidr>, <PROTOCOL>, from port: <from>, to port: <to>, ALLOW" already exists`. Fixed text: `the specified rule "peer: `, `, `, `, from port: `, `, to port: `, `, ALLOW" already exists`. Variable parts, all from the request: the CIDR, the protocol name upper-cased, and the two ports. The existing rule set is unchanged, and the same port from a different CIDR is still accepted.
Emulator: the code is right but the message is `the specified rule "<sgr-id>" already exists` — it substitutes the stored SecurityGroupRuleId for the permission tuple.
Why: the duplicate message is the only place AWS tells a caller WHICH permission collided; clients and humans that parse or display it see an opaque internal id and cannot identify the offending rule from their own request.
Fix: the duplicate-permission refusal in the AuthorizeSecurityGroupIngress handler's message construction.
Traces: ec2.security-group-duplicate-ingress-rule-fails, ec2.tgwc-duplicate-cache-port-ingress-rejected.

Cause 2 of 9 — the duplicate-group-name message is truncated before the VPC id.
AWS: recorded behavior only (CreateSecurityGroup models no errors). A second CreateSecurityGroup with the same GroupName in the same VPC is refused with error code `InvalidGroup.Duplicate` and the message `The security group '<group-name>' already exists for VPC '<vpc-id>'`. Fixed text: `The security group '`, `' already exists for VPC '`, `'`. Variable parts: the group name from the request and the VPC id from the request/state. Under concurrency exactly one create wins and every loser gets that same message.
Emulator: the message stops short — `The security group '<group-name>' already exists for VPC` — with the quoted VPC id missing entirely.
Why: callers that disambiguate a name collision per VPC (the common multi-VPC provisioning case) get no VPC id, so the message cannot be acted on.
Fix: the duplicate-name refusal in the CreateSecurityGroup handler's message construction.
Traces: ec2.security-group-duplicate-name-concurrent.

Cause 3 of 9 — the group-name NotFound message omits the default VPC clause.
AWS: recorded behavior only. DescribeSecurityGroups(GroupNames=[absent-name]) is refused with error code `InvalidGroup.NotFound` and the message `The security group '<group-name>' does not exist in default VPC '<default-vpc-id>'`. Fixed text: `The security group '`, `' does not exist in default VPC '`, `'`. Variable parts: the requested name, and the account's default VPC id from state.
Emulator: `The security group '<group-name>' does not exist` — the `in default VPC '<id>'` clause is absent.
Why: a name lookup is resolved against the DEFAULT VPC, and the message is what tells the caller that. Dropping it hides why a group that exists elsewhere was not found.
Fix: the name-lookup not-found refusal in the DescribeSecurityGroups handler's message construction.
Traces: ec2.security-group-name-notfound-error.

Cause 4 of 9 — the duplicate placement-group message is missing its terminating period.
AWS: Smithy: CreatePlacementGroup.GroupName/Strategy; no modeled errors (ec2Query), so recorded behavior only. A repeated CreatePlacementGroup is refused with error code `InvalidPlacementGroup.Duplicate` and the message `The placement group '<group-name>' already exists.` — fixed text ending in a full stop, with the group name from the request as the only variable part.
Emulator: `The placement group '<group-name>' already exists` (no trailing `.`).
Why: clients that match the message exactly — a documented, stable AWS string — stop matching.
Fix: the duplicate refusal in the CreatePlacementGroup handler's message construction.
Traces: ec2.placement-group-and-public-ipv4-pool.

Cause 5 of 9 — ResetFpgaImageAttribute's malformed-id message is emulator-authored prose.
AWS: Smithy: ResetFpgaImageAttribute.FpgaImageId (FpgaImageId) required, Attribute enum; no modeled errors (ec2Query), so recorded behavior only. A malformed AFI id is refused with error code `InvalidFpgaImageID.Malformed` and the message `The fpga-image ID '<id>' is malformed` (fixed text around the id from the request); a well-formed but unassigned id is refused `InvalidFpgaImageID.NotFound` with `Image ID '<id>' not found.`
Emulator: the malformed leg returns `Image ID '<id>' is invalid.` — the wrong template, and one that reads like the NotFound family.
Why: the two refusal classes become indistinguishable by message, so a caller cannot tell a client-side formatting bug from a missing resource.
Fix: the id-format refusal in the ResetFpgaImageAttribute handler's message construction.
Traces: ec2.op-contract-reset-fpga-image-attribute.

Cause 6 of 9 — the already-associated transit-gateway message uses the wrong template.
AWS: Smithy: AssociateTransitGatewayRouteTable.TransitGatewayRouteTableId and .TransitGatewayAttachmentId required; no modeled errors (ec2Query), so recorded behavior only. Associating an attachment that is already associated with one route table is refused with error code `Resource.AlreadyAssociated` and the message `Transit Gateway Attachment <attachment-id> is already associated to a route table.` Fixed text: `Transit Gateway Attachment `, ` is already associated to a route table.`; the attachment id from the request is the only variable part — AWS does NOT name the existing route table. The first association stays in place and the refused table lists none.
Emulator: the code is right but the message is `The transit gateway attachment '<attachment-id>' is already associated with the transit gateway route table '<tgw-rtb-id>'` — different fixed text, quoted ids, an extra variable part and no terminating period.
Why: message-matching clients break, and the emulator's own wording implies AWS discloses the holding route table when it does not.
Fix: the already-associated refusal in the AssociateTransitGatewayRouteTable handler's message construction.
Traces: ec2.tgwc-attachment-second-route-table-association-rejected.

Cause 7 of 9 — the duplicate-subnets-in-one-AZ message is generated prose instead of AWS's fixed string.
AWS: Smithy: CreateTransitGatewayVpcAttachment.SubnetIds (TransitGatewaySubnetIdList) required; no modeled errors (ec2Query), so recorded behavior only. Naming two subnets in the same Availability Zone is refused with error code `DuplicateSubnetsInSameZone` and the message `Duplicate Subnets for same AZ` — entirely fixed text with NO variable part — and the request creates no attachment.
Emulator: `The subnets <subnet-a> and <subnet-b> are both in the Availability Zone <az>; you can specify only one subnet per Availability Zone` — a sentence the emulator composed, interpolating request values into a message AWS has no variables in.
Why: callers matching AWS's short fixed string never match, and the emulator teaches a message contract that does not exist.
Fix: the same-AZ refusal in the CreateTransitGatewayVpcAttachment handler's message construction.
Traces: ec2.tgwc-vpc-attachment-two-subnets-in-one-zone-rejected.

Cause 8 of 9 — the implicit all-protocol egress rule reads back without FromPort/ToPort.
AWS: Smithy: SecurityGroupRule.FromPort and .ToPort are Integer members (`If the protocol is ICMP or ICMPv6, this is the ICMP type or -1 (all ICMP types)`), IpProtocol `-1` means all protocols. Recorded behavior: the implicit allow-all IPv4 egress rule of a new security group reads back from DescribeSecurityGroupRules with `IpProtocol: '-1'`, `FromPort: -1` and `ToPort: -1`.
Emulator: the same rule reads back with FromPort and ToPort ABSENT (both None), everything else matching.
Why: clients that compare or round-trip rule tuples (revoke-by-tuple, drift detection, rule diffing) see a shape AWS never emits and either mis-compare or crash on the missing integers.
Fix: the SecurityGroupRule serializer / the default-egress rule stored by CreateSecurityGroup — emit -1 for both ports on an all-protocol rule.
Traces: ec2.security-group-default-egress-and-rule-ids.

Cause 9 of 9 — ModifyReservedInstances answers the wrong error CODE for missing parameters.
AWS: Smithy: ModifyReservedInstances.ReservedInstancesIds and .TargetConfigurations are both required; no modeled errors (ec2Query), so recorded behavior only. (a) A target configuration with neither AvailabilityZone nor Platform is refused with error code `MissingParameter`, message `The request must contain the parameter  AvailabilityZone` (fixed text, note the double space, no variable part), evaluated before the id is looked up. (b) A request with TargetConfigurations absent entirely is refused with error code `MissingParameter`, message `TargetReservedInstancesConfigurationSet`. A well-formed request for an unknown id is a DIFFERENT code, `InvalidInput`.
Emulator: leg (a) answers `InvalidInput` and leg (b) answers `MalformedInput` — in both cases the missing-parameter class is collapsed into the input-validation class, so a missing required field and a bogus resource id are no longer distinguishable by code.
Why: clients that branch on MissingParameter (retry after filling the field) versus InvalidInput (give up on the id) take the wrong branch.
Fix: request-structure validation in the ModifyReservedInstances handler — classify absent required members and absent scope axes as MissingParameter ahead of resource lookup.
Traces: ec2.op-contract-modify-reserved-instances-rejected, ec2.op-contract-modify-reserved-instances-required-params-rejected.

## Evidence
Cause 1: expected message 'the specified rule "peer: 198.51.100.0/32, TCP, from port: 443, to port: 443, ALLOW" already exists'; got 'the specified rule "sgr-00000000000000039" already exists'. Second instance: expected 'the specified rule "peer: 13.1.0.0/16, TCP, from port: 11211, to port: 11211, ALLOW" already exists'; got 'the specified rule "sgr-0000000000000002d" already exists'. Code InvalidPermission.Duplicate was correct in both.
Cause 2: expected "The security group '<name>' already exists for VPC '<vpc-id>'"; got "The security group '<name>' already exists for VPC".
Cause 3: expected a message starting "The security group '<name>' does not exist in default VPC 'vpc-" and ending "'"; got "The security group '<name>' does not exist".
Cause 4: expected "The placement group '<name>' already exists."; got "The placement group '<name>' already exists" (AssertionError body is the emulator's message).
Cause 5: expected "The fpga-image ID 'not-an-afi-id' is malformed"; got "Image ID 'not-an-afi-id' is invalid."
Cause 6: expected 'Transit Gateway Attachment <tgw-attach-id> is already associated to a route table.'; got "The transit gateway attachment '<tgw-attach-id>' is already associated with the transit gateway route table '<tgw-rtb-id>'".
Cause 7: expected 'Duplicate Subnets for same AZ'; got 'The subnets <subnet-a> and <subnet-b> are both in the Availability Zone us-west-2a; you can specify only one subnet per Availability Zone'.
Cause 8: implicit egress rule read back as {'IsEgress': True, 'IpProtocol': '-1', 'FromPort': None, 'ToPort': None, 'CidrIpv4': '0.0.0.0/0', ...}; expected the same with 'FromPort': -1, 'ToPort': -1.
Cause 9: expected MissingParameter for a scope-less target configuration, got InvalidInput; expected MissingParameter for an absent TargetConfigurations, got MalformedInput.

## Steps to Reproduce
Replay the behavioral suite checks listed in trace_ids against the emulator (service heading `ec2`).
Cause 1: create a VPC and security group, AuthorizeSecurityGroupIngress with tcp/443 from 198.51.100.0/32, then repeat the identical call; read Error.Message.
Cause 2: create a VPC, then issue several concurrent CreateSecurityGroup calls with the same GroupName and VpcId; read the losers' Error.Message.
Cause 3: DescribeSecurityGroups(GroupNames=['<name-that-does-not-exist>']); read Error.Message.
Cause 4: CreatePlacementGroup(GroupName=<n>, Strategy='cluster') twice; read Error.Message.
Cause 5: ResetFpgaImageAttribute(FpgaImageId='not-an-afi-id', Attribute='loadPermission'); read Error.Message.
Cause 6: create a TGW, two TGW route tables and a VPC attachment; AssociateTransitGatewayRouteTable to the first table, wait for 'associated', then associate the same attachment to the second table; read Error.Message.
Cause 7: create a TGW, a VPC and two subnets in the SAME AZ; CreateTransitGatewayVpcAttachment with both subnet ids; read Error.Message.
Cause 8: CreateSecurityGroup in a fresh VPC, then DescribeSecurityGroupRules filtered by group-id and inspect the IsEgress rule with IpProtocol '-1' for FromPort/ToPort.
Cause 9: ModifyReservedInstances(ReservedInstancesIds=['<uuid>'], TargetConfigurations=[{'InstanceCount':1,'InstanceType':'t3.micro'}]) (no scope axis), and — with client-side validation disabled — ModifyReservedInstances(ReservedInstancesIds=['<uuid>']) with no TargetConfigurations; read Error.Code in both.
