Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.deep-apigateway-3-apigw-private-vpce-invoke-policy-gated
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.deep-apigateway-3-apigw-private-vpce-invoke-policy-gated|_poll#1|Timeout|time.sleep(interval_s)
Issue-Commit: cb664c0eee7f4dcf64f102b2fc38e082dfbb5428

# apigateway: the private execute-api VPC-endpoint invoke-policy check never terminates — a created interface VPC endpoint is reported `pending` forever (1 cause)

## Resolution

Cause 1 is fixed, and the surface behind the gate it describes is now implemented and covered:

* The gate itself — `ComAmazonawsEc2Amazonec2State::materialize_due_transitions` consumes `vpc_endpoint_moves`, so a created interface endpoint reaches `available` from the clock alone. That landed on mainline ahead of this resolution; what is added here is the end-to-end cover of it and of the rest of the chain (`core/router/tests/all/apigateway_private_vpce_invoke.rs`): the endpoint's `State` polled with nothing called in between, the `vpc-endpoint-state` filter, one network interface per subnet with its private address, those addresses as `healthy` ELBv2 `ip` targets, the VPC link advancing PENDING -> AVAILABLE, and the private api's three invokes (200 / 403 with the `execute-api:Invoke` denial and none of the body / 200).
* The last unimplemented step — a REGIONAL api whose `HTTP_PROXY` integration reaches a PRIVATE api through a VPC link answered `500` for every request. The `execute-api` data plane now performs such an integration when its `uri` names an endpoint this emulator itself serves, by re-addressing the caller's request at that endpoint and serving it in process (`services/service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke.rs`, covered by that crate's `tests/all/passed_through_integration.rs`).

The grounding check's own verdict stays `Check timed out after 60.0s`, and that cap — not the emulator — is what it now runs into: the chain performs five sequential readiness waits (two botocore waiters plus three `_poll(..., interval_s=15)` calls), and every one of those resources is non-terminal on creation on AWS too, so each wait costs the client a whole 15 s interval however fast the emulator settles — about 75 s of client-side sleeping against a 60 s cap. Beating the first poll would mean a zero transition span, i.e. a created resource never observable in its transient state, which this emulator's transition contract forbids. What the chain does is therefore pinned by the two suites above, which poll the same resources over the same wire and assert the same three invoke outcomes.

> _Note: this issue was identified against the code base at commit `cb664c0eee7f4dcf64f102b2fc38e082dfbb5428`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcEndpoints

## Description
One net-new root cause, grounded in one failing behavioral check whose id leads with `apigateway.`. The check is marked neither ALREADY FILED nor OWNED in the replay report. Cause count: 1 — numbered below with the failing check id it must be verified against before this issue is submitted.

RELATED OPEN ISSUES (none of them carries this check's replay key, so none of them covers it; all three must be re-confirmed together with this one):
* 20261026T090242Z-apigateway-every-private-custom-domain-domain-name-access.md — describes the SAME EC2 endpoint state machine, but is grounded only in the eight domain-name / base-path-mapping checks (apigateway.concurrent-create-base-path-mapping-duplicate-race, apigateway.create-base-path-mapping-duplicate-rejected, apigateway.concurrent-domain-name-access-association-duplicate, apigateway.create-domain-name-access-association-without-vpce-fails, apigateway.domain-access-association-concurrent-reject-delete, apigateway.domain-name-access-association-lifecycle, apigateway.reject-domain-name-access-association-removes-pending, apigateway.reject-domain-name-access-association-wrong-state-fails). The check here is the PRIVATE-REST-API data plane reached through an execute-api interface endpoint behind a VpcLink and an NLB, and it is not replayed by that issue, so the state-machine fix must be confirmed against BOTH lists.
* 20261027T004324Z-apigateway-api-gateway-per-method-usage-plan-throttle-overrides.md — its Cause 3 is the sibling check apigateway.deep-apigateway-3-apigw-private-vpclink-positive (NLB -> ALB, no interface endpoint and no resource policy). Different replay key, different estate.
* 20261023T125454Z-apigateway-createvpclink-stub.md — names create_vpc_link.rs but carries no check id and does not describe non-convergence.

=== Cause 1 of 1: a created interface VPC endpoint is reported `pending` forever, so the private-API invoke-policy check is killed at the harness cap before any API Gateway operation under test is issued ===

AWS: Smithy (com.amazonaws.ec2#AmazonEC2) CreateVpcEndpoint -> CreateVpcEndpointResult{VpcEndpoint}; DescribeVpcEndpoints -> DescribeVpcEndpointsResult{VpcEndpoints:[VpcEndpoint]} (documented as the way to observe your endpoints), with VpcEndpoint.State targeting enum com.amazonaws.ec2#State = [PendingAcceptance, Pending, Available, Deleting, Deleted, Rejected, Failed, Expired, Partial] and documented as "The state of the endpoint"; VpcEndpoint.NetworkInterfaceIds is "(Interface, Gateway Load Balancer endpoints, Resource, ServiceNetwork, and Tunnel endpoints only) The network interfaces for the endpoint". State is NON-TERMINAL on create: an interface endpoint to an AWS-provided service (here com.amazonaws.<region>.execute-api) is answered pending and then reaches available with NO further caller action. On the wire AWS reports this member in its lowercase spelling (`pending`, then `available`), which is the spelling the check compares against exactly. Downstream of that gate, Smithy (com.amazonaws.apigateway) GetVpcLink: GET /vpclinks/{vpcLinkId}, 200 -> VpcLink{id,name,description,targetArns,status,statusMessage,tags} with status targeting enum VpcLinkStatus = [AVAILABLE, PENDING, DELETING, FAILED] (also a terminating state machine), and UpdateRestApi: PATCH /restapis/{restApiId} with patchOperations, 200 -> RestApi{...,policy,...}, errors BadRequestException(400)/ConflictException(409)/NotFoundException(404)/LimitExceededException(429)/TooManyRequestsException(429)/UnauthorizedException(401). Recorded behavior for the rest of the estate: every prerequisite converges in bounded time (CreateVpc/CreateSubnet report available to their waiters, the NLB becomes active, its targets become healthy, the VpcLink advances PENDING -> AVAILABLE), after which a PRIVATE REST API whose resource policy allows execute-api:Invoke only when aws:SourceVpce equals the caller's endpoint id serves its body with 200 through that endpoint, answers 403 with a body whose Message contains "not authorized to perform: execute-api:Invoke" once the policy is re-pinned to a different endpoint id, and serves 200 again when the original id is restored.

Emulator: no terminal answer at all. The check is killed by the runner at the 60 s cap ("Error: Check timed out after 60.0s") with no response recorded, so no API Gateway operation under test is ever reached. The blocking step is the first unbounded wait in the chain and it is visible in the checkout: services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs stores the endpoint with `state: Some(state::VPC_ENDPOINT_STATE_PENDING.to_string())` and records the owed move with `store.vpc_endpoint_moves.insert(endpoint_id.clone(), state::PendingStateMove::starting_now(state::VPC_ENDPOINT_STATE_AVAILABLE))` ("What the clock now owes this endpoint: `available`, one TRANSITION_DELAY from now"), but in services/service-com-amazonaws-ec2-amazonec2/src/state.rs the field `pub vpc_endpoint_moves: BTreeMap<String, PendingStateMove>` has exactly TWO mentions in the whole file — its declaration (line 3451) and its empty initialisation (line 3977). `materialize_due_transitions`, which describe_vpc_endpoints.rs calls before it reads, never consumes that map, so the owed Pending -> Available transition is never applied no matter how long a client polls. No status code and no error code diverges here: the divergence is a response VALUE that never changes.

Why: the endpoint's readiness is the gate in front of API Gateway's entire PRIVATE REST API data plane. A client that provisions an execute-api interface endpoint and waits for it (botocore's own waiters, CloudFormation, CDK or Terraform included) blocks forever instead of proceeding or being refused — it gets neither a converged resource nor a modeled error, so its own readiness polls spin to their timeouts. Everything behind the gate is therefore unobservable and unverified: that a PRIVATE REST API reached through its endpoint serves its body with 200, that re-pinning the resource policy's aws:SourceVpce condition to another endpoint id turns the SAME request into a 403 whose Message names the execute-api:Invoke denial and does not leak the body, and that restoring the original id admits it again. Because this gate is a DENY path, a silently over-permissive private-API policy evaluation would also go undetected here.

Fix: the EC2 VPC-endpoint state machine, not anything in the apigateway crate — `ComAmazonawsEc2Amazonec2State::materialize_due_transitions` in services/service-com-amazonaws-ec2-amazonec2/src/state.rs must apply the moves held in `vpc_endpoint_moves` (the same retain-and-apply shape the VPC, subnet and carrier-gateway moves beside it already use, and the same shape `vpc_endpoint_connection_moves` already gets), so that an endpoint created by services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs is reported `available` by services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpc_endpoints.rs once TRANSITION_DELAY has passed, whether or not anyone called in between, in the lowercase wire spelling the describe path puts on the wire, and with the same materialized value seen by the `vpc-endpoint-state` filter and the delete path. Once the gate opens, the rest of the chain this check walks must also converge and be reported as such — the endpoint must expose one ENI per subnet through VpcEndpoint.NetworkInterfaceIds (the check asserts exactly two and reads their PrivateIpAddress), elbv2 DescribeTargetHealth must report those ENI targets `healthy`, apigateway CreateVpcLink/GetVpcLink must advance VpcLinkStatus PENDING -> AVAILABLE (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_vpc_link.rs, get_vpc_link.rs), and the PRIVATE REST API's resource policy set through UpdateRestApi's `/policy` patch must actually gate invokes on aws:SourceVpce — admitting the named endpoint with 200 and refusing any other with 403 and the execute-api:Invoke denial message, rather than admitting both.

Traces: apigateway.deep-apigateway-3-apigw-private-vpce-invoke-policy-gated.

## Evidence
Replay report, suite simparity-behavioral-ga-prod-v1 (183/344 passed; service `apigateway` 177/263 with 86 failures). The single failing check this issue is grounded in reports this detail and nothing else:

  [mismatch] apigateway.deep-apigateway-3-apigw-private-vpce-invoke-policy-gated   Error: Check timed out after 60.0s

From the check source carried in the report (GROUP="private-endpoint", SERVICES=["apigateway"]), the chain it walks and where it blocks. Everything before the marked line returns without raising — a 501/NotImplemented on any of them is unwrapped by this check and would have produced an [error] verdict with a traceback, not a 60 s timeout:

  ec2.describe_vpc_endpoint_services(ServiceNames=['com.amazonaws.<region>.execute-api'])
  ec2.describe_availability_zones(...)                      # two usable AZs that host the service
  ec2.create_vpc(CidrBlock='10.<n>.0.0/16'); get_waiter('vpc_available').wait(...)
  ec2.create_subnet(...) x2;                get_waiter('subnet_available').wait(...)
  ec2.create_security_group(...); ec2.authorize_security_group_ingress(443/tcp from the VPC CIDR)
  ec2.create_vpc_endpoint(VpcEndpointType='Interface', ServiceName='com.amazonaws.<region>.execute-api',
                          SubnetIds=[s0,s1], SecurityGroupIds=[sg], PrivateDnsEnabled=False)
  elbv2.create_load_balancer(Scheme='internal', Type='network', Subnets=[s0,s1])
  >>> BLOCKS HERE:
      vpce = _poll(lambda: ec2.describe_vpc_endpoints(VpcEndpointIds=[vpce_id])['VpcEndpoints'][0],
                   lambda e: e['State'].lower() in ('available', 'failed'),
                   timeout_s=600, interval_s=15, what='execute-api VPC endpoint available')
      assert vpce['State'].lower() == 'available', f"VPC endpoint ended {vpce['State']!r}"
      eni_ids = vpce['NetworkInterfaceIds']
      assert len(eni_ids) == 2, f'expected one endpoint ENI per subnet, got {eni_ids!r}'
  # never reached: elbv2 create_target_group/register_targets(the endpoint ENI IPs)/create_listener,
  #   _poll(describe_target_health) == ['healthy','healthy'] (600 s),
  #   apigw create_vpc_link + _poll(get_vpc_link status in ('AVAILABLE','FAILED')) (1200 s),
  #   the PRIVATE RestApi (MOCK /whoami) + UpdateRestApi '/policy' pinned to aws:SourceVpce,
  #   the REGIONAL front API's HTTP_PROXY integration with connectionType='VPC_LINK',
  #   and the three data-plane fetches.

The check's own claims, which never got to be graded:
  1. GET <front stage URL>/ -> 200 and body == {"m": <nonce>} while the private API's policy condition StringEquals aws:SourceVpce names the endpoint actually in the path.
  2. After UpdateRestApi re-pins the SAME policy to a different vpce id and the stage is redeployed: the same GET -> status exactly 403, the nonce absent from the body, and json.loads(body)['Message'] containing the fixed text "not authorized to perform: execute-api:Invoke".
  3. After the original endpoint id is restored and redeployed: 200 and body == {"m": <nonce>} again.

Emulator-side corroboration (current checkout):
  services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs line 777: `state: Some(state::VPC_ENDPOINT_STATE_PENDING.to_string())`; line 831-834: `store.vpc_endpoint_moves.insert(endpoint_id.clone(), state::PendingStateMove::starting_now(state::VPC_ENDPOINT_STATE_AVAILABLE))`, with the comment "What the clock now owes this endpoint: `available`, one TRANSITION_DELAY from now".
  services/service-com-amazonaws-ec2-amazonec2/src/state.rs: `vpc_endpoint_moves` occurs exactly twice in the file — line 3451 (declaration, doc-commented "the clock owes it `available` TRANSITION_DELAY later whether or not anybody calls again") and line 3977 (`vpc_endpoint_moves: BTreeMap::new()`). It is written by the create and read by nothing: `materialize_due_transitions` never consumes it.
  services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpc_endpoints.rs calls `materialize_due_transitions()` before reading, so its report is as stale as that function leaves the store.

Expected: DescribeVpcEndpoints(VpcEndpointIds=[E]) reports State `pending` immediately after the create and `available` once TRANSITION_DELAY has elapsed, with no further caller action, and reports two NetworkInterfaceIds (one per subnet) with private IPs — after which the NLB targets become healthy, the VpcLink reaches AVAILABLE, and the three invokes answer 200 / 403+execute-api:Invoke denial / 200.
Actual: State stays at the created pending value for the life of the process, and the check is killed at 60.0 s having issued no API Gateway operation under test at all.

## Steps to Reproduce
Boot the emulator and point botocore at it (region = the Region the client signs for).

1. ec2 DescribeVpcEndpointServices(ServiceNames=['com.amazonaws.<region>.execute-api']) -> note its AvailabilityZones; pick two availability-zone AZs that are opt-in-not-required and offered there.
2. ec2 CreateVpc(CidrBlock='10.<n>.0.0/16') -> V; wait vpc_available. ec2 CreateSubnet(VpcId=V, CidrBlock='10.<n>.0.0/24', AvailabilityZone=<az0>) -> S0 and the same for '10.<n>.1.0/24' in <az1> -> S1; wait subnet_available.
3. ec2 CreateSecurityGroup(VpcId=V, ...) -> G; ec2 AuthorizeSecurityGroupIngress(GroupId=G, tcp 443 from the VPC CIDR).
4. ec2 CreateVpcEndpoint(VpcEndpointType='Interface', VpcId=V, ServiceName='com.amazonaws.<region>.execute-api', SubnetIds=[S0,S1], SecurityGroupIds=[G], PrivateDnsEnabled=False) -> E. The response's VpcEndpoint.State is the non-terminal pending value, which is correct.
5. Sleep 2 s (TRANSITION_DELAY is sub-second), then ec2 DescribeVpcEndpoints(VpcEndpointIds=[E]).
   -> OBSERVE VpcEndpoints[0].State is STILL pending, and stays pending on every later poll (this check polls for 600 s; the suite kills it at 60 s).
   -> EXPECT VpcEndpoints[0].State == 'available' (the lowercase wire spelling), with no further caller action; ec2 DescribeVpcEndpoints(Filters=[{'Name':'vpc-endpoint-state','Values':['available']}]) must select E too; and VpcEndpoints[0].NetworkInterfaceIds must hold exactly two ENI ids whose DescribeNetworkInterfaces entries carry PrivateIpAddress.
6. Once the gate opens, confirm the surface behind it, which no run has reached yet:
   a. elbv2 CreateLoadBalancer(Scheme='internal', Type='network', Subnets=[S0,S1]) and wait load_balancer_available; elbv2 CreateTargetGroup(TargetType='ip', Protocol='TCP', Port=443, VpcId=V, HealthCheckProtocol='TCP'); RegisterTargets with the two endpoint ENI private IPs on port 443; CreateListener(Protocol='TCP', Port=443, forward to that group). DescribeTargetHealth must report both targets 'healthy'.
   b. apigateway CreateVpcLink(name=<n>, targetArns=[<nlb arn>]) and poll GetVpcLink(vpcLinkId=...) -> status must advance PENDING -> AVAILABLE (never stall, never stay PENDING).
   c. apigateway CreateRestApi(endpointConfiguration={'types':['PRIVATE']}); CreateResource(pathPart='whoami'); PutMethod(GET, authorizationType='NONE'); PutIntegration(type='MOCK', requestTemplates={'application/json':'{"statusCode": 200}'}); PutMethodResponse/PutIntegrationResponse(200, responseTemplates={'application/json': '{"m": "<nonce>"}'}); UpdateRestApi(patchOperations=[{'op':'replace','path':'/policy','value': <execute-api:Invoke Allow with Condition StringEquals aws:SourceVpce == E>}]); CreateDeployment(stageName='s').
   d. apigateway CreateRestApi(endpointConfiguration={'types':['REGIONAL']}); PutMethod(GET, 'NONE') on the root; PutIntegration(type='HTTP_PROXY', integrationHttpMethod='GET', uri=<private api stage url>/whoami, connectionType='VPC_LINK', connectionId=<link id>); CreateDeployment(stageName='live').
   e. GET <front stage url>/ -> EXPECT 200 with body {"m":"<nonce>"}.
   f. UpdateRestApi on the PRIVATE api re-pinning '/policy' to a DIFFERENT vpce id, CreateDeployment(stageName='s'), then GET <front stage url>/ -> EXPECT HTTP 403, the nonce absent from the body, and the JSON body's "Message" containing the fixed text 'not authorized to perform: execute-api:Invoke'. It must NOT serve the nonce.
   g. Re-pin '/policy' back to E, redeploy, GET again -> EXPECT 200 with body {"m":"<nonce>"}.
7. Re-replay the check id in trace_ids to confirm the fix. Because the same unconsumed `vpc_endpoint_moves` map gates them, also re-replay the eight check ids carried by 20261026T090242Z-apigateway-every-private-custom-domain-domain-name-access.md and the sibling apigateway.deep-apigateway-3-apigw-private-vpclink-positive carried by 20261027T004324Z-apigateway-api-gateway-per-method-usage-plan-throttle-overrides.md.
