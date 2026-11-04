Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.ec2mr-inv-describe-instances-rejects-an-unknown-filter-name-instead-of-ignoring-it ec2.ec2mr-inv-describe-instances-rejects-max-results-combined-with-instance-ids ec2.describe-network-interfaces-contract ec2.ec2mr-inv2-authorize-ingress-rejects-a-cidr-with-an-out-of-range-prefix-length ec2.ec2mr-inv2-authorize-ingress-rejects-a-duplicate-ssh-rule-and-leaves-the-rule-count-unchanged ec2.ec2mr-inv2-authorize-ingress-rejects-an-inverted-port-range ec2.op-contract-create-security-group ec2.op-contract-stop-instances ec2.op-contract-get-password-data ec2.op-contract-start-instances ec2.op-contract-describe-vpc-endpoints ec2.op-contract-delete-network-insights-path ec2.copy-fpga-image-invalid-source-region-rejected ec2.create-network-interface ec2.eip-reassociation-blocked-without-allow-reassociation ec2.tgwc-vpc-route-to-nonexistent-transit-gateway-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.ec2mr-inv-describe-instances-rejects-an-unknown-filter-name-instead-of-ignoring-it|run#10|AssertionError|assert got == ("InvalidParameterValue", "The filter 'instance-state' is invalid"), (
Issue-Key: ec2.ec2mr-inv-describe-instances-rejects-max-results-combined-with-instance-ids|run#13|AssertionError|assert combo == expected, (
Issue-Key: ec2.describe-network-interfaces-contract|run#18|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: ec2.ec2mr-inv2-authorize-ingress-rejects-a-cidr-with-an-out-of-range-prefix-length|run#7|AssertionError|assert got == ("InvalidParameterValue", "CIDR block 0.0.0.0/33 is malformed"), (
Issue-Key: ec2.ec2mr-inv2-authorize-ingress-rejects-a-duplicate-ssh-rule-and-leaves-the-rule-count-unchanged|run#11|AssertionError|assert got == expected, (
Issue-Key: ec2.ec2mr-inv2-authorize-ingress-rejects-an-inverted-port-range|run#12|AssertionError|assert got == expected, (
Issue-Key: ec2.op-contract-create-security-group|run#17|AssertionError|assert message == f"The security group '{name}' already exists for VPC '{vpc_id}'", message
Issue-Key: ec2.op-contract-stop-instances|run#10|AssertionError|assert bogus_message == f"The instance ID '{candidate}' is malformed", bogus_message
Issue-Key: ec2.op-contract-get-password-data|run#13|AssertionError|assert message == f"The instance ID '{bogus_id}' is malformed", message
Issue-Key: ec2.op-contract-start-instances|run#14|AssertionError|assert message == (
Issue-Key: ec2.op-contract-describe-vpc-endpoints|run#15|AssertionError|assert message == "The Vpc Endpoint Id 'vpce-00000000000000000' does not exist", (
Issue-Key: ec2|DescribeNetworkInsightsPaths|InvalidNetworkInsightsPathId.NotFound
Issue-Key: ec2.copy-fpga-image-invalid-source-region-rejected|run#3|AssertionError|assert region_message == f"Region name '{bogus_region}' is invalid.", f"unexpected error message {region_message!r}"
Issue-Key: ec2.create-network-interface|run#18|AssertionError|assert message == ("The specified subnet does not have enough free addresses to satisfy the "
Issue-Key: ec2.eip-reassociation-blocked-without-allow-reassociation|run#18|AssertionError|assert message == (f"resource {alloc_id} is already associated with associate-id "
Issue-Key: ec2.tgwc-vpc-route-to-nonexistent-transit-gateway-rejected|run#8|AssertionError|assert got == expected, f"a route to {_GHOST_TGW} answered {got!r}"
Issue-Commit: c832bd0d619782e17fabc455ea6e3058ac5fd237

# Ten EC2 refusal messages are emulator-authored prose instead of AWS's message templates

> _Note: this issue was identified against the code base at commit `c832bd0d619782e17fabc455ea6e3058ac5fd237`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AuthorizeSecurityGroupIngress

## Description
This issue bundles 10 net-new root causes in the EC2 emulator. In each the status and (except where noted) the code are right, but the message text diverges from AWS's template. A message is part of the contract: operators read it, and clients parse the member names and limits out of it. Smithy: the ec2 model binds awsQuery with no modeled errors, so the templates below are recorded behavior only; the member names AWS quotes are the WIRE names the model declares (e.g. DescribeNetworkInterfaces' list member xmlName=NetworkInterfaceId, DescribeInstances' instancesSet/maxResults).

Cause 1 of 10: DescribeInstances with an unknown filter name.
AWS: InvalidParameterValue, message "The filter '<name>' is invalid" — fixed text with the filter name from the request.
Emulator: "The filter 'instance-state' is not one this emulator resolves" — prose that names the emulator.
Why: the message leaks the implementation and does not match the text operators and parsers expect.
Fix: the DescribeInstances filter validator's message template.
Traces: ec2.ec2mr-inv-describe-instances-rejects-an-unknown-filter-name-instead-of-ignoring-it.

Cause 2 of 10: DescribeInstances with InstanceIds and MaxResults together.
AWS: InvalidParameterCombination, "The parameter instancesSet cannot be used with the parameter maxResults" — the WIRE member names, and the combination error outranks the MaxResults floor check.
Emulator: "The parameter InstanceIds cannot be used with the parameter MaxResults" — SDK-shaped names.
Why: a caller that matches on the wire member names (as the recording shows them) does not recognise the message.
Fix: the DescribeInstances parameter-combination validator's message template.
Traces: ec2.ec2mr-inv-describe-instances-rejects-max-results-combined-with-instance-ids.

Cause 3 of 10: DescribeNetworkInterfaces with NetworkInterfaceIds and MaxResults together.
AWS: "The parameter NetworkInterfaceIds cannot be used with the parameter MaxResults" — the offending parameter first, plural form.
Emulator: "The parameter MaxResults cannot be used with the parameter NetworkInterfaceId" — operands reversed and singular.
Why: the message blames the wrong parameter, so the caller removes MaxResults rather than the id list.
Fix: the DescribeNetworkInterfaces combination validator's message template.
Traces: ec2.describe-network-interfaces-contract.

Cause 4 of 10: AuthorizeSecurityGroupIngress with an out-of-range IPv4 prefix length.
AWS: InvalidParameterValue, "CIDR block <cidr> is malformed" — the CIDR unquoted, no permission index.
Emulator: "CIDR block '0.0.0.0/33' of ipPermissions.1 is malformed".
Why: text differs from the template, so message-matching validation fails.
Fix: the security-group CIDR validator's message template.
Traces: ec2.ec2mr-inv2-authorize-ingress-rejects-a-cidr-with-an-out-of-range-prefix-length.

Cause 5 of 10: AuthorizeSecurityGroupIngress with a duplicate rule bundled with a new one.
AWS: InvalidPermission.Duplicate, 'the specified rule "peer: <cidr>, <PROTOCOL>, from port: <n>, to port: <n>, ALLOW" already exists' — the rule is described by its peer/protocol/ports, taken from the request.
Emulator: 'the specified rule "sgr-00000000000000046" already exists' — it names the internal rule id instead.
Why: the operator cannot see WHICH rule collided, and the id is meaningless to the caller that sent CIDRs and ports.
Fix: the duplicate-rule refusal in the authorize handler — render the rule description.
Traces: ec2.ec2mr-inv2-authorize-ingress-rejects-a-duplicate-ssh-rule-and-leaves-the-rule-count-unchanged.

Cause 6 of 10: AuthorizeSecurityGroupIngress with an inverted port range.
AWS: InvalidParameterValue, "Invalid TCP/UDP port range(<from>:<to>)".
Emulator: InvalidPermission.Malformed, "ipPermissions.1 ends at 21, before it starts at 22" — wrong code AND wrong message.
Why: a client branching on InvalidParameterValue for input repair never sees it.
Fix: the port-range validator — code and message template.
Traces: ec2.ec2mr-inv2-authorize-ingress-rejects-an-inverted-port-range.

Cause 7 of 10: CreateSecurityGroup with a duplicate name in a VPC.
AWS: InvalidGroup.Duplicate, "The security group '<name>' already exists for VPC '<vpc-id>'" — both values from the request/state.
Emulator: the message ends after "for VPC", with the vpc id missing entirely.
Why: the operator is not told which VPC owns the colliding group.
Fix: the CreateSecurityGroup duplicate-name refusal — interpolate the VPC id.
Traces: ec2.op-contract-create-security-group.

Cause 8 of 10: malformed instance ids everywhere.
AWS: "The instance ID '<id>' is malformed" — fixed text, id from the request, single quotes.
Emulator: 'Invalid id: "i-10000000000000001"' / 'Invalid id: "i-fffffffffffffffff"' — a generic internal template with double quotes.
Why: every caller and operator reading a malformed-id failure sees text AWS never produces, in two operations at least (StopInstances, GetPasswordData).
Fix: the shared EC2 id-validation message template.
Traces: ec2.op-contract-stop-instances, ec2.op-contract-get-password-data.

Cause 9 of 10: StartInstances on a terminated instance.
AWS: IncorrectInstanceState with AWS's wording for the start-from-terminated case, naming the instance id and its state.
Emulator: "The instance 'i-...' is in state 'terminated', which is not a state from which it can be started" — emulator-authored phrasing.
Why: the code is right but the text is not AWS's, so message assertions and operator runbooks diverge.
Fix: the StartInstances state-check message template.
Traces: ec2.op-contract-start-instances.

Cause 10 of 10: not-found and parameter messages reworded across six more read/validate paths.
AWS templates (recorded): DescribeVpcEndpoints -> "The Vpc Endpoint Id '<id>' does not exist"; DeleteNetworkInsightsPath -> "<nip-id> does not exist"; CopyFpgaImage with a bad SourceRegion -> "Region name '<region>' is invalid."; CreateNetworkInterface in a full subnet -> "The specified subnet does not have enough free addresses to satisfy the request"; AssociateAddress on an address already associated without AllowReassociation -> "resource <eipalloc-id> is already associated with associate-id <eipassoc-id>"; CreateRoute to a nonexistent transit gateway -> InvalidTransitGatewayID.NotFound "The transitGateway ID '<tgw-id>' does not exist.". In each the fixed text is the template and the id/region/value is taken from the request or state.
Emulator: "The VPC endpoint ID '<id>' does not exist"; "The network insights path ID '<id>' does not exist"; "Value (eu-fake-9) for parameter sourceRegion is invalid. This is not a valid Region name."; "The subnet does not have enough free private IPv4 addresses to satisfy the request"; "The address with allocation id '<id>' is already associated with '<instance-id>', and allowReassociation is false"; "Transit Gateway <tgw-id> was deleted or does not exist.".
Why: six more refusals whose text no AWS-trained client or operator recognises; two of them also drop the identifier AWS reports (the associate-id).
Fix: the message templates in those operations' not-found/validation paths.
Traces: ec2.op-contract-describe-vpc-endpoints, ec2.op-contract-delete-network-insights-path, ec2.copy-fpga-image-invalid-source-region-rejected, ec2.create-network-interface, ec2.eip-reassociation-blocked-without-allow-reassociation, ec2.tgwc-vpc-route-to-nonexistent-transit-gateway-rejected.

## Evidence
Replay output, expected vs actual (fixed text vs request/state values are identified in each cause above):
C1: got ("InvalidParameterValue", "The filter 'instance-state' is not one this emulator resolves"), expected ("InvalidParameterValue", "The filter 'instance-state' is invalid").
C2: got ("InvalidParameterCombination", "The parameter InstanceIds cannot be used with the parameter MaxResults"), expected ("InvalidParameterCombination", "The parameter instancesSet cannot be used with the parameter maxResults").
C3: expected "The parameter NetworkInterfaceIds cannot be used with the parameter MaxResults", got "The parameter MaxResults cannot be used with the parameter NetworkInterfaceId".
C4: got "CIDR block '0.0.0.0/33' of ipPermissions.1 is malformed", expected "CIDR block 0.0.0.0/33 is malformed".
C5: got ('InvalidPermission.Duplicate', 'the specified rule "sgr-00000000000000046" already exists'), expected ('InvalidPermission.Duplicate', 'the specified rule "peer: 198.51.100.0/24, TCP, from port: 22, to port: 22, ALLOW" already exists').
C6: got ('InvalidPermission.Malformed', 'ipPermissions.1 ends at 21, before it starts at 22'), expected ('InvalidParameterValue', 'Invalid TCP/UDP port range(22:21)').
C7: got "The security group 'sim2real-sg-...' already exists for VPC" (vpc id absent).
C8: got 'Invalid id: "i-10000000000000001"' and 'Invalid id: "i-fffffffffffffffff"', expected "The instance ID '<id>' is malformed".
C9: got "The instance 'i-...' is in state 'terminated', which is not a state from which it can be started".
C10: got "The VPC endpoint ID 'vpce-...' does not exist" / "The network insights path ID 'nip-...' does not exist" / "Value (eu-fake-9) for parameter sourceRegion is invalid. This is not a valid Region name." / "The subnet does not have enough free private IPv4 addresses to satisfy the request" / "The address with allocation id 'eipalloc-...' is already associated with 'i-...', and allowReassociation is false" / "Transit Gateway tgw-0123456789abcdef0 was deleted or does not exist."

## Steps to Reproduce
Replay the checks in trace_ids and read Error.Message on each refusal. Minimal reproductions: DescribeInstances(Filters=[{'Name':'instance-state','Values':['running']}]); DescribeInstances(InstanceIds=[...], MaxResults=4); DescribeNetworkInterfaces(NetworkInterfaceIds=[...], MaxResults=5); AuthorizeSecurityGroupIngress with 0.0.0.0/33, with a duplicate tcp/22 rule bundled with tcp/2222, and with FromPort=22/ToPort=21; CreateSecurityGroup twice with one name in one VPC; StopInstances(InstanceIds=['i-10000000000000001']); GetPasswordData(InstanceId='i-fffffffffffffffff'); StartInstances on a terminated instance; DescribeVpcEndpoints(VpcEndpointIds=['vpce-00000000000000000']); DeleteNetworkInsightsPath on an unknown nip-; CopyFpgaImage(SourceRegion='eu-fake-9'); CreateNetworkInterface in a /28 with no free addresses; AssociateAddress onto an already-associated address without AllowReassociation; CreateRoute(TransitGatewayId='tgw-0123456789abcdef0').
