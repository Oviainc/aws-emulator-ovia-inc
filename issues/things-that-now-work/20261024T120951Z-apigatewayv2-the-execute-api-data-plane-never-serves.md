Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.create-vpc-link-invalid-subnet-bad-request apigatewayv2.vpc-link-mismatched-subnet-vpcs-rejected apigatewayv2.domain-name-empty-configuration-rejected apigatewayv2.get-routes-pagination-chains-every-route-once apigatewayv2.http-proxy-stage-variable-selects-backend-host apigatewayv2.reset-authorizers-cache-invalid-stage-rejected apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected
Issue-Key: apigatewayv2.create-vpc-link-invalid-subnet-bad-request|_error_code#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: apigatewayv2.vpc-link-mismatched-subnet-vpcs-rejected|_error_code#5|AssertionError|raise AssertionError(f"expected CreateVpcLink to be rejected, but it succeeded: {result!r}")
Issue-Key: apigatewayv2.domain-name-empty-configuration-rejected|_error_code#3|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: apigatewayv2.get-routes-pagination-chains-every-route-once|_error#20|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: apigatewayv2|CreateStage|BadRequestException
Issue-Key: apigatewayv2.reset-authorizers-cache-invalid-stage-rejected|run#9|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Key: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys|run#37|KeyError|outputs = {o["OutputKey"]: o["OutputValue"]
Issue-Key: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected|run#18|KeyError|outputs = {o["OutputKey"]: o["OutputValue"]
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# apigatewayv2: the execute-api data plane never serves (HTTP invoke and WebSocket upgrade), plus 6 control-plane fidelity defects (8 causes)

> _RESOLVED IN PART, AND SPLIT. Causes 3 to 8 are closed: `CreateVpcLink` resolves every
> `SubnetIds` entry against the service that owns subnets and requires them to agree on one VPC
> (`services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/state/ec2_subnets.rs`,
> `tests/all/vpc_link_subnet_resolution.rs`); `ResetAuthorizersCache` is implemented — 204 for a
> stage this service holds, `NotFoundException` 404 for an `ApiId`/`StageName` that resolves to
> nothing (`src/ops/reset_authorizers_cache.rs`, `tests/all/authorizers_cache_reset.rs`); a stage
> variable whose value lies inside the documented `[A-Za-z0-9-._~:/?#&=,]+` set is accepted,
> including the host-and-path of the invoke URL this control plane announces, and one outside it
> is refused (cause 6 was already correct and is now pinned by
> `tests/all/stage_variable_values.rs`); `CreateDomainName` refuses an absent or empty
> `DomainNameConfigurations` with `BadRequestException` 400
> (`tests/all/domain_name_endpoint_required.rs`); `GetRoutes` refuses a `NextToken` it never
> issued with `BadRequestException` 400 "Invalid position parameter"
> (`src/state/paging.rs`); and a completed stack's `Outputs` entries carry an `OutputValue`
> (`services/service-com-amazonaws-cloudformation-cloudformation/tests/all/provisioned_resources.rs`,
> `stack_outputs.rs`, `stack_output_resolution.rs`). The `Issue-Trace-Ids` and `Issue-Key` lines
> above are those causes' own: causes 1 and 2's 35 keys are NOT here, they live only in the split
> report named below, so nothing still failing is recorded as working._
>
> _Causes 1 and 2 — the HTTP invoke data plane and the RFC 6455 upgrade — are NOT closed. Both are
> blocked in the vendored, read-only path: the only serving seam,
> `vendor/smithy-server-runtime/src/service.rs:390`
> (`override_routing_error(&self, &RtRequest) -> Option<RtResponse>`), is synchronous, buffered and
> handed no callback, so neither a hop to the backend an integration names nor the handover of an
> upgraded connection can happen there, and
> `vendor/smithy-server-runtime/src/multi_service_router.rs:590-593` strips `connection` and
> `upgrade` from every response. Their replay keys are re-filed, with that analysis, as
> `issues/things-that-do-not-work/20261024T120951Z-apigatewayv2-the-execute-api-http-and-websocket-data-planes-remain-open.md`,
> so they are not recorded here as working; the same core gap is also carried by
> `20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md` and
> `20261025T030504Z-apigatewayv2-lambda-an-aws-proxy-route-cannot-answer-200-until-a.md`._

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane), CreateVpcLink, CreateDomainName, GetRoutes, CreateStage, ResetAuthorizersCache

## Description
Eight distinct root causes in the apigatewayv2 emulator, all grounded in failing behavioral checks of this suite. A fixer must close ALL EIGHT before submitting; they are numbered below and each carries its own failing check ids.

=== Cause 1 of 8: an HTTP API with a route, an integration and a stage never answers on its own invoke URL ===
AWS: recorded behavior only (data plane; not modeled in Smithy). After CreateApi(ProtocolType=HTTP) + CreateIntegration + CreateRoute + CreateStage(AutoDeploy=true, or a stage created with an explicit DeploymentId), a GET of the stage's own invoke URL (the one the target itself announces for that api id and stage) is served by API Gateway: HTTP 200 with the integration's response for a matching route key, and the documented error statuses (403/404/413/429/503) for the negative legs the checks exercise.
Emulator: no invoke ever reaches 200. Every one of these checks polls the invoke URL the emulator announced for the API it just created (a 90 s poll loop with 3 s sleeps) and never gets the expected status, so the check runs past the suite's 60 s cap and is reported as `Check timed out after 60.0s`. The checks carry an explicit `DATAPLANE_UNIMPLEMENTED` marker and their own prose for exactly this outcome: "nothing answered at the HTTP API endpoint ... the execute-api data plane is absent, which is a fidelity failure and not an env error".
Why: everything a client does with an HTTP API after creating it is invoke traffic — Lambda proxy and HTTP proxy integrations, payload format 1.0/2.0 event shaping, CORS preflight, parameter mapping, authorizer gating and caching, stage variables, route throttling, binary bodies. All of it is unobservable, so an emulator that only bookkeeps the control plane looks correct to control-plane assertions and serves nothing.
Fix: the execute-api data-plane listener for HTTP APIs — route-key matching, stage resolution, integration dispatch and response shaping — and the invoke-URL host/path the control plane announces for it.
Traces: apigatewayv2.create-stage-and-deployment, apigatewayv2.invoke-http-api-httpproxy, apigatewayv2.invoke-http-api-lambda-proxy, apigatewayv2.delete-stage-stops-serving-requests, apigatewayv2.update-route-target-switch-takes-effect-on-autodeploy, apigatewayv2.http-api-cors-configuration-overrides-backend-cors-headers, apigatewayv2.http-api-cors-preflight-caught-by-any-route-is-authorized, apigatewayv2.http-api-cors-preflight-origin-allowlist-and-delete, apigatewayv2.http-api-named-stage-rawpath-includes-stage-prefix, apigatewayv2.http-api-v2-binary-body-base64-encoded-both-ways, apigatewayv2.parameter-mapping-rewrites-a-real-request-and-response, apigatewayv2.quick-create-lambda-target-invoked-directly-delivers-v2-event, apigatewayv2.lambda-authorizer-array-context-value-passes-through-unchanged, apigatewayv2.lambda-authorizer-payload-format-1-0-iam-policy-response, apigatewayv2.lambda-authorizer-simple-response-explicit-deny-forbidden, apigatewayv2.lambda-payload-format-2-0-response-inference-and-cookies, apigatewayv2.simple-response-authorizer-under-payload-format-1-0-fails-500, apigatewayv2.request-authorizer-cache-is-keyed-per-identity-source-value, apigatewayv2.request-authorizer-missing-one-of-two-identity-sources-unauthorized, apigatewayv2.reset-authorizers-cache-affects-invoke, apigatewayv2.reset-authorizers-cache-server-side, apigatewayv2.route-throttling-returns-429-while-twin-route-serves, apigatewayv2.deep-apigatewayv2-2-enforce-aws-iam-route-unsigned-forbidden-signed-served, apigatewayv2.deep-apigatewayv2-2-enforce-integration-timeout-503-while-twin-route-serves, apigatewayv2.deep-apigatewayv2-2-enforce-payload-10mib-limit-413-not-delivered, apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot, apigatewayv2.deep-apigatewayv2-3-stagesem-request-authorizer-cache-expires-after-ttl, apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-selects-lambda-backend, apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected.

=== Cause 2 of 8: a deployed WEBSOCKET API refuses the RFC 6455 upgrade with HTTP 404 ===
AWS: recorded behavior only (data plane). A WEBSOCKET API with $connect/$disconnect/$default routes and a deployed stage completes the WebSocket handshake with HTTP 101 Switching Protocols on the stage's wss:// URL, then dispatches frames by RouteSelectionExpression and invokes $disconnect on close.
Emulator: the upgrade request is answered `HTTP/1.1 404 Not Found` with `content-type: application/x-amz-json-1.1` and `x-amzn-errortype: UnknownService` — i.e. the front door does not recognise the execute-api WebSocket host/path as any service at all, so no handshake, no route dispatch and no $disconnect ever happen. The checks that poll the upgrade instead of asserting the first answer run to the 60 s cap.
Why: the whole WebSocket feature set (connect authorization, action-based route selection, frame size limits, binary-frame rejection, server-initiated and client-initiated close, broadcast via @connections) is unreachable.
Fix: the execute-api WebSocket listener and the router entry that maps a WEBSOCKET stage's announced host/path to it. Related open issue 20261023T124140Z-apigatewayv2-websocket-duplex-data-plane-is-not-served.md describes the same surface but does not carry these checks' replay keys.
Traces: apigatewayv2.client-initiated-close-invokes-disconnect-route, apigatewayv2.websocket-inbound-binary-frame-rejected, apigatewayv2.websocket-inbound-message-32kb-boundary, apigatewayv2.websocket-broadcast-to-multiple-connections, apigatewayv2.websocket-connect-request-authorizer-token-gate, apigatewayv2.websocket-route-selection-by-action.

=== Cause 3 of 8: CreateVpcLink accepts subnets that do not exist and subnets from different VPCs ===
AWS: Smithy: CreateVpcLink input SubnetIds (SubnetIdList) required, errors BadRequestException (httpError 400) and TooManyRequestsException; success is HTTP 201. Recorded behavior: a VPC link whose SubnetIds name a nonexistent subnet, or name subnets belonging to two different VPCs, is refused with BadRequestException 400 and no VpcLink is created.
Emulator: both requests succeed with HTTP 201 and a VpcLink in VpcLinkStatus PENDING (observed `{'HTTPStatusCode': 201, ..., 'SubnetIds': ['subnet-0000000000bad0bad'], 'VpcLinkStatus': 'PENDING'}`). No cross-reference against EC2 subnets or their VPCs is performed.
Why: a client that relies on CreateVpcLink to validate its network wiring gets a link that will never provision, with no error at creation time; negative-path IaC tests pass against the emulator and fail on AWS.
Fix: CreateVpcLink's request validation in its handler — resolve each SubnetId in the account's EC2 state and require a single common VPC before admitting the link.
Traces: apigatewayv2.create-vpc-link-invalid-subnet-bad-request, apigatewayv2.vpc-link-mismatched-subnet-vpcs-rejected.

=== Cause 4 of 8: CreateDomainName accepts a request with no DomainNameConfigurations ===
AWS: Smithy: CreateDomainName errors include BadRequestException (httpError 400); DomainNameConfigurations is an optional list on the model but a REGIONAL domain cannot be created without an endpoint configuration. Recorded behavior: the call is refused with BadRequestException 400.
Emulator: the call succeeds with HTTP 201 and returns a domain name with `'DomainNameConfigurations': []` (observed alongside RoutingMode API_MAPPING_ONLY and a synthesised DomainNameArn).
Why: a client gets a custom domain object that can never present a certificate or serve TLS, and the misconfiguration is only discovered later (or never), instead of at the create call.
Fix: CreateDomainName's request validation in its handler — reject an empty/absent DomainNameConfigurations with BadRequestException 400.
Traces: apigatewayv2.domain-name-empty-configuration-rejected.

=== Cause 5 of 8: GetRoutes accepts a NextToken it never issued ===
AWS: Smithy: GetRoutes errors BadRequestException (httpError 400), NotFoundException (404), TooManyRequestsException (429); success code 200. Recorded behavior: GetRoutes with a NextToken that was never issued is refused with BadRequestException 400 (recorded message, as an example: 'Invalid position parameter').
Emulator: the call returns HTTP 200 with `{'Items': []}`. The paging legs of the same check pass (MaxResults is honoured and the chain visits every route once), so only the invalid-token rejection is missing.
Why: a paging client that corrupts or carries over a token silently sees an empty page instead of an error, and treats "no more routes" as truth — a drift detector then reports every route as deleted.
Fix: the pagination-token decoder shared by the apigatewayv2 Get* list handlers — an undecodable/unissued token must raise BadRequestException 400 rather than yielding an empty page.
Traces: apigatewayv2.get-routes-pagination-chains-every-route-once.

=== Cause 6 of 8: CreateStage rejects a StageVariables value that is inside the allowed character set ===
AWS: Smithy: CreateStage input StageVariables (StageVariablesMap), errors BadRequestException 400; success 201. Recorded behavior: a stage variable whose value is the host-and-path portion of an API Gateway invoke URL is accepted (that is the documented HTTP-proxy use case — a stage variable interpolated into an HTTP_PROXY IntegrationUri host).
Emulator: CreateStage answers BadRequestException with `Invalid value for StageVariables: the value of 'backendUrl' must match [A-Za-z0-9-._~:/?#&=,]+.` for a value that is exactly such a host+path, so the check aborts before it can exercise stage-variable substitution at all. Note the value handed to CreateStage is derived from the invoke URL the EMULATOR itself announced for the backend stage. Hypothesis: that announced URL contains a character outside AWS's stage-variable charset (e.g. a literal stage name such as `$default` in the path), in which case the defect is in the announced invoke-URL shape rather than in the validator; otherwise the validator is stricter than the pattern its own message quotes.
Why: HTTP_PROXY backends selected per stage cannot be built at all — the client is refused at CreateStage for a value AWS accepts.
Fix: CreateStage's StageVariables validation and the invoke-URL builder the control plane announces for a stage (one of the two is wrong; the error message quotes the correct AWS charset).
Traces: apigatewayv2.http-proxy-stage-variable-selects-backend-host.

=== Cause 7 of 8: ResetAuthorizersCache is a NotImplemented stub ===
AWS: Smithy: ResetAuthorizersCache, DELETE /v2/apis/{ApiId}/stages/{StageName}/cache/authorizers, success code 204, modeled errors NotFoundException (httpError 404) and TooManyRequestsException (429). For a stage name that does not exist the call is refused with NotFoundException 404.
Emulator: the call answers error code `NotImplemented` ('operation ... is scaffolded but not implemented in this emulator'), which is not a modeled error of this operation, so neither the 204 success nor the 404 negative can be observed. The suite did not mark this check as covered by a scaffold-stub issue.
Why: a client that flushes the authorizer cache after rotating an authorizer gets an unmodeled error its SDK cannot classify, and cannot distinguish a bad stage name from an unsupported call.
Fix: the ResetAuthorizersCache handler — implement the 204 success and the NotFoundException 404 for an unknown ApiId/StageName.
Traces: apigatewayv2.reset-authorizers-cache-invalid-stage-rejected.

=== Cause 8 of 8: CloudFormation stack Outputs for a CDK-deployed API Gateway v2 stack carry no OutputValue ===
AWS: recorded behavior only (the failing call is CloudFormation DescribeStacks, reached from these apigatewayv2 checks). Each entry of a stack's Outputs list carries both OutputKey and OutputValue once the stack reaches CREATE_COMPLETE.
Emulator: the Outputs entries have an OutputKey but no OutputValue, so the checks raise `KeyError: 'OutputValue'` while building the key->value map, and the deployed API's endpoint is never obtainable. Both affected checks deploy an asset-free aws-cdk-lib HttpApi template (LegacyStackSynthesizer, no bootstrap) through plain CloudFormation.
Why: no client can read a deployed stack's outputs — the endpoint URL, function name and api id a CDK app publishes are all unreachable, so nothing downstream of a deploy can run.
Fix: the CloudFormation DescribeStacks response serializer (the stack Outputs member) and whatever resolves an output's Value at stack-complete time. Related open issue 20261023T125319Z-apigatewayv2-cloudformation-deployed-api-gateway-v2-stacks-are-unusable.md covers Ref of AWS::ApiGatewayV2::Api and IAM role-name length, not a missing OutputValue, and does not carry these checks' replay keys.
Traces: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-cors-lambda-default-route-serves-then-destroys, apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected.

## Evidence
Cause 1 (expected vs actual): expected HTTP 200 from the stage's own invoke URL within the check's 90 s poll; actual — never any expected status, check killed at the suite's 60 s cap (`Error: Check timed out after 60.0s`) on 29 checks that all carry the DATAPLANE_UNIMPLEMENTED marker.
Cause 2: expected HTTP 101 Switching Protocols on the WEBSOCKET stage URL; actual `ConnectionError: websocket handshake failed: b'HTTP/1.1 404 Not Found'` with `x-amzn-errortype: UnknownService`.
Cause 3: expected BadRequestException 400; actual HTTP 201 — `{'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'SubnetIds': ['subnet-0000000000bad0bad'], 'VpcLinkId': ..., 'VpcLinkStatus': 'PENDING'}` (nonexistent subnet) and HTTP 201 with `'SubnetIds': ['subnet-00000000000000024', 'subnet-0000000000000002a']` (two VPCs).
Cause 4: expected BadRequestException 400; actual HTTP 201 — `{'HTTPStatusCode': 201, ..., 'DomainNameConfigurations': [], 'RoutingMode': 'API_MAPPING_ONLY'}`.
Cause 5: expected BadRequestException 400 (recorded message example 'Invalid position parameter') for `get_routes(ApiId=..., MaxResults='2', NextToken='sim2real-not-a-token')`; actual HTTP 200 `{'Items': []}`.
Cause 6: expected HTTP 201 from CreateStage; actual BadRequestException `Invalid value for StageVariables: the value of 'backendUrl' must match [A-Za-z0-9-._~:/?#&=,]+.`
Cause 7: expected NotFoundException (404) for an unknown stage; actual error code `NotImplemented`.
Cause 8: expected every DescribeStacks Outputs entry to carry OutputValue; actual `KeyError: 'OutputValue'`.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an SDK/CLI at it.
2. Cause 1: CreateApi(ProtocolType=HTTP) -> CreateIntegration(HTTP_PROXY or AWS_PROXY) -> CreateRoute -> CreateStage(StageName='$default', AutoDeploy=true), then GET the invoke URL the emulator announced for that api id and stage; it never answers 200.
3. Cause 2: CreateApi(ProtocolType=WEBSOCKET) with $connect/$disconnect/$default routes, CreateDeployment, CreateStage, then open a WebSocket upgrade to the stage URL; it answers 404 with x-amzn-errortype: UnknownService.
4. Cause 3: CreateVpcLink(SubnetIds=['subnet-0000000000bad0bad']) and CreateVpcLink with two subnets from different VPCs — both return 201.
5. Cause 4: CreateDomainName(DomainName=...) with no DomainNameConfigurations — returns 201.
6. Cause 5: GetRoutes(ApiId=<existing>, MaxResults='2', NextToken='not-a-token') — returns 200 with an empty Items list.
7. Cause 6: CreateStage(..., StageVariables={'backendUrl': '<host>/<path> of an announced invoke URL'}) — returns BadRequestException.
8. Cause 7: ResetAuthorizersCache(ApiId=<existing>, StageName='does-not-exist') — returns NotImplemented instead of NotFoundException.
9. Cause 8: deploy any CloudFormation template with Outputs through CreateStack, wait for CREATE_COMPLETE, then DescribeStacks and read Stacks[0].Outputs[*].OutputValue — the member is absent.
10. Replay the check ids in trace_ids to confirm each cause.
