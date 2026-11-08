Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.create-portal-access-denied-without-permission apigatewayv2.create-route-response-invalid-route-rejected apigatewayv2.integration-response-key-format-validation apigatewayv2.vpc-link-mismatched-subnet-vpcs-rejected
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.create-portal-access-denied-without-permission|run#7|AssertionError|assert code == "AccessDeniedException", (
Issue-Key: apigatewayv2.create-route-response-invalid-route-rejected|run#9|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Key: apigatewayv2.integration-response-key-format-validation|run#8|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: apigatewayv2.vpc-link-mismatched-subnet-vpcs-rejected|run#6|AssertionError|assert code == "BadRequestException", (
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# apigatewayv2: four negative-path control-plane behaviours are unreachable because CreatePortal, CreateRouteResponse, CreateIntegrationResponse and CreateVpcLink answer NotImplemented (4 causes)

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePortal

## Description
Four apigatewayv2 behavioral checks fail because the operation they exercise is still an unimplemented scaffold stub that answers the off-model error code 'NotImplemented' instead of the operation's modelled success or rejection. None of these four checks was marked ALREADY FILED or OWNED by the replay report, so no open issue carries their replay keys; the closest open issues are the per-operation scaffold issues 20261023T125319Z-apigatewayv2-createportal-stub.md, 20261023T114649Z-apigatewayv2-createrouteresponse-stub.md, 20261023T114649Z-apigatewayv2-createintegrationresponse-stub.md and 20261023T125319Z-apigatewayv2-createvpclink-stub.md, which name the handlers but do not name these checks or the specific semantics below. Four causes, numbered.

Cause 1 of 4: CreatePortal does not authorize, so an explicitly denied principal is not refused with AccessDeniedException.
AWS: Smithy model (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model) CreatePortal -> POST /v2/portals, success code 201, modelled errors AccessDeniedException (403), BadRequestException (400), TooManyRequestsException (429). A principal carrying an explicit Deny on apigateway:CreatePortal must be refused with AccessDeniedException (HTTP 403) and no portal must be created, while an action the Deny does not name (ListPortals) must still succeed on the same session, and an unrestricted principal must succeed and have the portal readable via GetPortal. The recorded message is of the documented AWS authorization form 'User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: apigateway:CreatePortal on resource: arn:aws:apigateway:<region>:<account>:/portals/* with an explicit deny in an identity-based policy' — that message rule (built from the request principal, action and resource) is the requirement; the literal recorded text is an example.
Emulator: CreatePortal answers error code 'NotImplemented' ('operation CreatePortal is scaffolded but not implemented in this emulator') for every caller, restricted or not.
Why: a client cannot distinguish an authorization failure from an unimplemented API; IAM-policy regression tests and portal provisioning both break, and the unrestricted positive-control path cannot create a portal at all.
Fix: the CreatePortal handler and its authorization/validation path (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_portal.rs) plus portal state for ListPortals/GetPortal.
Traces: apigatewayv2.create-portal-access-denied-without-permission.

Cause 2 of 4: CreateRouteResponse against a RouteId that does not exist on the API is not refused with NotFoundException.
AWS: Smithy CreateRouteResponse -> POST /v2/apis/{ApiId}/routes/{RouteId}/routeresponses, success 201, modelled errors BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429). A RouteId never created on the API must be rejected with NotFoundException (HTTP 404), no route response must be created, and the same call against the real route must succeed and be readable via GetRouteResponse with RouteResponseKey '$default'. The recorded message 'Invalid route identifier specified' is an example of the 404 body.
Emulator: CreateRouteResponse answers error code 'NotImplemented', so neither the rejection nor the accepted control path exists.
Why: WebSocket two-way routes cannot be configured, and a client that treats 404 as 'route id typo' sees an unmodelled error code instead.
Fix: the CreateRouteResponse handler and its route-id existence validation (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_route_response.rs) plus route-response state for GetRouteResponse/GetRouteResponses.
Traces: apigatewayv2.create-route-response-invalid-route-rejected.

Cause 3 of 4: CreateIntegrationResponse does not validate IntegrationResponseKey, so neither of the two IntegrationResponseKey grammar rejections exists.
AWS: Smithy CreateIntegrationResponse -> POST /v2/apis/{ApiId}/integrations/{IntegrationId}/integrationresponses, success 201, modelled errors BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429); IntegrationResponseKey targets SelectionKey and is required. Recorded behavior: the field is validated in two independent stages for a non-Lambda (MOCK) integration — a value that is neither '$default' nor wrapped '/like/this/' is rejected with BadRequestException (400) naming the literal value (example message: "IntegrationResponseKey must be a regular expression pattern of format '/<pattern>/' or have a value of $default. Current value is badkey"), and a correctly wrapped key whose inner pattern is not status-code shaped is rejected by a second, more specific BadRequestException naming 'non-Lambda Integration'; a wrapped, status-code-shaped key such as '/2\\d\\d/' is accepted. The requirement is the 400/BadRequestException verdict per stage and the acceptance of a status-code-shaped key; the exact strings are examples.
Emulator: CreateIntegrationResponse answers error code 'NotImplemented' for every key, valid or invalid.
Why: a client gets an unmodelled code for a request AWS accepts and for one AWS rejects, so MOCK/WebSocket integration-response configuration and its negative tests are both unusable.
Fix: the CreateIntegrationResponse handler and its IntegrationResponseKey validation (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_integration_response.rs).
Traces: apigatewayv2.integration-response-key-format-validation.

Cause 4 of 4: CreateVpcLink does not validate that all SubnetIds belong to one VPC.
AWS: Smithy CreateVpcLink -> POST /v2/vpclinks, success 201, modelled errors BadRequestException (400), TooManyRequestsException (429); SubnetIds targets SubnetIdList and is required. SubnetIds drawn from two different VPCs must be rejected with BadRequestException (HTTP 400) and no VpcLink created (example message: 'All subnets provided should be in the same VPC'); a single-VPC subnet list must succeed and the link must be readable via GetVpcLink with the submitted Name and SubnetIds.
Emulator: CreateVpcLink answers error code 'NotImplemented' for both the mismatched-VPC request and the valid one.
Why: private-integration provisioning cannot be modelled at all, and a client cannot tell a cross-VPC input error from a missing API.
Fix: the CreateVpcLink handler and its subnet/VPC cross-validation (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_vpc_link.rs) plus VpcLink state for GetVpcLink/GetVpcLinks.
Traces: apigatewayv2.vpc-link-mismatched-subnet-vpcs-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 (4/225 passed).

Cause 1 — apigatewayv2.create-portal-access-denied-without-permission [mismatch]: AssertionError: CreatePortal must be rejected with AccessDeniedException for a principal carrying an explicit Deny on apigateway:CreatePortal; got 'NotImplemented'. Expected: error code AccessDeniedException, HTTP 403. Actual: error code NotImplemented, message 'operation CreatePortal is scaffolded but not implemented in this emulator'.

Cause 2 — apigatewayv2.create-route-response-invalid-route-rejected [mismatch]: AssertionError: expected NotFoundException, got 'NotImplemented'. Expected: error code NotFoundException, HTTP 404 for a bogus RouteId, then a 201 success for the real RouteId. Actual: NotImplemented for both.

Cause 3 — apigatewayv2.integration-response-key-format-validation [mismatch]: AssertionError: expected BadRequestException, got 'NotImplemented'. Expected: error code BadRequestException, HTTP 400 for IntegrationResponseKey='badkey'. Actual: NotImplemented.

Cause 4 — apigatewayv2.vpc-link-mismatched-subnet-vpcs-rejected [mismatch]: AssertionError: expected BadRequestException for SubnetIds drawn from two different VPCs, got 'NotImplemented'. Expected: error code BadRequestException, HTTP 400. Actual: NotImplemented.

None of these four checks carried an 'ALREADY FILED' or 'OWNED by' marker in the replay report.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a botocore apigatewayv2 client at it.
2. Cause 1: assume a role carrying an explicit Deny on apigateway:CreatePortal and call CreatePortal(Authorization={'None':{}}, EndpointConfiguration={'None':{}}, PortalContent=...). Expect AccessDeniedException/403; observe NotImplemented. Then call CreatePortal with an unrestricted principal and expect 201.
3. Cause 2: CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action'), CreateRoute(RouteKey='$default'), then CreateRouteResponse(ApiId=api, RouteId='bogus...', RouteResponseKey='$default'). Expect NotFoundException/404; observe NotImplemented. Repeat with the real RouteId and expect 201.
4. Cause 3: on a WEBSOCKET API, CreateIntegration(IntegrationType='MOCK'), then CreateIntegrationResponse(IntegrationResponseKey='badkey'). Expect BadRequestException/400; observe NotImplemented. Repeat with '/notastatus/' (expect a second, distinct 400) and '/2\\d\\d/' (expect 201).
5. Cause 4: create two VPCs each with a subnet, then CreateVpcLink(Name=..., SubnetIds=[subnet_in_vpc_a, subnet_in_vpc_b]). Expect BadRequestException/400; observe NotImplemented. Repeat with a single-VPC subnet list and expect 201 plus a matching GetVpcLink.
6. Re-replay the four check ids in trace_ids.
