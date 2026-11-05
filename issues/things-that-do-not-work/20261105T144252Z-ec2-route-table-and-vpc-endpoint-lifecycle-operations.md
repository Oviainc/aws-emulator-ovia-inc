Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-gateway-vpc-endpoint-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-gateway-vpc-endpoint-lifecycle ec2-lsrno-traj-cross-region-s3-endpoint-refused ec2-lsrno-traj-isolated-subnet-routes-only-same-region-s3 ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter ec2-subnet-route-table-lifecycle ec2-route-table-association-swap
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateVpcEndpoint|2xx->2xx|$.CreateVpcEndpointResponse.vpcEndpoint.dnsEntrySet
Issue-Key: ec2|CreateVpcEndpoint|2xx->2xx|$.CreateVpcEndpointResponse.vpcEndpoint.dnsOptions
Issue-Key: ec2|CreateVpcEndpoint|2xx->2xx|$.CreateVpcEndpointResponse.vpcEndpoint.groupSet
Issue-Key: ec2|CreateVpcEndpoint|2xx->2xx|$.CreateVpcEndpointResponse.vpcEndpoint.ipAddressType
Issue-Key: ec2|CreateVpcEndpoint|2xx->2xx|$.CreateVpcEndpointResponse.vpcEndpoint.networkInterfaceIdSet
Issue-Key: ec2|CreateVpcEndpoint|2xx->2xx|$.CreateVpcEndpointResponse.vpcEndpoint.policyDocument
Issue-Key: ec2|CreateVpcEndpoint|2xx->2xx|$.CreateVpcEndpointResponse.vpcEndpoint.privateDnsEnabled
Issue-Key: ec2|CreateVpcEndpoint|2xx->2xx|$.CreateVpcEndpointResponse.vpcEndpoint.state
Issue-Key: ec2|CreateVpcEndpoint|2xx->2xx|$.CreateVpcEndpointResponse.vpcEndpoint.subnetIdSet
Issue-Key: ec2|DescribeVpcEndpoints|2xx->2xx|$.DescribeVpcEndpointsResponse.vpcEndpointSet.item.dnsEntrySet
Issue-Key: ec2|DescribeVpcEndpoints|2xx->2xx|$.DescribeVpcEndpointsResponse.vpcEndpointSet.item.dnsOptions
Issue-Key: ec2|DescribeVpcEndpoints|2xx->2xx|$.DescribeVpcEndpointsResponse.vpcEndpointSet.item.groupSet
Issue-Key: ec2|DescribeVpcEndpoints|2xx->2xx|$.DescribeVpcEndpointsResponse.vpcEndpointSet.item.ipAddressType
Issue-Key: ec2|DescribeVpcEndpoints|2xx->2xx|$.DescribeVpcEndpointsResponse.vpcEndpointSet.item.networkInterfaceIdSet
Issue-Key: ec2|DescribeVpcEndpoints|2xx->2xx|$.DescribeVpcEndpointsResponse.vpcEndpointSet.item.privateDnsEnabled
Issue-Key: ec2|DescribeVpcEndpoints|2xx->2xx|$.DescribeVpcEndpointsResponse.vpcEndpointSet.item.state
Issue-Key: ec2|DescribeVpcEndpoints|2xx->2xx|$.DescribeVpcEndpointsResponse.vpcEndpointSet.item.subnetIdSet
Issue-Key: ec2|CreateRoute|4xx->2xx|$.Response
Issue-Key: ec2|CreateRoute|4xx->2xx|status
Issue-Key: ec2|DescribeRouteTables|2xx->2xx|$.DescribeRouteTablesResponse.routeTableSet.item.routeSet.item[*]
Issue-Key: ec2|DisassociateRouteTable|2xx->4xx|status
Issue-Key: ec2|DisassociateRouteTable|2xx->2xx|$.DisassociateRouteTableResponse.associationState
Issue-Key: ec2|DeleteRouteTable|2xx->4xx|status
Issue-Related: 20261103T123451Z-ec2-vpc-endpoint-dns-entries-policy-document-and.md
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# EC2 route-table and VPC-endpoint lifecycle operations return wrong state, shape and skip target validation

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
Shared fault: the VPC-endpoint and route-table planes do not model the resource the way AWS serialises it and do not validate route targets, so created objects come back under-populated and in the wrong state, an invalid route is accepted into state, and valid teardown calls are refused. Seven causes, all ec2. An open issue, 20261103T123451Z-ec2-vpc-endpoint-dns-entries-policy-document-and.md ('EC2 VPC endpoint DNS entries, policy document and routeTableIdSet diverge'), describes adjacent symptoms but carries none of these replay keys, so these causes are filed here.

Cause 1 of 7: CreateVpcEndpoint omits modeled members, mis-serialises the default policy and reports the wrong state.
AWS: Smithy: CreateVpcEndpoint output member VpcEndpoint (xmlName 'vpcEndpoint') targets VpcEndpoint, whose members include DnsEntries (xmlName dnsEntrySet), DnsOptions (dnsOptions), Groups (groupSet), IpAddressType (ipAddressType), NetworkInterfaceIds (networkInterfaceIdSet), PrivateDnsEnabled (privateDnsEnabled), SubnetIds (subnetIdSet), State (state, enum State) and PolicyDocument (policyDocument). The recording shows all of them present for a gateway endpoint: empty list members serialised as empty elements, dnsOptions {dnsRecordIpType: 'service-defined'}, ipAddressType 'ipv4', privateDnsEnabled 'false', state 'available' on creation, and the default policy document exactly '{"Version":"2008-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"*","Resource":"*"}]}'.
Emulator: those nine members are absent or wrong — the list/option members are missing entirely, state is 'pending', and policyDocument is emitted as '{"Statement":[{"Action":"*","Effect":"Allow","Principal":"*","Resource":"*"}]}' with no Version and with keys re-ordered by the JSON serializer.
Why: a client reads back the endpoint it just made and finds no ipAddressType/privateDnsEnabled to drive its next call, a policy with no Version (which AWS rejects on re-submission), and a state it will wait on forever because a gateway endpoint is available at once.
Fix: the CreateVpcEndpoint handler's VpcEndpoint construction plus the policy-document serializer (emit Version first and preserve the modeled key order) and the endpoint state machine.
Traces: ec2-gateway-vpc-endpoint-lifecycle#task#4 (also ec2-lsrno-traj-cross-region-s3-endpoint-refused#task#6, ec2-lsrno-traj-isolated-subnet-routes-only-same-region-s3#task#6).

Cause 2 of 7: DescribeVpcEndpoints carries the same gaps on read.
AWS: the same VpcEndpoint shape members are present in the describe body (vpcEndpointSet.item), with state 'available'.
Emulator: dnsEntrySet, dnsOptions, groupSet, ipAddressType, networkInterfaceIdSet, privateDnsEnabled and subnetIdSet missing; policyDocument without Version; state 'pending'.
Why: the gap is not a creation-response-only defect — polling the endpoint never yields the modeled shape, so waiters never settle.
Fix: the shared VpcEndpoint serializer used by the describe path (same component as cause 1).
Traces: ec2-gateway-vpc-endpoint-lifecycle#task#5 (also #task#7, #task#9, ec2-lsrno-traj-cross-region-s3-endpoint-refused#task#9).

Cause 3 of 7: CreateRoute accepts a route whose VPC-endpoint target does not exist.
AWS: Smithy: CreateRoute input has VpcEndpointId/GatewayId and returns only Return; the recording shows AWS refusing the call with 400, Code 'InvalidVpcEndpointId.NotFound', Message "The vpcEndpoint ID '<id>' does not exist" (fixed text with the id from the request).
Emulator: 200 with return=true — no target validation at all.
Why: the caller believes it has a working route to a firewall endpoint that does not exist; the failure surfaces much later as a black-hole route instead of at the API boundary.
Fix: target resolution in the CreateRoute handler's validation, before the route is written to state.
Traces: ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes#task#7.

Cause 4 of 7: the route accepted in cause 3 is then reported as real.
AWS: the route table contains only the routes AWS accepted, so the bogus entry is absent from routeSet.
Emulator: DescribeRouteTables returns an extra routeSet item with the nonexistent vpce id as gatewayId, origin 'CreateRoute', state 'active'.
Why: state is corrupted, not just the response: reconciliation loops and drift detectors see a route that cannot exist and will try to converge against it.
Fix: once cause 3 rejects the call nothing is written; the DescribeRouteTables route projection must not invent state for unresolvable targets.
Traces: ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes#task#11.

Cause 5 of 7: DisassociateRouteTable refuses a valid association during teardown.
AWS: Smithy: DisassociateRouteTable takes the required AssociationId and returns 200 with no body members; the recording shows 200 for the association created earlier in the same fixture.
Emulator: 400.
Why: teardown cannot complete, so the subnet keeps its association and every later cleanup step (delete route table, delete subnet, delete VPC) fails too.
Fix: association lookup in the DisassociateRouteTable handler's state (the association created by AssociateRouteTable must be resolvable by its id).
Traces: ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter#teardown#17.

Cause 6 of 7: DisassociateRouteTable omits associationState when it does succeed.
AWS: recorded behavior only (the Smithy output shape carries no members): the response body contains associationState {state: 'disassociating'}.
Emulator: 200 with associationState absent.
Why: a caller that watches the association's transition has nothing to read and cannot tell a no-op from a started disassociation.
Fix: the DisassociateRouteTable response serializer.
Traces: ec2-subnet-route-table-lifecycle#task#17.

Cause 7 of 7: DeleteRouteTable refuses a valid route table during teardown.
AWS: recorded behavior only — 200 for a route table whose associations have been swapped away earlier in the fixture.
Emulator: 400.
Why: the same teardown dead-end as cause 5; resources leak and the fixture's remaining steps cascade.
Fix: the DeleteRouteTable handler's association/dependency check in route-table state.
Traces: ec2-route-table-association-swap#teardown#11.

## Evidence
Cause 1 (ec2-gateway-vpc-endpoint-lifecycle#task#4, 200 vs 200): dnsEntrySet missing (expected ''), dnsOptions missing (expected {'dnsRecordIpType': 'service-defined'}), groupSet missing, ipAddressType missing (expected 'ipv4'), networkInterfaceIdSet missing, privateDnsEnabled missing (expected 'false'), subnetIdSet missing, state 'pending' vs expected 'available', policyDocument '{"Statement":[{"Action":"*","Effect":"Allow","Principal":"*","Resource":"*"}]}' vs expected '{"Version":"2008-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"*","Resource":"*"}]}'.
Cause 2 (ec2-gateway-vpc-endpoint-lifecycle#task#5): identical nine-key divergence on DescribeVpcEndpoints vpcEndpointSet.item.
Cause 3 (ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes#task#7): expected 400 with Response.Errors.Error {Code 'InvalidVpcEndpointId.NotFound', Message "The vpcEndpoint ID 'vpce-0000000000000003e' does not exist"}; actual 200.
Cause 4 (same fixture #task#11): routeSet.item[2] extra — {'destinationCidrBlock': '0.0.0.0/0', 'gatewayId': 'vpce-0000000000000003e', 'origin': 'CreateRoute', 'state': 'active'} present where AWS has no such route.
Cause 5 (ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter#teardown#17): expected 200, got 400.
Cause 6 (ec2-subnet-route-table-lifecycle#task#17): associationState missing (expected {'state': 'disassociating'}).
Cause 7 (ec2-route-table-association-swap#teardown#11): expected 200, got 400.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay ec2-gateway-vpc-endpoint-lifecycle step task#4 (representative) and diff the vpcEndpoint element against the recorded body.
3. Replay the six other bindings in trace_ids: ec2-gateway-vpc-endpoint-lifecycle#task#5, ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes#task#7 and #task#11, ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter#teardown#17, ec2-subnet-route-table-lifecycle#task#17, ec2-route-table-association-swap#teardown#11.
4. All seven steps must pass before closing; cause 4 only passes once cause 3 stops writing the route.
