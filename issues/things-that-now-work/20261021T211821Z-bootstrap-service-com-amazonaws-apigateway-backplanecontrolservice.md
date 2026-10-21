Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the API Gateway (apigateway-backplanecontrolservice) emulator

_Filed: 2026-10-21T21:18:21Z_
_Operation: service-com-amazonaws-apigateway-backplanecontrolservice | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 124 operations in this crate is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state (e.g. `src/ops/create_rest_api.rs:235` returns `Scaffolded::not_implemented("CreateRestApi")`). Implement a FIRST, SMALL, stateful slice: the REST API resource lifecycle, wire protocol `restJson1` (SDK id `API Gateway`, signing name `apigateway`).

## Happy path to implement (in order)
1. **CreateRestApi** — `POST /restapis` -> 201. Input: `name` (required), `description`, `version`, `binaryMediaTypes`, `apiKeySource`, `endpointConfiguration`, `policy`, `tags`, `disableExecuteApiEndpoint` (default false). Must mint a fresh 10-char lowercase alphanumeric `id`, set `createdDate` to now, AND create the implicit root resource (`path` = "/", no `parentId`, no `pathPart`) whose id is returned as `rootResourceId`. Echoing input without persisting it is a failure.
2. **CreateResource** — `POST /restapis/{restApiId}/resources/{parentId}` -> 201. Input: `restApiId`, `parentId`, `pathPart` (all required). Output: `id`, `parentId`, `pathPart`, `path`, `resourceMethods`. `path` must be COMPUTED by concatenating the parent's stored path with `/{pathPart}` (so a child of root named `pets` has path `/pets`). Unknown `restApiId` or `parentId` -> `NotFoundException` (404); duplicate `pathPart` under the same parent -> `ConflictException` (409).
3. **PutMethod** — `PUT /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}` -> 201. Input: `authorizationType` (required), `apiKeyRequired` (default false), `authorizerId`, `operationName`, `requestParameters`, `requestModels`, `requestValidatorId`, `authorizationScopes`. Must store the method on the resource keyed by `httpMethod` so a later GetResource/GetResources surfaces it in `resourceMethods`. Re-putting an existing method -> `ConflictException` (409); unknown resource -> `NotFoundException`.
4. **GetResources** — `GET /restapis/{restApiId}/resources` -> 200, `@paginated(inputToken=position, outputToken=position, items=items)`. Must return the root resource plus every created resource for that API, each with its stored `resourceMethods`, honouring `limit` and returning/accepting `position` as the page cursor (omit `position` on the last page).
5. **GetRestApi** — `GET /restapis/{restApiId}` -> 200. Must return exactly the persisted record (same `id`, `name`, `description`, `createdDate`, `rootResourceId`, `tags`, `disableExecuteApiEndpoint`) — not a synthesised body. Unknown id -> `NotFoundException` (404).
6. **DeleteRestApi** — `DELETE /restapis/{restApiId}` -> 202, empty body. Must remove the API and all of its resources/methods; a subsequent GetRestApi must yield `NotFoundException` (404), and GetRestApis must no longer list it. Deleting an unknown id -> 404.

Supporting read for the same slice: **GetRestApis** (`GET /restapis` -> 200, paginated on `position`/`limit`) must list the APIs that were actually created, in creation order.

Every one of these must compute its response from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoed input, no random ids that are never stored. Errors must use the modelled shapes (`NotFoundException` 404, `ConflictException` 409, `BadRequestException` 400) rather than 501.

## State Model
In `ComAmazonawsApigatewayBackplanecontrolserviceState`:
- `rest_apis: BTreeMap<String /*restApiId*/, RestApi>` where `RestApi { id, name, description, version, created_date, binary_media_types: Vec<String>, minimum_compression_size: Option<i32>, api_key_source: Option<ApiKeySourceType>, endpoint_configuration: Option<EndpointConfiguration>, policy: Option<String>, tags: BTreeMap<String,String>, disable_execute_api_endpoint: bool, root_resource_id: String, resources: BTreeMap<String /*resourceId*/, ResourceEntry> }`
- `ResourceEntry { id, parent_id: Option<String>, path_part: Option<String>, path: String /*materialised full path*/, methods: BTreeMap<String /*HTTP verb, uppercase*/, MethodEntry> }`
- `MethodEntry { http_method, authorization_type, authorizer_id: Option<String>, api_key_required: bool, operation_name: Option<String>, request_parameters: BTreeMap<String,bool>, request_models: BTreeMap<String,String>, request_validator_id: Option<String>, authorization_scopes: Vec<String>, method_responses: BTreeMap<String, MethodResponseEntry>, integration: Option<IntegrationEntry> }` (the last two left empty by this slice, populated later by PutMethodResponse/PutIntegration)
- `id_seed` / counter used to mint the 10-character api and resource ids deterministically.

All other operations (deployments, stages, authorizers, api keys, usage plans, domain names, documentation, VPC links, export/SDK generation, TestInvoke*) stay stubbed at 501 and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-apigateway-backplanecontrolservice/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json`) lists all 124 operations with routes and members: CreateRestApi `POST /restapis -> 201` with output `id`/`rootResourceId`/`createdDate` (line 103-108), CreateResource `POST /restapis/{restApiId}/resources/{parentId} -> 201` with output `path` (line 96-101), PutMethod `PUT /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod} -> 201` (line 696-701), GetResources `GET /restapis/{restApiId}/resources -> 200` with `@paginated(inputToken=position, outputToken=position, items=items)` (line 536-542), GetRestApi `GET /restapis/{restApiId} -> 200` (line 544-549), GetRestApis `GET /restapis -> 200` paginated (line 551-557), DeleteRestApi `DELETE /restapis/{restApiId} -> 202` with no output members (line 257-262). All of these declare `NotFoundException (HTTP 404)` / `ConflictException (HTTP 409)` / `BadRequestException (HTTP 400)`. Stub proof: `src/ops/create_rest_api.rs:4` carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 235 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateRestApi"))`; every file under `src/ops/` is the same scaffold. No tests exercise any operation (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an apigateway client at it (`aws --endpoint-url=<emulator> apigateway ...`).
2. `create-rest-api --name petstore` -> expect 201 with a non-empty `id`, a `createdDate`, and a `rootResourceId`; today returns HTTP 501 NotImplemented.
3. `get-rest-api --rest-api-id <id>` -> expect 200 echoing the same persisted id/name/createdDate/rootResourceId.
4. `create-resource --rest-api-id <id> --parent-id <rootResourceId> --path-part pets` -> expect 201 with `path` == "/pets" and `parentId` == rootResourceId.
5. `put-method --rest-api-id <id> --resource-id <petsId> --http-method GET --authorization-type NONE` -> expect 201 with `httpMethod` GET, `authorizationType` NONE, `apiKeyRequired` false.
6. `get-resources --rest-api-id <id>` -> expect 200 listing both the root resource (path "/") and /pets, with /pets carrying `resourceMethods.GET`; repeat with `--limit 1` and expect a `position` cursor that pages to the remainder.
7. `create-resource` again with the same parent and path-part `pets` -> expect 409 ConflictException; `get-resource` with a bogus resource id -> expect 404 NotFoundException.
8. `delete-rest-api --rest-api-id <id>` -> expect 202 with an empty body; then `get-rest-api --rest-api-id <id>` -> expect 404 NotFoundException and `get-rest-apis` -> expect the api absent from `items`.
