Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-asgeni-traj-attach-device-index-and-limit
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-asgeni-traj-attach-device-index-and-limit ec2-asgeni-traj-attach-placement-ids-and-teardown-order ec2-ipam-trajwire-child-pool-create-mistakes ec2-tgwc-traj-empty-tgw-route-table-search ec2-tgwc-traj-tgw-route-table-lookup-mistakes ec2-tgwc-traj-vpc-attachment-to-absent-tgw ec2-usg-traj-rule-referenced-group-is-unattached-yet-used ec2-vvd-traj-subnet-create-mistakes-then-recovery
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AttachNetworkInterface|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|AttachNetworkInterface|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateIpamPool|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateIpamPool|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|SearchTransitGatewayRoutes|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|SearchTransitGatewayRoutes|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateTransitGatewayVpcAttachment|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateTransitGatewayVpcAttachment|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeSecurityGroupReferences|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeSecurityGroupReferences|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateSubnet|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateSubnet|4xx->4xx|$.Response.Errors.Error.Message
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# EC2 missing required parameters are refused as MalformedInput instead of MissingParameter

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachNetworkInterface

## Description
Across six implemented ec2 operations, a request that omits a modeled required input member is refused with the emulator's own structural-validation error (code MalformedInput, message '1 validation error detected: Value null at '<member>' failed to satisfy constraint: Member must not be null') instead of AWS's MissingParameter refusal. The HTTP status (400) is right; the error code and the whole message are wrong, and they are wrong in the same way on every operation, so this is one shared defect in the required-member check that runs ahead of each handler.

Cause 1 of 6: AttachNetworkInterface without deviceIndex.
AWS: Smithy: AttachNetworkInterface input member DeviceIndex (ec2QueryName DeviceIndex, clientOptional+required) — a request without it is refused 400 with Code 'MissingParameter' and Message 'The request must contain the parameter deviceIndex'. Fixed text: 'The request must contain the parameter '; variable part: the wire member name taken from the request model (deviceIndex).
Emulator: 400 with Code 'MalformedInput' and Message "1 validation error detected: Value null at 'deviceIndex' failed to satisfy constraint: Member must not be null".
Why: clients branch on the error code; code MissingParameter identifies an omitted parameter and is retried by filling it in, while MalformedInput reads as a protocol/serialisation fault and is not retried. Error-message assertions in tooling also break.
Fix: the required-member validation layer for the operation's handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/attach_network_interface.rs and the shared input-validation helper it calls) must emit MissingParameter with AWS's sentence for a null required member.
Traces: ec2-asgeni-traj-attach-device-index-and-limit#task#8 (cluster also covers task#10/#12/#13 and ec2-asgeni-traj-attach-placement-ids-and-teardown-order#task#10/#11/#12).

Cause 2 of 6: CreateIpamPool without AddressFamily.
AWS: Smithy: CreateIpamPool input member AddressFamily (enum, clientOptional+required) — refused 400 MissingParameter, Message 'The request must include the AddressFamily parameter. Add the required parameter and retry the request.' (fixed text with the member name AddressFamily substituted).
Emulator: 400 MalformedInput, "1 validation error detected: Value null at 'addressFamily' failed to satisfy constraint: Member must not be null".
Why: as above; additionally AWS's IPAM family of operations uses the 'must include the <X> parameter. Add the required parameter and retry the request.' sentence form, which the emulator never produces.
Fix: validation for create_ipam_pool.rs.
Traces: ec2-ipam-trajwire-child-pool-create-mistakes#task#7.

Cause 3 of 6: SearchTransitGatewayRoutes with no Filters.
AWS: recorded behavior only — 400 Code 'MissingParameter', Message 'Filters cannot be null or empty' (fixed text, no variable part).
Emulator: 400 MalformedInput, "1 validation error detected: Value null at 'filters' failed to satisfy constraint: Member must not be null".
Why: same code/message break.
Fix: validation for search_transit_gateway_routes.rs.
Traces: ec2-tgwc-traj-empty-tgw-route-table-search#task#5 (cluster also covers ec2-tgwc-traj-tgw-route-table-lookup-mistakes#task#5/#8).

Cause 4 of 6: CreateTransitGatewayVpcAttachment without SubnetIds.
AWS: recorded behavior only — 400 Code 'MissingParameter', Message 'The request must include the SubnetIds parameter. Add the required parameter and retry the request.'
Emulator: 400 MalformedInput, "1 validation error detected: Value null at 'subnetIds' failed to satisfy constraint: Member must not be null".
Why: same; also the attachment is refused before the absent transit-gateway id is even considered, so clients cannot tell the two mistakes apart.
Fix: validation for create_transit_gateway_vpc_attachment.rs.
Traces: ec2-tgwc-traj-vpc-attachment-to-absent-tgw#task#6 (cluster also covers task#7..#10).

Cause 5 of 6: DescribeSecurityGroupReferences without GroupId.
AWS: recorded behavior only — 400 Code 'MissingParameter', Message 'The request must contain the parameter groupIds' (note AWS names the plural list parameter groupIds).
Emulator: 400 MalformedInput, "1 validation error detected: Value null at 'groupId' failed to satisfy constraint: Member must not be null".
Why: same code break, plus the emulator names a different (singular) parameter than AWS does, so a client that parses the parameter name out of the message fixes the wrong field.
Fix: validation for describe_security_group_references.rs.
Traces: ec2-usg-traj-rule-referenced-group-is-unattached-yet-used#task#8 (cluster also covers task#9).

Cause 6 of 6: CreateSubnet without VpcId.
AWS: recorded behavior only — 400 Code 'MissingParameter', Message 'The request must contain the parameter vpcId'.
Emulator: 400 MalformedInput, "1 validation error detected: Value null at 'vpcId' failed to satisfy constraint: Member must not be null".
Why: same.
Fix: validation for create_subnet.rs.
Traces: ec2-vvd-traj-subnet-create-mistakes-then-recovery#task#2.

## Evidence
Replayed wire corpus simparity-wire-aws-v1. Representative: ec2-asgeni-traj-attach-device-index-and-limit step task#8 — expected status 400 / Code 'MissingParameter' / Message 'The request must contain the parameter deviceIndex'; actual status 400 / Code 'MalformedInput' / Message "1 validation error detected: Value null at 'deviceIndex' failed to satisfy constraint: Member must not be null". The same expected-vs-actual pair recurs for CreateIpamPool (AddressFamily), SearchTransitGatewayRoutes (Filters), CreateTransitGatewayVpcAttachment (SubnetIds), DescribeSecurityGroupReferences (groupIds) and CreateSubnet (vpcId); 20 steps are affected in total across the six clusters.

## Steps to Reproduce
1. Point an EC2 client at the emulator's data listener.
2. Replay fixture ec2-asgeni-traj-attach-device-index-and-limit and inspect step task#8: AttachNetworkInterface with NetworkInterfaceId and InstanceId but no DeviceIndex.
3. Observe Code 'MalformedInput' where the recording has 'MissingParameter' and the AWS sentence 'The request must contain the parameter deviceIndex'.
4. Repeat with the five other fixture#step tokens listed in trace_ids (CreateIpamPool, SearchTransitGatewayRoutes, CreateTransitGatewayVpcAttachment, DescribeSecurityGroupReferences, CreateSubnet).
