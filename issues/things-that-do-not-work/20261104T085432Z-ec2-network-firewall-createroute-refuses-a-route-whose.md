Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__network-firewall.nfwr-routed-firewall-endpoint-refusals-rejected
Issue-Key: ec2__network-firewall|CreateRoute|InvalidGatewayID.NotFound
Issue-Commit: 2a533320acecc5e3d5510f43fe45b8adcca9ff2d

# ec2__network-firewall: CreateRoute refuses a route whose target is a READY firewall's VPC endpoint with 400 InvalidGatewayID.NotFound instead of accepting it

> _Note: this issue was identified against the code base at commit `2a533320acecc5e3d5510f43fe45b8adcca9ff2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoute

## Description
Cause 1 of 1: a CreateRoute naming an EXISTING Network Firewall endpoint as VpcEndpointId, in a route table of the endpoint's own VPC, is refused with HTTP 400 instead of being accepted.

AWS: Smithy com.amazonaws.ec2#CreateRoute takes RouteTableId (required) plus exactly one target member; VpcEndpointId (target shape VpcEndpointId) is one of those members. CreateRouteResult carries only `return`, and the operation models no errors (errors: []). A route whose VpcEndpointId is the Gateway Load Balancer endpoint Network Firewall materializes for a READY firewall — the id AWS reports at DescribeFirewall FirewallStatus.SyncStates.<az>.Attachment.EndpointId — and whose route table belongs to the same VPC as that endpoint, is accepted: HTTP 200 with return=true. This is the normal way traffic is steered through a firewall endpoint, and it is the check's own positive control.

Emulator: HTTP 400 with error code InvalidGatewayID.NotFound and message "The gateway ID '<vpcEndpointId>' does not exist". The fixed text of that message is `The gateway ID '` … `' does not exist`; the quoted value is the VpcEndpointId the request carried (a vpce-… id taken from the firewall's SyncStates), not a value fixed by the message. So the emulator reports the gateway noun and the gateway not-found code for a parameter the caller supplied as a VPC endpoint, and the route is never created. Hypothesis: VpcEndpointId is resolved through the gateway lookup rather than against VPC-endpoint state, and/or the endpoint Network Firewall reports in SyncStates is never registered in EC2's VPC-endpoint state — neither the model nor the replay output states which.

Why: a client that follows AWS's documented pattern for Network Firewall (create firewall, wait for READY, read the endpoint id from SyncStates, point a route at it) cannot create that route at all. Every behaviour that depends on a firewall endpoint being routed is unreachable: the routed-firewall DeleteFirewall refusal (InvalidRequestException "Unable to fulfill request because the following related VPC endpoint(s) still exist in route table(s): [<endpointId>]"), the endpoint-subnet DependencyViolation refusal, and the cross-VPC InvalidParameterValue refusal are never exercised, and a client branching on the error code is told to fix a gateway it never named.

Fix: the EC2 CreateRoute handler's target resolution and the VPC-endpoint state it resolves against — a VpcEndpointId target must be looked up as a VPC endpoint (including endpoints created on behalf of a READY Network Firewall) and, when it exists in the route table's VPC, the route must be created and the call must answer 200 with return=true.

Related already-open issue (does NOT cover this): 20261104T062058Z-ec2-createroute-refuses-a-valid-route-and-authors.md describes CreateRoute refusing a NON-EXISTENT VpcEndpointId with the gateway code/noun, and a different valid-route refusal from the subnet/route-table fixture. The replay report did not mark this check as owned by it, and the divergence here is an EXISTING firewall endpoint being refused outright.

Traces: behavioral check ec2__network-firewall.nfwr-routed-firewall-endpoint-refusals-rejected (the uncaught failure is the positive-control create_route on the DMZ route table). The same root cause also blocks network-firewall.stateful-rule-drops-matching-traffic-allows-other, filed separately because it is listed under a different service heading.

## Evidence
Replay of suite simparity-behavioral-aws-v1 (emulator up; ec2__network-firewall 3/9 passed).

[error] ec2__network-firewall.nfwr-routed-firewall-endpoint-refusals-rejected
  botocore.exceptions.ClientError: An error occurred (InvalidGatewayID.NotFound) when calling the CreateRoute operation: The gateway ID 'vpce-e5254a0aa4fdf64b4' does not exist

The exception escapes the check uncaught, which places it at the un-wrapped positive-control call:
  assert ec2.create_route(RouteTableId=dmz_rt, DestinationCidrBlock="0.0.0.0/0", VpcEndpointId=ep)["Return"] is True, "positive control: same-VPC route refused"
where `ep` = describe_firewall(FirewallArn=arn)["FirewallStatus"]["SyncStates"][az]["Attachment"]["EndpointId"] of a firewall the emulator itself reported READY, and `dmz_rt` is a route table in that firewall's own VPC (10.2.0.0/16).

Expected: HTTP 200, CreateRouteResult return=true, and the route present in the table.
Actual: HTTP 400, Error.Code=InvalidGatewayID.NotFound, Error.Message="The gateway ID '<vpce-id>' does not exist" (the vpce- id is the request's VpcEndpointId; only the id varies between runs).

The immediately preceding probe in the same check — the same endpoint id passed as GatewayId — is expected to be refused with exactly InvalidGatewayID.NotFound / "The gateway ID '<ep>' does not exist", so the emulator answers the VpcEndpointId form of the request with the GatewayId form's refusal.

Model consulted: smithy_query query=operation operation=CreateRoute on services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json — input member VpcEndpointId (target VpcEndpointId), output CreateRouteResult.Return, errors: [].

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it.
2. Run the behavioral suite (simparity-behavioral-aws-v1).
3. Replay check ec2__network-firewall.nfwr-routed-firewall-endpoint-refusals-rejected.
4. Equivalently, by hand against the emulator: create a VPC (10.2.0.0/16) with two subnets in two AZs; create a Network Firewall policy and a firewall on those subnets; poll DescribeFirewall until FirewallStatus.Status is READY and read FirewallStatus.SyncStates.<az>.Attachment.EndpointId; create a route table in the same VPC; call ec2 CreateRoute with RouteTableId=<that table>, DestinationCidrBlock=0.0.0.0/0, VpcEndpointId=<that endpoint id>.
5. The call answers 400 InvalidGatewayID.NotFound "The gateway ID '<vpce-id>' does not exist" instead of 200 with return=true; it reproduces deterministically.
