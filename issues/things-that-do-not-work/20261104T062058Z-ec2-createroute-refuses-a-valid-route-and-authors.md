Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-subnet-route-table-lifecycle
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-subnet-route-table-lifecycle ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes ec2-pst-traj-detached-igw-leaves-blackhole-default-route
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateRoute|2xx->4xx|$.CreateRouteResponse
Issue-Key: ec2|CreateRoute|2xx->4xx|status
Issue-Key: ec2|CreateRoute|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateRoute|4xx->4xx|$.Response.Errors.Error.Message
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# EC2 CreateRoute refuses a valid route and authors its own target refusal text

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoute

## Description
Three net-new causes in the EC2 CreateRoute handler: one request AWS accepts is refused, and two refusals AWS words precisely are answered with emulator-authored code or text. All three must be closed before this issue is submitted.

Cause 1 of 3: a valid CreateRoute is refused with HTTP 400.
AWS: Smithy com.amazonaws.ec2#CreateRoute takes RouteTableId (required) plus exactly one target member (GatewayId, NatGatewayId, VpcEndpointId, ...) and returns CreateRouteResult with return 'true' on success; the operation models no errors, and the recording shows HTTP 200 with {'requestId': ..., 'return': 'true'} for this request.
Emulator: HTTP 400 and no CreateRouteResponse body at all ($.CreateRouteResponse missing).
Why: a client cannot add a route to its own route table — the normal subnet/route-table build-out fails at the first route, so every later step of the trajectory is unreachable.
Fix: the CreateRoute handler's request validation (it rejects a target/destination combination AWS accepts).
Traces: ec2-subnet-route-table-lifecycle#task#10.

Cause 2 of 3: an unknown VPC endpoint target is refused as a gateway, with the wrong error code and noun.
AWS: CreateRoute with VpcEndpointId naming an endpoint that does not exist is refused with HTTP 400 and error code InvalidVpcEndpointId.NotFound. Message template: "The vpcEndpoint ID '<vpcEndpointId>' does not exist" — fixed text outside the quotes, the quoted id being the VpcEndpointId value from the request.
Emulator: HTTP 400 but code InvalidGatewayID.NotFound and message "The gateway ID '<vpcEndpointId>' does not exist" — it resolves VpcEndpointId through the gateway lookup and reports the gateway noun and gateway error code.
Why: a client that branches on the error code (retry/create-the-endpoint vs. fix-the-gateway) takes the wrong branch, and the message names a resource type the caller never supplied.
Fix: the CreateRoute handler's target resolution — VpcEndpointId must be looked up as a VPC endpoint and its not-found path must raise InvalidVpcEndpointId.NotFound.
Traces: ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes#task#4 (cluster also covers #task#5, #task#6 and ec2-pst-traj-detached-igw-leaves-blackhole-default-route#task#16).

Cause 3 of 3: the multiple-target refusal message is emulator-authored.
AWS: CreateRoute naming more than one target is refused with HTTP 400 (the recorded error code matches the emulator's and is not in dispute) and the fixed message "The request must contain exactly one of gatewayId, localGatewayId, carrierGatewayId, natGatewayId, networkInterfaceId, vpcPeeringConnectionId, egressOnlyInternetGatewayId, transitGatewayId, vpcEndpointId, coreNetworkArn or instanceId". The whole message is fixed text — it enumerates the modeled target members and interpolates nothing from the request.
Emulator: message "The parameters transitGatewayId, vpcEndpointId cannot be used together", which names only the two members that happened to be sent.
Why: a client or operator matching AWS's documented refusal text does not recognise the refusal, and the emulator's variant implies the restriction is pairwise rather than exactly-one-of.
Fix: the CreateRoute handler's validation message for conflicting targets.
Traces: ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up).
Cause 1 — ec2-subnet-route-table-lifecycle#task#10, CreateRoute, expected 200 / actual 400: "status: expected 200, got 400" and $.CreateRouteResponse missing (expected {'requestId': '<shaped-not-asserted>', 'return': 'true'}).
Cause 2 — ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes#task#4, CreateRoute, 400/400: $.Response.Errors.Error.Code value_mismatch (expected 'InvalidVpcEndpointId.NotFound', got 'InvalidGatewayID.NotFound'); $.Response.Errors.Error.Message value_mismatch (expected "The vpcEndpoint ID 'vpce-0a1b2c3d4e5f60718' does not exist", got "The gateway ID 'vpce-0a1b2c3d4e5f60718' does not exist"); 4 steps affected.
Cause 3 — ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes#task#9, CreateRoute, 400/400: $.Response.Errors.Error.Message value_mismatch (expected 'The request must contain exactly one of gatewayId, localGatewayId, carrierGatewayId, natGatewayId, networkInterfaceId, vpcPeeringConnectionId, egressOnlyInternetGatewayId, transitGatewayId, vpcEndpointId, coreNetworkArn or instanceId', got 'The parameters transitGatewayId, vpcEndpointId cannot be used together').
Model consulted: smithy_query operation=CreateRoute on services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json (input members, CreateRouteResult.Return, errors: []).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Inspect ec2-subnet-route-table-lifecycle task#10, and ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes task#4 and task#9.
4. The status, error code and message diffs listed under Evidence reproduce deterministically.
