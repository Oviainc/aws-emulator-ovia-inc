Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.protocol-type-illegal-attributes-rejected
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.protocol-type-illegal-attributes-rejected|_error#15|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# apigatewayv2 CreateModel accepts a Model on an HTTP API instead of refusing it with 400 BadRequestException (ProtocolType member gating missing)

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateModel

## Description
Cause 1 of 1: the apigatewayv2 control plane does not gate ProtocolType-restricted request members, so a WebSocket-only resource is created on an HTTP API.

AWS: API Gateway v2 allows Models only on WEBSOCKET APIs. Smithy (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model/com-amazonaws-apigatewayv2-apigatewayv2.json): CreateModel is POST /v2/apis/{ApiId}/models with success code 201 and models BadRequestException (httpError 400, "The request is not valid, for example, the input is incomplete or incorrect"), ConflictException (409), NotFoundException (404), TooManyRequestsException (429). The model documents the same protocol gating for the sibling members this check also exercises — CreateRouteRequest$RouteResponseSelectionExpression: "The route response selection expression for the route. Supported only for WebSocket APIs." — and IntegrationType enumerates AWS|HTTP|MOCK|HTTP_PROXY|AWS_PROXY of which an HTTP API accepts only the proxy subset. The model does not carry a per-protocol allowlist as a constraint, so for the refusal itself this is recorded behavior only: the recording shows CreateModel against an API whose ProtocolType is HTTP answered with HTTP 400 and error code BadRequestException, message (example) "Currently, Models are not permitted for APIs with a protocol type of HTTP".

Emulator: the call SUCCEEDS with HTTP 201 and mints a model — the replay records {'HTTPStatusCode': 201, 'ContentType': 'application/json', 'ModelId': '8sh3tqaaaa', 'Name': 'ProbeModel', 'Schema': '{"$schema":"http://json-schema.org/draft-04/schema#","title":"Probe","type":"object"}'}. The handler never reads the parent API's ProtocolType: it validates only that the API exists, that Name is alphanumeric, that Schema parses, name uniqueness and the per-API model cap. Because the check stops at the first rejection leg, the two other legs it asserts (CreateIntegration with IntegrationType=MOCK on an HTTP API, and CreateRoute with RouteResponseSelectionExpression on an HTTP API, both expected 400 BadRequestException) were never reached and are very likely unguarded in the same way — the same missing validation layer.

Why: a client gets a success where AWS gives it a 400, and the HTTP API ends up holding a resource that cannot exist on real AWS. Code written against the emulator will build HTTP APIs with Models (and, by the same gap, MOCK integrations and route-response selection expressions) that AWS refuses outright at deploy time, so the emulator hides a whole class of configuration errors rather than reporting them. Negative-path clients that expect to catch BadRequestException see no exception at all, and GetModels on an HTTP API returns items where AWS returns none.

Fix: the ProtocolType-conditional request validation of the apigatewayv2 control-plane operation handlers — for the observed leg, the CreateModel handler must look up the parent API's ProtocolType and refuse a non-WEBSOCKET parent with the modeled BadRequestException (400) before any state is written, leaving GetModels empty. The same gate belongs in CreateIntegration (non-proxy IntegrationType on an HTTP API) and CreateRoute (RouteResponseSelectionExpression on an HTTP API). Note this is a distinct root cause from the open issue 20261023T125319Z-apigatewayv2-api-gateway-v2-control-plane-9-fidelity-defects.md (AWS_PROXY service-subtype integrations, UpdateApi stub, off-model ValidationException), which does not cover per-protocol member gating; file/fix separately.

Traces: failing check apigatewayv2.protocol-type-illegal-attributes-rejected (mismatch), which asserts the three WebSocket-only attributes are accepted on a WEBSOCKET API and each refused on an HTTP API, and that the HTTP API's GetModels/GetIntegrations/GetRoutes are unchanged afterwards. Handler: services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_model.rs (handle(), lines 150-242 — no ProtocolType read).

## Evidence
Expected (AWS, from the recording): agw.create_model(ApiId=<http-api>, Name='ProbeModel', ContentType='application/json', Schema='{"$schema":"http://json-schema.org/draft-04/schema#","title":"Probe","type":"object"}') raises ClientError with HTTP 400 and Error.Code == 'BadRequestException' (example message: 'Currently, Models are not permitted for APIs with a protocol type of HTTP'); GetModels(ApiId=<http-api>) then returns [].

Actual (emulator): the call returns HTTP 201 with a minted ModelId. Replay output:
  AssertionError: expected the call to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'ContentType': 'application/json', 'ModelId': '8sh3tqaaaa', 'Name': 'ProbeModel', 'Schema': '{"$schema":"http://json-schema.org/draft-04/schema#","title":"Probe","type":"object"}'}
  raised at check_apigatewayv2_protocol_type_illegal_attributes_rejected.py line 94 (_error() helper, line 23).

Positive control in the same run: the identical CreateModel against a WEBSOCKET API is accepted, as AWS does — so the divergence is attributable to the missing ProtocolType gate, not to CreateModel being broken.

Code: services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_model.rs handle() validates API existence, Name alphanumeric, Schema JSON validity, name uniqueness and MAX_MODELS_PER_API, and never inspects the parent API's protocol type.

## Steps to Reproduce
1. agw = client('apigatewayv2')
2. http_id = agw.create_api(Name='probe-http', ProtocolType='HTTP')['ApiId']
3. agw.create_model(ApiId=http_id, Name='ProbeModel', ContentType='application/json', Schema='{"$schema":"http://json-schema.org/draft-04/schema#","title":"Probe","type":"object"}')
   -> emulator: HTTP 201 with a ModelId. AWS: HTTP 400 BadRequestException.
4. agw.get_models(ApiId=http_id)['Items'] -> emulator: one item. AWS: [].
5. Control (should keep working): the same CreateModel on an API created with ProtocolType='WEBSOCKET' is accepted.
6. Same gate, not reached by the replay once step 3 diverges: agw.create_integration(ApiId=http_id, IntegrationType='MOCK') and agw.create_route(ApiId=http_id, RouteKey='GET /rrse', Target='integrations/<http-proxy-integration>', RouteResponseSelectionExpression='$default') must each answer 400 BadRequestException on an HTTP API.

Replay: run_behavioral_suite and re-run check apigatewayv2.protocol-type-illegal-attributes-rejected.
