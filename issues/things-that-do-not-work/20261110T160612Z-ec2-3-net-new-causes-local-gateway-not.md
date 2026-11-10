Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.create-local-gateway-route-table-rejected-unknown-gateway ec2.ec2-delete-vpc-removes-own-and-default-resource-tags ec2.e2e-ec2mr-usg-unused-security-groups-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.create-local-gateway-route-table-rejected-unknown-gateway|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: ec2|CreateTags|InvalidNetworkAclID.NotFound
Issue-Key: ec2.e2e-ec2mr-usg-unused-security-groups-lifecycle|phase#5|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# ec2: 3 net-new causes — local-gateway not-found wording, CreateTags refuses a VPC's own default network ACL, and DescribeNetworkInterfaces omits the interface's security groups

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTags

## Description
Three independent EC2 root causes, each with its own failing check.

Cause 1 of 3: the CreateLocalGatewayRouteTable not-found message is reworded.
AWS:      Smithy: CreateLocalGatewayRouteTable takes a required LocalGatewayId and declares no modelled errors (ec2Query carries errors untyped), so code and wording are recorded behavior: an id that does not exist is refused with HTTP 400, error code `InvalidLocalGatewayID.NotFound` and message `The LocalGateway ID '<LocalGatewayId>' does not exist`. The fixed text is `The LocalGateway ID '` … `' does not exist`; the only variable part is the id taken from the request.
Emulator: the code is right but the message reads `The local-gateway ID '<id>' does not exist` — the resource noun is rendered as the hyphenated lowercase resource-type token instead of AWS's `LocalGateway`.
Why:      refusal messages are matched by operators, runbooks and test suites; a reworded sentence breaks every consumer that pattern-matches AWS's text, and the hyphenated form suggests a generic resource-type-driven message builder that will mis-word other local-gateway refusals too.
Fix:      the CreateLocalGatewayRouteTable handler's not-found refusal (the message template that formats the resource noun for the local-gateway id class).
Traces:   ec2.create-local-gateway-route-table-rejected-unknown-gateway.

Cause 2 of 3: CreateTags refuses a network-ACL id that EC2 itself just returned.
AWS:      Smithy: CreateTags takes Resources (ResourceIdList) and Tags and returns an empty body; it declares no modelled errors, so the refusal class is recorded behavior — a resource id that exists is tagged and the call answers HTTP 200 with no body, and `InvalidNetworkAclID.NotFound` ("The networkAcl ID '<id>' does not exist") is reserved for an ACL that really does not exist. A VPC's default network ACL — the one DescribeNetworkAcls reports with IsDefault=true for that VPC — is an ordinary taggable resource.
Emulator: CreateTags over [vpc-id, default security-group id, main route-table id, default network-acl id, security-group-rule id] is refused with HTTP 400 `InvalidNetworkAclID.NotFound` and message `The networkAcl ID 'acl-<id>' does not exist`, using the very id DescribeNetworkAcls(Filters=[vpc-id]) returned one call earlier as IsDefault=true. The id is per-run; the defect is that an existing default ACL is unknown to the tagging path.
Why:      tagging is how every estate is labelled, costed and governed. A default ACL that cannot be tagged breaks tag-based policy and cost allocation, and because CreateTags is all-or-nothing the refusal also drops the tags for the other four resources in the same request.
Fix:      the CreateTags handler's resource-id resolution for the network-acl id class (and the state that registers a VPC's default ACL as a taggable resource).
Traces:   ec2.ec2-delete-vpc-removes-own-and-default-resource-tags.

Cause 3 of 3: DescribeNetworkInterfaces does not report the security groups an interface was created with.
AWS:      Smithy: NetworkInterface.Groups — "Any security groups for the network interface" (a GroupIdentifier list of GroupId/GroupName). An interface created with a group set reports each of those groups in DescribeNetworkInterfaces, which is what makes a security group "ENI-attached"; a derivation of unattached groups therefore excludes exactly the groups some interface names.
Emulator: no interface returned by DescribeNetworkInterfaces names the stack's attached group, so that group is counted as unattached: the derivation lists every group in the VPC, including the one the interface is created with, as ENI-unattached. The specific ids are per-run; the defect is that the interface's Groups member does not carry the groups it was created with.
Why:      "which security groups are actually in use" is derived from ENI membership. With Groups empty or missing, every unused-security-group audit reports in-use groups as unused and a cleanup driven by it deletes groups that are attached, taking down the traffic they admit.
Fix:      the DescribeNetworkInterfaces serializer / the network-interface state that should record and emit the interface's security-group set (including interfaces created by CloudFormation AWS::EC2::NetworkInterface with a GroupSet).
Traces:   ec2.e2e-ec2mr-usg-unused-security-groups-lifecycle (fails in phase 2/5 pre_invoke).

## Evidence
Cause 1 — Expected: code `InvalidLocalGatewayID.NotFound`, message `The LocalGateway ID 'lgw-…' does not exist`. Actual: `AssertionError: expected message "The LocalGateway ID 'lgw-00000000000000000' does not exist", got "The local-gateway ID 'lgw-00000000000000000' does not exist"` (the id is the one the request supplied; only the noun diverges).

Cause 2 — Expected: CreateTags(Resources=[vpc, default sg, main rtb, default acl, sg-rule], Tags=[{Name: …}]) answers HTTP 200 with an empty body. Actual: `botocore.exceptions.ClientError: An error occurred (InvalidNetworkAclID.NotFound) when calling the CreateTags operation: The networkAcl ID 'acl-00000000000000083' does not exist` — for the acl id DescribeNetworkAcls had just returned with IsDefault=true for that VPC.

Cause 3 — Expected: the ENI-attached group is the only group excluded from the unattached set. Actual: `AssertionError: [phase 2/5 pre_invoke] the hook's predicate calls ['sg-…086', 'sg-…099', 'sg-…09b', 'sg-…09d', 'sg-…09f', 'sg-…0a1'] unattached inside 'vpc-…080', expected ['sg-…086', 'sg-…09b', 'sg-…09d', 'sg-…09f', 'sg-…0a1'] — the ENI-attached group 'sg-…099' must be the only one excluded`. The derivation is `attached = {g["GroupId"] for eni in DescribeNetworkInterfaces for g in eni.get("Groups", [])}`, which came back without the attached group.

## Steps to Reproduce
Cause 1: call CreateLocalGatewayRouteTable(LocalGatewayId='lgw-00000000000000000') and compare the error code and message with AWS's `InvalidLocalGatewayID.NotFound` / `The LocalGateway ID 'lgw-00000000000000000' does not exist`.

Cause 2: CreateVpc; wait for state 'available'; read the VPC's default security group (DescribeSecurityGroups Filters vpc-id, GroupName='default'), its main route table (DescribeRouteTables, Associations[].Main) and its default network ACL (DescribeNetworkAcls Filters vpc-id, IsDefault=true); then CreateTags(Resources=[all four ids + a security-group-rule id], Tags=[{Key:'Name',Value:…}]) — the call is refused with InvalidNetworkAclID.NotFound naming the ACL just read.

Cause 3: create a VPC with several security groups and a network interface created with one of them (CreateNetworkInterface(Groups=[sg]) or a CloudFormation AWS::EC2::NetworkInterface with GroupSet); then page DescribeNetworkInterfaces and collect `eni["Groups"][*]["GroupId"]` — the group the interface was created with is missing, so subtracting it from the VPC's groups still leaves it in the 'unattached' set.
