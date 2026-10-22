Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the ApiGatewayV2 emulator

_Filed: 2026-10-21T21:18:21Z_
_Operation: service-com-amazonaws-apigatewayv2-apigatewayv2 | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 103 operations in `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/` is an unmodified scaffold (`//! SCAFFOLDED, NOT IMPLEMENTED:` header, returns HTTP 501 `NotImplemented`) and the crate holds no state, so no client can do anything with this service. Implement a FIRST, SMALL, stateful slice: the HTTP-API lifecycle of the primary resource (`Api`) plus its child `Route`.

Happy path to implement, in order (restJson1 wire protocol, sigv4 signing name `apigateway`):

1. **CreateApi** — `POST /v2/apis` -> 201. Input: `Name` (required), `ProtocolType` (required, HTTP|WEBSOCKET), plus optional `Description`, `Version`, `RouteSelectionExpression`, `ApiKeySelectionExpression`, `CorsConfiguration`, `DisableExecuteApiEndpoint`, `IpAddressType`, `Tags`, `Target`, `RouteKey`. Must mint a fresh 10-char lowercase-alphanumeric `ApiId`, set `CreatedDate` to now, synthesize `ApiEndpoint` (`https://{ApiId}.execute-api.{region}.amazonaws.com`), default `RouteSelectionExpression` to `$request.method $request.path` for HTTP APIs, persist the whole record, and echo back the stored record (not the raw input).
2. **CreateRoute** — `POST /v2/apis/{ApiId}/routes` -> 201. Input: `ApiId` (label, required), `RouteKey` (required), optional `Target`, `AuthorizationType`, `AuthorizerId`, `ApiKeyRequired`, `OperationName`, `RequestParameters`, `RequestModels`. Must 404 `NotFoundException` for an unknown `ApiId`, mint a `RouteId`, store the route under its parent api, and return the stored route.
3. **GetApi** — `GET /v2/apis/{ApiId}` -> 200. Returns the persisted api record field-for-field, including `ApiId`, `Name`, `ProtocolType`, `ApiEndpoint`, `CreatedDate`, `Tags`; `NotFoundException` (404) when the id is unknown or already deleted.
4. **GetRoutes** — `GET /v2/apis/{ApiId}/routes` -> 200. Returns `Items` = the routes stored for that api (404 if the api does not exist), honouring `MaxResults`/`NextToken` by slicing the stable-ordered route list and emitting `NextToken` only when more remain.
5. **GetApis** — `GET /v2/apis` -> 200. Returns `Items` = every persisted api, honouring `MaxResults`/`NextToken` the same way. A created api MUST appear here and a deleted one MUST NOT.
6. **DeleteApi** — `DELETE /v2/apis/{ApiId}` -> 204, empty body. Removes the api and cascades to its routes; a subsequent GetApi must return 404 `NotFoundException` and GetApis must no longer list it.

Every one of these must compute its response from the persisted state — no canned bodies, no echoing the input, no minting an id that is never stored. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line from each file you implement.

## State Model

In `src/state.rs` (`ComAmazonawsApigatewayv2Apigatewayv2State`, reached via `crate::state::service_state(&*self.engine)`):

- `apis: BTreeMap<String /* ApiId */, Api>` — insertion-ordered-by-key store of apis.
- `Api { api_id: String, name: String, protocol_type: String /* HTTP|WEBSOCKET */, api_endpoint: String, description: Option<String>, version: Option<String>, route_selection_expression: Option<String>, api_key_selection_expression: Option<String>, cors_configuration: Option<Cors>, disable_execute_api_endpoint: bool, disable_schema_validation: bool, ip_address_type: Option<String>, created_date: SystemTime, tags: BTreeMap<String,String>, routes: BTreeMap<String /* RouteId */, Route>, stages: BTreeMap<String /* StageName */, Stage> }`
- `Route { route_id: String, route_key: String, target: Option<String>, authorization_type: Option<String>, authorizer_id: Option<String>, api_key_required: bool, operation_name: Option<String>, authorization_scopes: Vec<String>, request_models: BTreeMap<String,String>, request_parameters: BTreeMap<String,ParameterConstraints> }`
- `Stage { ... }` may stay empty for this slice (reserved so CreateStage/GetStage can land next without a state rewrite).
- An id counter or RNG used only to mint `ApiId`/`RouteId` values that are immediately inserted into the maps above.

All other operations (authorizers, integrations, deployments, stages, domain names, api mappings, vpc links, models, portals, routing rules, tagging, import/export) stay at 501 for now and will be filed as individual gap issues.

## Evidence

`services/service-com-amazonaws-apigatewayv2-apigatewayv2/OPERATIONS.md` documents all 103 operations and the exact wire contracts cited above: CreateApi (line 26-31: `POST /v2/apis -> 201`, required `Name`+`ProtocolType`, output `ApiId`/`ApiEndpoint`/`CreatedDate`), CreateRoute (line 110-115: `POST /v2/apis/{ApiId}/routes -> 201`, required `RouteKey`, output `RouteId`), GetApi (line 314-319: `GET /v2/apis/{ApiId} -> 200`, errors `NotFoundException (HTTP 404)`), GetApis (line 335-340: `Items` (list<Api>) + `NextToken`, inputs `MaxResults`/`NextToken`), GetRoutes (line 489-494), DeleteApi (line 152-157: `DELETE /v2/apis/{ApiId} -> 204`, no output members, `NotFoundException`). Protocol is `restJson1` per OPERATIONS.md line 10. Confirmed all-stub: `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/create_api.rs` matches at line 4, and README.md lines 36-53 state every generated stub answers HTTP 501 until the marker line is deleted. No tests exist beyond `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with service `apigatewayv2`.
2. `aws apigatewayv2 create-api --name demo-api --protocol-type HTTP` — expect 201 with a non-empty `ApiId`, an `ApiEndpoint` containing that id, and a `CreatedDate`; today it returns HTTP 501 NotImplemented.
3. `aws apigatewayv2 create-route --api-id <ApiId> --route-key 'GET /items'` — expect 201 with a fresh `RouteId` and `RouteKey` echoed from the stored route.
4. `aws apigatewayv2 get-api --api-id <ApiId>` — expect 200 with `Name=demo-api`, `ProtocolType=HTTP` and the same `ApiId`/`CreatedDate` returned by create.
5. `aws apigatewayv2 get-routes --api-id <ApiId>` — expect `Items` to contain exactly the route created above.
6. `aws apigatewayv2 get-apis` — expect `Items` to include the created api; repeat with `--max-results 1` and follow `NextToken` to page through a second created api.
7. `aws apigatewayv2 delete-api --api-id <ApiId>` — expect 204 with an empty body.
8. `aws apigatewayv2 get-api --api-id <ApiId>` again — expect HTTP 404 `NotFoundException`, and `get-apis` must no longer list it.
9. `aws apigatewayv2 get-route --api-id <ApiId> ...` / `create-route` against the deleted api — expect HTTP 404 `NotFoundException`, proving the cascade delete.
