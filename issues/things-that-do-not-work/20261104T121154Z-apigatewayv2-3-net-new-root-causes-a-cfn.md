Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deploy-vpc-link-cfn-behaves apigatewayv2.http-proxy-payload-format-version-2-rejected apigatewayv2.e2e-vpc-link-private-integration-flow apigatewayv2.e2e-vpc-link-private-integration-invoke
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|GetVpcLink|NotFoundException
Issue-Key: apigatewayv2.http-proxy-payload-format-version-2-rejected|run#10|AssertionError|assert message == PFV2_HTTP_PROXY_MESSAGE, (
Issue-Key: apigatewayv2.e2e-vpc-link-private-integration-flow|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.e2e-vpc-link-private-integration-invoke|_poll_http_get#20|Timeout|time.sleep(interval_s)
Issue-Commit: 08672d49945dcbdaeb30ca89916c398a9fb8e682

# apigatewayv2: 3 net-new root causes — a CFN-created AWS::ApiGatewayV2::VpcLink is unknown to GetVpcLink, the HTTP_PROXY PayloadFormatVersion refusal is emulator-authored, and a VpcLink-backed private integration never serves

> _Note: this issue was identified against the code base at commit `08672d49945dcbdaeb30ca89916c398a9fb8e682`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVpcLink

## Description
Three distinct root causes in the apigatewayv2 emulator, all grounded in failing behavioral checks of this service. A fixer must close ALL THREE before submitting.

=== Cause 1 of 3: a CloudFormation-created AWS::ApiGatewayV2::VpcLink exists only as a stack output — apigatewayv2 never holds the link, so GetVpcLink refuses its own !Ref id with 404 NotFoundException ===
Failing check id: apigatewayv2.deploy-vpc-link-cfn-behaves

AWS: a stack declaring `Type: AWS::ApiGatewayV2::VpcLink` provisions a real VPC link through CreateVpcLink; `!Ref` of that logical resource returns the link's VpcLinkId, and GetVpcLink on that id answers 200 with the modeled VpcLink envelope — VpcLinkStatus moves PENDING -> AVAILABLE (Smithy: GetVpcLink output `VpcLinkStatus` = AVAILABLE | DELETING | FAILED | INACTIVE | PENDING; `VpcLinkId` (Id); GET /v2/vpclinks/{VpcLinkId}, success 200). GetVpcLink's modeled NotFoundException (HTTP 404) is documented as "The resource specified in the request was not found" — it is reserved for an id the account does not hold, so a CFN-minted id must never produce it.

Emulator: the stack create succeeds and its Outputs carry a VpcLinkId, but the immediately following GetVpcLink on exactly that id is refused: HTTP 404, error code `NotFoundException`, message `Invalid Vpc Link identifier specified` (the whole sentence is fixed text in this emulator; it carries no value from the request). So the CloudFormation resource provider fabricates a physical resource id instead of creating a record in the apigatewayv2 store — the hand-written path `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_vpc_link.rs` does insert into `state.vpc_links` and `get_vpc_link.rs` reads that same map, so the stack is plainly not going through it.

Why: a client that deploys its private-integration estate by template gets a stack id it can never use. Every subsequent call keyed by that id (GetVpcLink, DeleteVpcLink, CreateIntegration with ConnectionType=VPC_LINK/ConnectionId=<ref>) fails or attaches to nothing, and a waiter polling for AVAILABLE raises on the first poll instead of converging. The stack also lies: it reports CREATE_COMPLETE for a resource that does not exist.

Fix: the CloudFormation resource provider for `AWS::ApiGatewayV2::VpcLink` must create the link through the same state the apigatewayv2 operations own (the store behind `crate::state::service_state`, as `ops/create_vpc_link.rs` does) and return that minted VpcLinkId as the physical resource id / `!Ref` value; GetVpcLink's handler itself is not at fault.

=== Cause 2 of 3: the HTTP_PROXY + PayloadFormatVersion 2.0 refusal message is emulator-authored prose ===
Failing check id: apigatewayv2.http-proxy-payload-format-version-2-rejected

AWS: PayloadFormatVersion 2.0 is legal only for Lambda proxy (AWS_PROXY) integrations — the model states it: "Supported values for Lambda proxy integrations are 1.0 and 2.0. For all other integrations, 1.0 is the only supported value" (Smithy: CreateIntegration$PayloadFormatVersion / UpdateIntegration$PayloadFormatVersion, target StringWithLengthBetween1And64). Both CreateIntegration and UpdateIntegration refuse the combination with HTTP 400 `BadRequestException` and the message `PayloadFormatVersion 2.0 is not supported for integration of type HTTP_PROXY`. In that sentence the fixed text is "PayloadFormatVersion " … " is not supported for integration of type "; the two variable parts are the PayloadFormatVersion the request sent and the IntegrationType the integration has (here "2.0" and "HTTP_PROXY") — not literals to hard-code.

Emulator: the status (400) and the error code (BadRequestException) are right, but the message is `Unsupported PayloadFormatVersion: 2.0` — it drops AWS's wording entirely and never names the integration type that made the value illegal. The check asserts the same sentence on CreateIntegration and on UpdateIntegration; the create assertion fired first, and the shared validator means the update path carries the identical text.

Why: a client that keys on the refusal text (log triage, user-facing diagnostics, tests pinning the contract) cannot tell which member/type combination was rejected, and anything matching AWS's sentence does not match at all.

Fix: the shared integration validator `crate::state::validate_integration` in `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/state.rs` (called from `ops/create_integration.rs` and `ops/update_integration.rs`) must emit AWS's sentence, interpolating the requested PayloadFormatVersion and the integration's IntegrationType.

=== Cause 3 of 3: a VpcLink-backed HTTP_PROXY private integration never delivers, so both end-to-end private-integration checks reach no verdict ===
Failing check ids: apigatewayv2.e2e-vpc-link-private-integration-flow, apigatewayv2.e2e-vpc-link-private-integration-invoke

AWS: an HTTP API route whose integration is `IntegrationType=HTTP_PROXY`, `ConnectionType=VPC_LINK`, `ConnectionId=<vpc link id>`, `IntegrationUri=<ALB/NLB listener ARN>`, PayloadFormatVersion 1.0, fronted by a deployed (AutoDeploy) stage, proxies a request arriving at the API's public invoke endpoint through the VPC link to the private load-balancer target and returns the backend's status and body to the caller. Recorded behavior only for the exact timings: the checks poll the public endpoint until the backend's own body appears, and on AWS that poll converges.

Emulator: neither check ever settles — both are reported as `Check timed out after 60.0s`, i.e. the polled GET of `<ApiEndpoint>/<stage>/<path>` never once returned the backend body, so no assertion in either check was ever reached and the private-integration contract is wholly unobservable. Note for the fixer: the open issue 20261024T120951Z-apigatewayv2-the-execute-api-data-plane-never-serves.md describes the execute-api HTTP invoke path not serving and may share a cause; the replay report did NOT mark these two checks as owned by it, so they are filed here and must be re-replayed in their own right.

Why: a client cannot use API Gateway v2 private integrations at all — the whole point of a VPC link (reaching a VPC-internal backend from a public HTTP API) produces no response, and because the call never answers, a client blocks on its own read/poll deadline rather than receiving an error it could handle.

Fix: the execute-api data plane's integration dispatch for HTTP_PROXY with ConnectionType=VPC_LINK — it must resolve the ConnectionId to the stored VpcLink, resolve the listener ARN in IntegrationUri to the backing load-balancer target, forward the request, and return the backend's response to the caller (and answer, rather than hang, when it cannot).

## Evidence
Cause 1 (apigatewayv2.deploy-vpc-link-cfn-behaves):
  expected: GetVpcLink(VpcLinkId=<stack Outputs.VpcLinkId>) -> HTTP 200, VpcLinkStatus eventually "AVAILABLE", and the link usable as ConnectionId of an HTTP_PROXY/VPC_LINK integration.
  actual:   botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the GetVpcLink operation: Invalid Vpc Link identifier specified  (HTTP 404) — raised on the first poll, for the id the stack itself returned via !Ref.

Cause 2 (apigatewayv2.http-proxy-payload-format-version-2-rejected):
  expected: CreateIntegration(ApiId=<http api>, IntegrationType="HTTP_PROXY", IntegrationUri="https://...", IntegrationMethod="GET", PayloadFormatVersion="2.0") -> HTTP 400, Code=BadRequestException, Message="PayloadFormatVersion 2.0 is not supported for integration of type HTTP_PROXY" (same sentence for the equivalent UpdateIntegration patch).
  actual:   HTTP 400, Code=BadRequestException (correct), Message='Unsupported PayloadFormatVersion: 2.0'.
  assertion: AssertionError: expected the exact HTTP_PROXY PayloadFormatVersion refusal on create, got 'Unsupported PayloadFormatVersion: 2.0'

Cause 3 (apigatewayv2.e2e-vpc-link-private-integration-flow, apigatewayv2.e2e-vpc-link-private-integration-invoke):
  expected: a GET of <ApiEndpoint>/<stage>/<path> on an HTTP API whose route targets an HTTP_PROXY integration with ConnectionType=VPC_LINK returns the private backend's body within the poll window.
  actual:   'Check timed out after 60.0s' for both checks — the polled request never produced the backend body, so neither check reached any assertion and produced no verdict at all.

## Steps to Reproduce
Cause 1:
1. Deploy a CloudFormation stack containing only:
     Resources: MyVpcLink: {Type: AWS::ApiGatewayV2::VpcLink, Properties: {Name: <name>, SubnetIds: <two subnet ids in distinct AZs>}}
     Outputs:   VpcLinkId: {Value: !Ref MyVpcLink}
2. Read the stack output VpcLinkId.
3. Call apigatewayv2 GetVpcLink(VpcLinkId=<that value>) — observe HTTP 404 NotFoundException 'Invalid Vpc Link identifier specified' instead of a 200 envelope reaching VpcLinkStatus=AVAILABLE.

Cause 2:
1. CreateApi(Name=<name>, ProtocolType="HTTP").
2. CreateIntegration(ApiId=<id>, IntegrationType="HTTP_PROXY", IntegrationUri="https://example.com/backend", IntegrationMethod="GET", PayloadFormatVersion="1.0") — succeeds.
3. CreateIntegration(... same api, IntegrationType="HTTP_PROXY", PayloadFormatVersion="2.0") — observe 400 BadRequestException with message 'Unsupported PayloadFormatVersion: 2.0' instead of 'PayloadFormatVersion 2.0 is not supported for integration of type HTTP_PROXY'.
4. UpdateIntegration(ApiId=<id>, IntegrationId=<the 1.0 integration>, PayloadFormatVersion="2.0") — same wrong message.

Cause 3:
1. Create two AZ-distinct subnets plus a security group, an internal load balancer with a listener, and a backend target that answers a known marker body.
2. CreateVpcLink(Name=<name>, SubnetIds=[...], SecurityGroupIds=[...]) and poll GetVpcLink until VpcLinkStatus=AVAILABLE.
3. CreateApi(ProtocolType="HTTP"); CreateIntegration(IntegrationType="HTTP_PROXY", IntegrationMethod="ANY", ConnectionType="VPC_LINK", ConnectionId=<vpc link id>, IntegrationUri=<listener ARN>, PayloadFormatVersion="1.0"); CreateRoute(RouteKey="ANY /proxy" or "ANY /{proxy+}", Target="integrations/<id>"); CreateStage(StageName="$default" or "prod", AutoDeploy=True).
4. GET <ApiEndpoint>[/<stage>]/proxy repeatedly — the backend marker body never appears and the check burns its 60 s cap with no response.
