Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the appsync (AWSDeepdishControlPlaneService) emulator

_Filed: 2026-10-21T21:19:02Z_
_Operation: service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501, holding zero state. Nothing in this service works end to end.

Implement a FIRST, SMALL, STATEFUL slice: the GraphQL API lifecycle, which is the service's primary resource. Scope is exactly these six operations, in this order:

1. **CreateGraphqlApi** — `POST /v1/apis`. Input: `name` (required, pattern `^[_A-Za-z][_0-9A-Za-z]*$`), `authenticationType` (required, one of AMAZON_COGNITO_USER_POOLS|API_KEY|AWS_IAM|AWS_LAMBDA|OPENID_CONNECT), `xrayEnabled`, `queryDepthLimit` (0..75), `resolverCountLimit` (0..10000), `visibility` (GLOBAL|PRIVATE, default GLOBAL), `apiType` (GRAPHQL|MERGED, default GRAPHQL), `tags`, `logConfig`, `userPoolConfig`, `openIDConnectConfig`, `lambdaAuthorizerConfig`, `ownerContact`. Must MINT a new `apiId`, persist the record, and return `graphqlApi` populated from what was stored: `apiId`, `name`, `arn` (`arn:aws:appsync:<region>:<account>:apis/<apiId>`), `authenticationType`, `uris` (e.g. `{"GRAPHQL": "https://<apiId>.appsync-api.<region>.amazonaws.com/graphql"}`), `tags`, `visibility`, `apiType`, `owner`, `xrayEnabled`, `queryDepthLimit`, `resolverCountLimit`.
2. **GetGraphqlApi** — `GET /v1/apis/{apiId}`. Must look the stored record up by `apiId` and return the SAME `graphqlApi` body CreateGraphqlApi returned (same apiId, name, arn, authenticationType, uris, tags). Unknown `apiId` => `NotFoundException` (HTTP 404).
3. **ListGraphqlApis** — `GET /v1/apis`. Must return `graphqlApis` = every persisted API (not a canned/empty list), honour `maxResults` (0..25) and round-trip `nextToken`, and filter by the `apiType` / `owner` query params when supplied.
4. **CreateApiKey** — `POST /v1/apis/{apiId}/apikeys`. Input: `apiId` (httpLabel), `description`, `expires` (seconds since epoch). Must mint a key `id`, store it under the parent API, and return `apiKey` {`id`, `description`, `expires` (rounded down to the hour), `deletes`}. Unknown `apiId` => `NotFoundException` 404; `expires` outside the 1-day..365-day window => `ApiKeyValidityOutOfBoundsException` (HTTP 400).
5. **ListApiKeys** — `GET /v1/apis/{apiId}/apikeys`. Must return `apiKeys` = the keys actually created for that `apiId` (the key from step 4 must appear with the same `id`/`description`/`expires`), honour `maxResults` (0..25) and `nextToken`. Unknown `apiId` => 404.
6. **DeleteGraphqlApi** — `DELETE /v1/apis/{apiId}`. Must REMOVE the record (and its API keys) from state and return an empty 200 body. A subsequent GetGraphqlApi must then return `NotFoundException` 404, and ListGraphqlApis must no longer include it. Deleting an unknown `apiId` => `NotFoundException` 404.

Every one of these must compute its response from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)`. No canned responses, no echoing the input back without storing it, no minting an id that is never persisted — a create followed by a get/list must return what was created, and a delete must be observable by a later read. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line in each of the six files as it is implemented.

## State Model

In `src/state.rs`, on `ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState`:

- `graphql_apis: BTreeMap<String /* apiId */, GraphqlApiRecord>` — insertion/creation order preserved for stable list + pagination.
- `GraphqlApiRecord { api_id: String, name: String, arn: String, authentication_type: AuthenticationType, api_type: GraphQLApiType, visibility: GraphQLApiVisibility, owner: String /* account id */, owner_contact: Option<String>, xray_enabled: bool, query_depth_limit: i32, resolver_count_limit: i32, log_config: Option<LogConfig>, user_pool_config: Option<UserPoolConfig>, open_id_connect_config: Option<OpenIDConnectConfig>, lambda_authorizer_config: Option<LambdaAuthorizerConfig>, uris: BTreeMap<String, String>, tags: BTreeMap<String, String>, created_at: SystemTime, api_keys: BTreeMap<String /* key id */, ApiKeyRecord> }`
- `ApiKeyRecord { id: String, description: Option<String>, expires: i64 /* epoch seconds, floored to the hour */, deletes: i64 }`
- `id_seed: u64` (or equivalent) so `apiId` / API-key `id` values are unique per creation and not recomputed from the input.

Pagination for ListGraphqlApis / ListApiKeys: encode `nextToken` as an opaque cursor over the map key ordering (matching the `^[\S]+$` pattern) and return it only when more items remain.

All other operations (data sources, resolvers, functions, types, schema creation/merge, channel namespaces, domain names, caches, Event APIs, tagging, Evaluate*) stay at 501 and will be filed as individual gap issues later.

## Evidence

1) `services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/OPERATIONS.md` documents the routes, inputs, outputs and errors cited above: CreateApiKey (line 61: `POST /v1/apis/{apiId}/apikeys`, input `apiId`/`description`/`expires`, output `apiKey`, errors include ApiKeyValidityOutOfBoundsException 400 and NotFoundException 404), DeleteGraphqlApi (line 168: `DELETE /v1/apis/{apiId}`, no output members, NotFoundException 404), GetGraphqlApi (line 287: `GET /v1/apis/{apiId}`, output `graphqlApi`, NotFoundException 404), ListApiKeys (line 336: @paginated inputToken=nextToken outputToken=nextToken items=apiKeys, maxResults range 0..25), ListGraphqlApis (line 384: @paginated items=graphqlApis, query params maxResults/apiType/owner).
2) `src/ops/create_graphql_api.rs` lines 4-6 carry `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 209 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateGraphqlApi"))` — HTTP 501; the same marker is present on every file in `src/ops/`, so no operation holds state.
3) Smithy model shapes: `GraphqlApi` members (apiId, name, arn, authenticationType, uris, tags, visibility, apiType, owner, xrayEnabled, queryDepthLimit range 0..75, resolverCountLimit range 0..10000) and `ApiKey` members (id, description, expires, deletes — documented as 'seconds since the epoch, rounded down to the nearest hour' for da2 keys) define the fields each response must carry.
4) No tests assert any of this behaviour; `tests/state_roundtrip.rs` only exercises the state API.

## Steps to Reproduce

1. Start the emulator and point an AppSync client at it.
2. CreateGraphqlApi with name="MyApi", authenticationType="API_KEY" — expect 200 with graphqlApi.apiId minted, name="MyApi", arn and uris populated; today: HTTP 501 NotImplemented.
3. GetGraphqlApi with the returned apiId — expect the identical graphqlApi body (same apiId, name, arn, authenticationType).
4. ListGraphqlApis — expect graphqlApis to contain exactly the created API; repeat with maxResults=1 after creating two APIs and expect a nextToken that pages to the second.
5. CreateApiKey on that apiId with description="demo" and expires = now+30 days — expect apiKey.id minted and expires floored to the hour.
6. ListApiKeys on that apiId — expect the key from the previous step with the same id/description/expires.
7. DeleteGraphqlApi on the apiId — expect 200 with an empty body.
8. GetGraphqlApi on the deleted apiId — expect NotFoundException with HTTP 404; ListGraphqlApis must no longer include it.
9. CreateApiKey / GetGraphqlApi on a never-created apiId such as "bogus123" — expect NotFoundException HTTP 404, not 501 and not 200.
