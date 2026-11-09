Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-vpce-traj-iot-data-endpoint-create-refusals
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-vpce-traj-iot-data-endpoint-create-refusals ec2-vpc-endpoints-filters-ids-empty-list-and-paginated-continuation ec2-gateway-vpc-endpoint-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateVpcEndpoint|4xx->2xx|$.CreateVpcEndpointResponse.vpcEndpoint.dnsEntrySet.item[*].dnsName
Issue-Key: ec2|DescribeVpcEndpoints|2xx->2xx|$.DescribeVpcEndpointsResponse.vpcEndpointSet.item[*]
Issue-Key: ec2|DescribeVpcEndpointServices|2xx->2xx|$.DescribeVpcEndpointServicesResponse.serviceDetailSet.item.privateDnsNameSet.item.privateDnsName
Issue-Related: 20261030T063325Z-ec2-input-validation-missing-on-four-read-and.md
Issue-Related: 20261030T063325Z-ec2-static-catalogs-diverge-from-aws-instance-type.md
Issue-Related: 20261104T130156Z-ec2-describevpcendpointservices-merges-gateway-and-interface-service-details.md
Issue-Related: 20261108T143229Z-ec2-static-describe-catalogs-serve-synthesized-hardware-and.md
Issue-Commit: 3b171590106c1f2d83787caf96ecc291950ccc13

# EC2 VPC endpoint plane: missing endpoint-policy refusal and endpoint reads drop modeled members

> _Note: this issue was identified against the code base at commit `3b171590106c1f2d83787caf96ecc291950ccc13`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
This issue bundles THREE distinct causes in the EC2 VPC-endpoint plane (CreateVpcEndpoint validation, the VpcEndpoint projection, and the single-service endpoint-service read). All THREE must be closed before submitting.

--- Cause 1 of 3: CreateVpcEndpoint accepts a restricted endpoint policy for a service that only supports the full-access policy ---
AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): CreateVpcEndpoint input members ServiceName and PolicyDocument are plain String; the operation declares no modeled errors (errors: []) because EC2 is an ec2Query service whose error codes are wire-only. Recorded behavior: for an endpoint service that does not support endpoint policies other than full access, AWS refuses the create with HTTP 400, Response.Errors.Error.Code 'InvalidParameter' and Response.Errors.Error.Message 'Service <serviceName> only supports the full-access endpoint policy.' Message template: the fixed text is 'Service ' ... ' only supports the full-access endpoint policy.'; the single variable part is the ServiceName copied verbatim from the request (the recording's 'com.amazonaws.us-west-2.iot.data' is one such request value, not part of the template).
Emulator: HTTP 200 and a created endpoint. Replay: 'status: expected 400, got 200' and '$.Response: missing (expected {Errors:{Error:{Code:InvalidParameter, Message:"Service com.amazonaws.us-west-2.iot.data only supports the full-access endpoint policy."}}}, got None)'. In the same response the emulator also publishes vpcEndpoint.dnsEntrySet[*].dnsName values under *.vpce.amazonaws.com (flagged host_not_own: 'vpce-...iot.data.us-west-2.vpce.amazonaws.com', 'vpce-...-us-west-2a.iot.data.us-west-2.vpce.amazonaws.com') — hostnames it does not serve, so a client that follows them leaves the emulated environment.
Why: the whole point of this refusal is that a restrictive policy on such a service would silently NOT be enforced. The emulator accepts it, so a client believes it has locked an endpoint down to a policy that is not applied, and negative tests that assert InvalidParameter never fire.
Fix: CreateVpcEndpoint's validation plus the endpoint-service capability table, in services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs — when the named service permits only the full-access policy, refuse a non-full-access PolicyDocument with 400 / InvalidParameter and the message template above; the same handler's serializer should emit dnsEntrySet names the emulator actually answers.
Traces: ec2-vpce-traj-iot-data-endpoint-create-refusals#task#4.

--- Cause 2 of 3: DescribeVpcEndpoints omits members AWS always emits, and drops the policy document's Version field ---
AWS: Smithy shape VpcEndpoint models dnsEntrySet (DnsEntries), dnsOptions (DnsOptions), groupSet (Groups), ipAddressType (IpAddressType), networkInterfaceIdSet (NetworkInterfaceIds), privateDnsEnabled (PrivateDnsEnabled) and subnetIdSet (SubnetIds). Recorded behavior: a Gateway endpoint row carries ALL of them — the list members as empty elements (dnsEntrySet '', groupSet '', networkInterfaceIdSet '', subnetIdSet ''), ipAddressType 'ipv4', privateDnsEnabled 'false', and dnsOptions {dnsRecordIpType 'service-defined'} — and its policyDocument is the default full-access document INCLUDING its '"Version":"2008-10-17"' statement-level key.
Emulator: those seven members are absent from every vpcEndpointSet item, and policyDocument is serialized without the Version key (got '{"Statement":[{"Action":"*","Effect":"Allow","Principal":"*","Resource":"*"}]}' where AWS returns '{"Version":"2008-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"*","Resource":"*"}]}'). Replay reports the whole item as 'missing' against the recorded item for both rows of the page.
Why: a client that reads an endpoint to learn its IP address type, its DNS record type or whether private DNS is on gets no value at all and must guess; a client that round-trips policyDocument through a policy parser gets a document with no Version, which IAM-style parsers reject or default differently. Absent-vs-empty is also the difference between 'no security groups' and 'unknown'.
Fix: the VpcEndpoint projection/serializer and the stored default policy document — services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpc_endpoints.rs and the vpc-endpoint state record — emit every modeled member (empty list members as empty elements) and keep the Version key in the default policy.
Traces: ec2-vpc-endpoints-filters-ids-empty-list-and-paginated-continuation#task#5 (cluster also covers #task#6).

--- Cause 3 of 3: DescribeVpcEndpointServices for a single Gateway service reports the wrong serviceType set, omits an IP address type, and publishes real-AWS private DNS names ---
AWS: Smithy shape ServiceDetail models serviceType (ServiceTypeDetailSet), supportedIpAddressTypeSet (length 0..2), privateDnsName, privateDnsNameSet and baseEndpointDnsNameSet. Recorded behavior for the s3 gateway service in the fixture: serviceType is the single entry {serviceType 'Gateway'}, supportedIpAddressTypeSet carries two entries ('ipv4' and 'ipv6'), and the recording asserts no privateDnsName / privateDnsNameSet for it.
Emulator: serviceType is a two-entry list [{'Gateway'}, {'Interface'}] (type_mismatch), supportedIpAddressTypeSet is missing its second entry 'ipv6', baseEndpointDnsNameSet carries two names instead of the asserted shape, and it publishes privateDnsName and privateDnsNameSet.item.privateDnsName as 's3.us-west-2.amazonaws.com' — flagged host_not_own, a hostname the emulator does not serve.
Why: serviceType is how a client decides whether to create a Gateway (route-table) or Interface (subnet+SG) endpoint; reporting both makes that choice ambiguous and makes a Gateway-only service look like it accepts subnets and security groups. A missing ipv6 entry makes dual-stack endpoint creation look unsupported, and a private DNS name pointing at real AWS sends in-VPC name resolution out of the emulated environment.
Fix: the endpoint-service catalog row construction and the DescribeVpcEndpointServices serializer in services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpc_endpoint_services.rs — derive serviceType, supportedIpAddressTypeSet and the DNS-name members per service rather than from one shared Interface-shaped template.
Traces: ec2-gateway-vpc-endpoint-lifecycle#task#3.

Related but NOT covering: open issue 20261109T114319Z-ec2-iam-imagebuilder-describevpcendpointservices-refuses-com-amazonaws-region.md reports a DIFFERENT DescribeVpcEndpointServices fault (InvalidServiceName for imagebuilder). The replay reports owned_by empty and stub_path empty for all three steps above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, bootstrap_signal false, 2742/3942 steps passed, 1081 mismatched. Three net-new clusters, each with stub_path empty and owned_by empty:

1) ec2-vpce-traj-iot-data-endpoint-create-refusals#task#4 — CreateVpcEndpoint, status expected 400, got 200.
   $.Response: missing (expected {'Errors': {'Error': {'Code': 'InvalidParameter', 'Message': 'Service com.amazonaws.us-west-2.iot.data only supports the full-access endpoint policy.'}}, 'RequestID': '<shaped-not-asserted>'}, got None)
   $.CreateVpcEndpointResponse.vpcEndpoint.dnsEntrySet.item[0].dnsName: host_not_own (expected None, got 'vpce-0000000000000008c.iot.data.us-west-2.vpce.amazonaws.com')
   $.CreateVpcEndpointResponse.vpcEndpoint.dnsEntrySet.item[1].dnsName: host_not_own (expected None, got 'vpce-0000000000000008c-us-west-2a.iot.data.us-west-2.vpce.amazonaws.com')

2) ec2-vpc-endpoints-filters-ids-empty-list-and-paginated-continuation#task#5 (affects #task#6) — DescribeVpcEndpoints, 200 vs 200.
   $.DescribeVpcEndpointsResponse.vpcEndpointSet.item[0] and item[1]: missing — expected rows carrying creationTimestamp, dnsEntrySet '', dnsOptions {dnsRecordIpType 'service-defined'}, groupSet '', ipAddressType 'ipv4', networkInterfaceIdSet '', ownerId, policyDocument '{"Version":"2008-10-17","Statement":[...]}', privateDnsEnabled 'false', requesterManaged 'false', routeTableIdSet, serviceName, serviceRegion, state 'available', subnetIdSet '', tagSet '', vpcEndpointId, vpcEndpointType 'Gateway', vpcId; got rows with dnsEntrySet/dnsOptions/groupSet/ipAddressType/networkInterfaceIdSet/privateDnsEnabled/subnetIdSet absent and policyDocument '{"Statement":[{"Action":"*","Effect":"Allow","Principal":"*","Resource":"*"}]}'.

3) ec2-gateway-vpc-endpoint-lifecycle#task#3 — DescribeVpcEndpointServices, 200 vs 200.
   $...serviceDetailSet.item.serviceType.item: type_mismatch (expected {'serviceType': 'Gateway'}, got [{'serviceType': 'Gateway'}, {'serviceType': 'Interface'}])
   $...serviceDetailSet.item.supportedIpAddressTypeSet.item[1]: missing (expected 'ipv6', got None)
   $...serviceDetailSet.item.baseEndpointDnsNameSet.item: value_mismatch (got ['s3.us-west-2.vpce.amazonaws.com', 's3.us-west-2.amazonaws.com'])
   $...serviceDetailSet.item.privateDnsName / privateDnsNameSet.item.privateDnsName: host_not_own (expected None, got 's3.us-west-2.amazonaws.com')
   $...serviceDetailSet.item.serviceId: value_mismatch against the asserted shape (got 'vpce-svc-fffbb61c')

Model evidence: smithy_query operation=CreateVpcEndpoint -> input {VpcId required, ServiceName, PolicyDocument, VpcEndpointType, RouteTableIds, SubnetIds, SecurityGroupIds, IpAddressType, DnsOptions, PrivateDnsEnabled, ...}, errors []. smithy_query shape=VpcEndpoint -> members include dnsEntrySet, dnsOptions, groupSet, ipAddressType, networkInterfaceIdSet, privateDnsEnabled, subnetIdSet, policyDocument. smithy_query shape=ServiceDetail -> members include serviceType (ServiceTypeDetailSet), supportedIpAddressTypeSet (length 0..2), privateDnsName, privateDnsNameSet, baseEndpointDnsNameSet.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay ec2-vpce-traj-iot-data-endpoint-create-refusals step task#4 — CreateVpcEndpoint for com.amazonaws.<region>.iot.data with a PolicyDocument that is not the full-access policy. Observe HTTP 200 plus dnsEntrySet names under *.vpce.amazonaws.com instead of HTTP 400 / InvalidParameter 'Service <serviceName> only supports the full-access endpoint policy.'
4. Cause 2: replay ec2-vpc-endpoints-filters-ids-empty-list-and-paginated-continuation step task#5 (and task#6) — DescribeVpcEndpoints over two Gateway endpoints. Observe each vpcEndpointSet item missing dnsEntrySet, dnsOptions, groupSet, ipAddressType, networkInterfaceIdSet, privateDnsEnabled and subnetIdSet, and policyDocument serialized without its Version key.
5. Cause 3: replay ec2-gateway-vpc-endpoint-lifecycle step task#3 — DescribeVpcEndpointServices filtered to the s3 gateway service. Observe serviceType listing both Gateway and Interface, supportedIpAddressTypeSet missing 'ipv6', and privateDnsName/privateDnsNameSet set to 's3.<region>.amazonaws.com'.
