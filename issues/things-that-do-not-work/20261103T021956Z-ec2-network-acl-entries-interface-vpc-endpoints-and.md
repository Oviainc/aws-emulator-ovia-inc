Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.asgssh-nacl-entries-round-trip-deny-22-before-allow-all ec2.asgssh-nacl-protocol-keyword-instead-of-number-rejected ec2.asgssh-nacl-tcp-entry-without-port-range-rejected ec2.asvpce-streaming-vpce-round-trips-interface-shape ec2.asvpce-second-private-dns-streaming-vpce-in-vpc-rejected ec2.ec2-interface-endpoint-private-dns-rejected-without-service-private-dns-name ec2.ec2-iot-data-endpoint-service-declares-no-policy-support-or-private-dns ec2.asgssh-launch-template-without-key-name-omits-it-from-version-data ec2.create-vpn-gateway-empty-availability-zone-creates-gateway ec2.modify-vpc-attribute-conflicting-attrs-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.asgssh-nacl-entries-round-trip-deny-22-before-allow-all|run#18|AssertionError|assert ranged == [(50, False)], (
Issue-Key: ec2.asgssh-nacl-protocol-keyword-instead-of-number-rejected|run#6|AssertionError|assert got == ("InvalidParameterValue", "Invalid value 'tcp' for IP protocol. Unknown protocol."), (
Issue-Key: ec2.asgssh-nacl-tcp-entry-without-port-range-rejected|run#6|AssertionError|assert got == ("MissingParameter", "The request must contain the parameter portRange.from"), (
Issue-Key: ec2.asvpce-streaming-vpce-round-trips-interface-shape|run#13|AssertionError|assert got["PrivateDnsEnabled"] is True, (
Issue-Key: ec2.asvpce-second-private-dns-streaming-vpce-in-vpc-rejected|_error#6|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error")
Issue-Key: ec2.ec2-interface-endpoint-private-dns-rejected-without-service-private-dns-name|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: ec2.ec2-iot-data-endpoint-service-declares-no-policy-support-or-private-dns|run#7|AssertionError|assert subject["VpcEndpointPolicySupported"] is False, (
Issue-Key: ec2.asgssh-launch-template-without-key-name-omits-it-from-version-data|run#17|AssertionError|assert data[key] == want, f"{key} should round-trip as {want!r}; got {data[key]!r}"
Issue-Key: ec2.create-vpn-gateway-empty-availability-zone-creates-gateway|run#11|AssertionError|assert (code, shaped) == want, (
Issue-Key: ec2.modify-vpc-attribute-conflicting-attrs-rejected|run#15|AssertionError|assert message.startswith("Fields for multiple attribute types specified: "), message
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# EC2 network ACL entries, interface VPC endpoints and VPC attributes: missing defaults, accepted invalid requests and wrong refusal classes (10 root causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
Ten net-new root causes in the EC2 network-ACL / VPC-endpoint / VPC-attribute surface. The EC2 Smithy model is ec2Query: smithy_query(query="operation", operation="CreateVpcEndpoint,CreateNetworkAclEntry") shows CreateVpcEndpoint.PrivateDnsEnabled is an optional Boolean with NO modelled default and "errors": [], and CreateNetworkAclEntry requires Protocol as a String with PortRange optional and "errors": []. Every code/message below is therefore the recorded AWS behavior.

Cause 1 of 10: an all-traffic network-ACL entry is stored with a PortRange.
AWS: recorded behavior only. A NACL entry with Protocol="-1" (all traffic) carries NO PortRange; DescribeNetworkAcls omits the member. Only protocol-specific entries (6/17) carry one.
Emulator: every entry reads back with a PortRange - the replay shows ranges on rule numbers 50, 100, 32767 in both directions, where only the tcp rule may have one.
Why: a client that reads NACL entries back to render or diff a rule set sees a port restriction on an all-traffic rule, i.e. a rule that means something different from what it created.
Fix: the CreateNetworkAclEntry handler's state write / the DescribeNetworkAcls serializer must not synthesise a PortRange for protocol -1.
Traces: ec2.asgssh-nacl-entries-round-trip-deny-22-before-allow-all

Cause 2 of 10: CreateNetworkAclEntry refuses a protocol keyword with the wrong message template.
AWS: recorded behavior only. Protocol is a numeric string; the keyword "tcp" is refused with AWS's own protocol-validation wording, not the emulator's range sentence.
Emulator: answers ('InvalidParameterValue', 'Value (tcp) for parameter protocol is invalid. The protocol number must be between 0 and 255, or -1 for all protocols') - emulator-authored prose.
Why: callers that match AWS's message to tell "wrong spelling" from "out of range" mis-handle the refusal.
Fix: the Protocol validation in the CreateNetworkAclEntry handler.
Traces: ec2.asgssh-nacl-protocol-keyword-instead-of-number-rejected

Cause 3 of 10: a tcp NACL entry without PortRange is refused with the wrong message template.
AWS: recorded behavior only. Protocol=6 with no PortRange is refused as a missing parameter using AWS's own wording naming the member.
Emulator: answers ('MissingParameter', 'The request must contain the parameter portRange for protocol 6') - emulator prose.
Why: the message is the contract for which member to add; a divergent template breaks message-driven remediation.
Fix: the required-member validation in the CreateNetworkAclEntry handler.
Traces: ec2.asgssh-nacl-tcp-entry-without-port-range-rejected

Cause 4 of 10: CreateVpcEndpoint does not default PrivateDnsEnabled to true for an AWS service.
AWS: an omitted PrivateDnsEnabled defaults to TRUE for an AWS-service interface endpoint; the created endpoint reads back PrivateDnsEnabled=true. Smithy: CreateVpcEndpoint.PrivateDnsEnabled is Boolean with no modelled default, so the default is service-side behavior (recorded).
Emulator: stores and reports PrivateDnsEnabled=false when the member is omitted.
Why: private DNS is what makes the endpoint intercept the service's public hostname. A client that relies on the default gets an endpoint that resolves nothing, and any test of in-VPC service addressing passes locally for the wrong reason.
Fix: default application in the CreateVpcEndpoint handler (and the VpcEndpoint serializer).
Traces: ec2.asvpce-streaming-vpce-round-trips-interface-shape

Cause 5 of 10: a second private-DNS interface endpoint for the same service in one VPC is accepted.
AWS: recorded behavior only. Only one interface endpoint per (VPC, service) may have PrivateDnsEnabled=true; a second is refused.
Emulator: accepts it with HTTP 200 - the check reports the call SUCCEEDED where AWS has no success path.
Why: two endpoints claiming the same private DNS name is an unreachable configuration on AWS; the emulator lets it be provisioned, so the failure surfaces only in production.
Fix: the per-VPC/per-service private-DNS uniqueness check in the CreateVpcEndpoint handler's validation.
Traces: ec2.asvpce-second-private-dns-streaming-vpce-in-vpc-rejected

Cause 6 of 10: PrivateDnsEnabled is accepted for a service that has no private DNS name.
AWS: recorded behavior only. Requesting PrivateDnsEnabled=true for a service whose service detail carries no PrivateDnsName is refused.
Emulator: accepts it with HTTP 200.
Why: same as above - an endpoint that can never resolve privately is created silently.
Fix: cross-check the service's PrivateDnsName in the CreateVpcEndpoint handler before honouring PrivateDnsEnabled.
Traces: ec2.ec2-interface-endpoint-private-dns-rejected-without-service-private-dns-name

Cause 7 of 10: DescribeVpcEndpointServices reports VpcEndpointPolicySupported=true for services that do not support endpoint policies.
AWS: recorded behavior only. The IoT data-plane service (`com.amazonaws.<region>.iot.data`) reports VpcEndpointPolicySupported=false, which is why a narrowing endpoint policy cannot be written for it.
Emulator: reports VpcEndpointPolicySupported=true for that service.
Why: a client inspects this flag to decide whether to attach an endpoint policy at all; a wrong `true` makes it attempt an operation AWS refuses.
Fix: the service-catalogue data / the ServiceDetail serializer behind DescribeVpcEndpointServices.
Traces: ec2.ec2-iot-data-endpoint-service-declares-no-policy-support-or-private-dns

Cause 8 of 10: launch-template MetadataOptions round-trip adds a State member AWS does not return there.
AWS: recorded behavior only. A launch-template version's LaunchTemplateData.MetadataOptions round-trips exactly the members that were set (HttpTokens, HttpPutResponseHopLimit, HttpEndpoint); `State` belongs to the instance's metadata-options view, not the template's.
Emulator: returns an extra `'State': 'applied'` inside the template's MetadataOptions.
Why: clients that compare the template data they submitted with the version read back see a spurious drift and re-create versions endlessly.
Fix: the LaunchTemplateData serializer for CreateLaunchTemplate / DescribeLaunchTemplateVersions.
Traces: ec2.asgssh-launch-template-without-key-name-omits-it-from-version-data

Cause 9 of 10: CreateVpnGateway's bad-AvailabilityZone refusal uses emulator prose.
AWS: recorded behavior only. An AvailabilityZone that cannot host a VPN gateway is refused ('InvalidParameterValue', 'Value (<az>) for parameter availabilityZone is invalid. VpnConnection gateways can currently only be created in the following availability zones: <zones>.') - fixed text with the requested AZ and the permitted zone list as variables.
Emulator: answers ('InvalidParameterValue', "Invalid value '<az>' for availabilityZone. Region '<region>' has no such Availability Zone"), a different sentence that also asserts the wrong reason (unknown zone rather than unsupported zone).
Why: the message is the only place the permitted zone list is published; a caller cannot recover by picking a supported zone.
Fix: the AvailabilityZone validation in the CreateVpnGateway handler.
Traces: ec2.create-vpn-gateway-empty-availability-zone-creates-gateway

Cause 10 of 10: ModifyVpcAttribute's conflicting-attribute refusal diverges from AWS's template.
AWS: recorded behavior only. Setting enableDnsSupport and enableDnsHostnames in one ModifyVpcAttribute call is refused with AWS's own single-attribute-per-request wording.
Emulator: answers with the emulator sentence `You cannot modify enableDnsSupport and enableDnsHostnames in the same request. Use separate requests for each attribute.`
Why: message-keyed remediation does not fire; the status/code pair is also what clients branch on when splitting the request.
Fix: the one-attribute-per-request validation in the ModifyVpcAttribute handler.
Traces: ec2.modify-vpc-attribute-conflicting-attrs-rejected

## Evidence
Replayed failing checks under `### ec2` (none marked ALREADY FILED / OWNED by):

1. ec2.asgssh-nacl-entries-round-trip-deny-22-before-allow-all -> AssertionError: only the tcp rule carries a PortRange; an all-traffic rule must OMIT it, got ranges on [(50, False), (100, False), (100, True), (32767, False), (32767, True)]
2. ec2.asgssh-nacl-protocol-keyword-instead-of-number-rejected -> AssertionError: protocol keyword 'tcp' must be refused by CreateNetworkAclEntry; got ('InvalidParameterValue', 'Value (tcp) for parameter protocol is invalid. The protocol number must be between 0 and 255, or -1 for all protocols')
3. ec2.asgssh-nacl-tcp-entry-without-port-range-rejected -> AssertionError: a tcp entry with no PortRange must be refused as a missing parameter; got ('MissingParameter', 'The request must contain the parameter portRange for protocol 6')
4. ec2.asvpce-streaming-vpce-round-trips-interface-shape -> AssertionError: an omitted PrivateDnsEnabled must default to true for an AWS service; got False
5. ec2.asvpce-second-private-dns-streaming-vpce-in-vpc-rejected -> AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error
6. ec2.ec2-interface-endpoint-private-dns-rejected-without-service-private-dns-name -> AssertionError: the call succeeded; real AWS rejects it, so there is no error
7. ec2.ec2-iot-data-endpoint-service-declares-no-policy-support-or-private-dns -> AssertionError: com.amazonaws.us-west-2.iot.data reports VpcEndpointPolicySupported=True; real AWS reports False, which is why an IoT data-plane restriction cannot be written as an endpoint policy
8. ec2.asgssh-launch-template-without-key-name-omits-it-from-version-data -> AssertionError: MetadataOptions should round-trip as {'HttpTokens': 'required', 'HttpPutResponseHopLimit': 2, 'HttpEndpoint': 'enabled'}; got {'State': 'applied', 'HttpTokens': 'required', 'HttpPutResponseHopLimit': 2, 'HttpEndpoint': 'enabled'}
9. ec2.create-vpn-gateway-empty-availability-zone-creates-gateway -> AssertionError: CreateVpnGateway AvailabilityZone 'us-west-2-nozone': expected ('InvalidParameterValue', 'Value (us-west-2-nozone) for parameter availabilityZone is invalid. VpnConnection gateways can currently only be created in the following availability zones: <zones>.'), got ('InvalidParameterValue', "Invalid value 'us-west-2-nozone' for availabilityZone. Region 'us-west-2' has no such Availability Zone")
10. ec2.modify-vpc-attribute-conflicting-attrs-rejected -> AssertionError: You cannot modify enableDnsSupport and enableDnsHostnames in the same request. Use separate requests for each attribute.

Model evidence: smithy_query(query="operation", operation="CreateVpcEndpoint,CreateNetworkAclEntry") on services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json - CreateVpcEndpoint input has PrivateDnsEnabled: {target: Boolean} with no default and errors: []; CreateNetworkAclEntry requires Protocol (String) and makes PortRange optional, errors: [].

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; find the ten checks in trace_ids under `### ec2` (all unmarked).
2. Standalone repros:
   - CreateNetworkAcl, then CreateNetworkAclEntry with Protocol='-1', RuleAction='allow', RuleNumber=32767; DescribeNetworkAcls must show NO PortRange on that entry.
   - CreateNetworkAclEntry with Protocol='tcp': expect AWS's protocol-validation message, not the emulator range sentence.
   - CreateNetworkAclEntry with Protocol='6' and no PortRange: expect AWS's missing-member message.
   - CreateVpcEndpoint(VpcEndpointType='Interface', ServiceName='com.amazonaws.<region>.appstream.streaming') with PrivateDnsEnabled omitted; DescribeVpcEndpoints must report PrivateDnsEnabled=true.
   - Repeat the same CreateVpcEndpoint in the same VPC with PrivateDnsEnabled=true: expect a refusal.
   - CreateVpcEndpoint with PrivateDnsEnabled=true for a service whose DescribeVpcEndpointServices detail has no PrivateDnsName: expect a refusal.
   - DescribeVpcEndpointServices(ServiceNames=['com.amazonaws.<region>.iot.data']): VpcEndpointPolicySupported must be false.
   - CreateLaunchTemplate with MetadataOptions={HttpTokens:'required',HttpPutResponseHopLimit:2,HttpEndpoint:'enabled'}; DescribeLaunchTemplateVersions must not add 'State'.
   - CreateVpnGateway(AvailabilityZone='<region>-nozone'): expect the `VpnConnection gateways can currently only be created in the following availability zones` message.
   - ModifyVpcAttribute with both EnableDnsSupport and EnableDnsHostnames: expect AWS's wording.
