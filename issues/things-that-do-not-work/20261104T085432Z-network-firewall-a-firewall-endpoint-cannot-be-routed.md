Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: network-firewall.stateful-rule-drops-matching-traffic-allows-other
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|CreateRoute|InvalidGatewayID.NotFound
Issue-Commit: 2a533320acecc5e3d5510f43fe45b8adcca9ff2d

# network-firewall: a firewall endpoint cannot be routed to — CreateRoute with the READY firewall's VpcEndpointId answers 400 InvalidGatewayID.NotFound, blocking the stateful data-plane check

> _Note: this issue was identified against the code base at commit `2a533320acecc5e3d5510f43fe45b8adcca9ff2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoute

## Description
Cause 1 of 1: the endpoint a READY firewall reports in SyncStates cannot be used as a route target, so no traffic can ever be steered through a firewall and the stateful-rule enforcement behaviour is unobservable.

AWS: Smithy com.amazonaws.ec2#CreateRoute takes RouteTableId (required) plus exactly one target member, VpcEndpointId among them (target shape VpcEndpointId); CreateRouteResult carries only `return` and the operation models no errors (errors: []). Pointing a more-specific route at the Gateway Load Balancer endpoint Network Firewall creates for a READY firewall — the id reported at DescribeFirewall FirewallStatus.SyncStates.<az>.Attachment.EndpointId — from a route table in the same VPC is accepted with HTTP 200 and return=true. This is the only supported way to put a Network Firewall endpoint in the data path, and AWS then enforces the policy's stateful rules on packets that traverse it (a STATEFUL DROP rule for tcp/8080 to one host drops that flow while tcp/8081 to the same host passes).

Emulator: the CreateRoute call that inserts the firewall endpoint into the workload route table is refused with HTTP 400, error code InvalidGatewayID.NotFound and message "The gateway ID '<vpcEndpointId>' does not exist" — fixed text `The gateway ID '` … `' does not exist`, with the quoted value being the VpcEndpointId the request carried (a vpce-… id read back from the emulator's own DescribeFirewall response). The exception escapes the check, so nothing past the routing step runs. Hypothesis: the endpoint id Network Firewall publishes in SyncStates is not registered as an EC2 VPC endpoint (and/or CreateRoute resolves VpcEndpointId through the gateway lookup) — neither the model nor the replay output states which.

Why: a client building the documented Network Firewall topology (firewall subnet + workload subnets, routes via the firewall endpoint) cannot build it at all against the emulator. Consequently the firewall never sees a packet, and a client cannot observe that its policy actually enforces anything — only API state is observable, which is exactly what this check exists to rule out.

Fix: the EC2 CreateRoute handler's target resolution together with the Network Firewall state that should materialize a routable VPC endpoint per subnet mapping when a firewall reaches READY; a VpcEndpointId target that exists in the route table's VPC must yield 200/return=true. Downstream, the firewall endpoint must be in the forwarding path for the stateful policy to be enforceable.

Traces: behavioral check network-firewall.stateful-rule-drops-matching-traffic-allows-other. The same root cause also blocks ec2__network-firewall.nfwr-routed-firewall-endpoint-refusals-rejected, filed separately because it is listed under a different service heading. Related already-open issue, which does NOT cover this (it concerns a NON-EXISTENT VpcEndpointId getting the gateway code/noun, and a different valid-route refusal): 20261104T062058Z-ec2-createroute-refuses-a-valid-route-and-authors.md.

## Evidence
Replay of suite simparity-behavioral-aws-v1 (emulator up; network-firewall 18/25 passed).

[error] network-firewall.stateful-rule-drops-matching-traffic-allows-other
  botocore.exceptions.ClientError: An error occurred (InvalidGatewayID.NotFound) when calling the CreateRoute operation: The gateway ID 'vpce-7277427719d3cb267' does not exist

The check had already, successfully: created the VPC (10.99.0.0/16) with firewall/workload/target subnets, created a STATEFUL rule group with a DROP rule for TCP 10.99.2.10/32 port 8080, created the policy referencing it, created the firewall on the firewall subnet, waited for READY, and read
  endpoint_id_fw = describe_firewall(...)["FirewallStatus"]["SyncStates"][az]["Attachment"]["EndpointId"].
It then failed on:
  ec2.create_route(RouteTableId=workload_rt, DestinationCidrBlock="10.99.2.0/24", VpcEndpointId=endpoint_id_fw)

Expected: HTTP 200 with CreateRouteResult return=true (the route table and the endpoint are in the same VPC, and the endpoint id came from the emulator's own DescribeFirewall).
Actual: HTTP 400, Error.Code=InvalidGatewayID.NotFound, Error.Message="The gateway ID '<vpce-id>' does not exist". Only the vpce- id varies per run; it is the request's VpcEndpointId.

Everything after step 5 of the check (target route table, workload instance, tag-reported 8080 drop / 8081 pass) is therefore never exercised.

Model consulted: smithy_query query=operation operation=CreateRoute on services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json — input member VpcEndpointId (target VpcEndpointId), output CreateRouteResult.Return, errors: [].

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it.
2. Run the behavioral suite (simparity-behavioral-aws-v1).
3. Replay check network-firewall.stateful-rule-drops-matching-traffic-allows-other.
4. Equivalently, by hand: create a VPC 10.99.0.0/16 with a firewall subnet and a workload subnet; create a STATEFUL rule group (DROP TCP to 10.99.2.10/32 port 8080) and a firewall policy referencing it; CreateFirewall on the firewall subnet and poll DescribeFirewall until READY; read FirewallStatus.SyncStates.<az>.Attachment.EndpointId; create a route table associated with the workload subnet and call ec2 CreateRoute with DestinationCidrBlock=10.99.2.0/24 and VpcEndpointId=<that endpoint id>.
5. The call answers 400 InvalidGatewayID.NotFound "The gateway ID '<vpce-id>' does not exist" instead of 200/return=true, and the firewall can never be placed in the data path. Reproduces deterministically.
