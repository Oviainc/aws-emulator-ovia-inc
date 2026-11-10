Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cognito-idp.deep-appsync-2-appsync-authz-oidc-iat-ttl-rejects-aged-token
Issue-Key: appsync__cognito-idp.deep-appsync-2-appsync-authz-oidc-iat-ttl-rejects-aged-token|_ask#10|PublishedUrlRejected|target, route = ctx.endpoint_route(url)
Issue-Commit: 4ee504be713ad5d2fe853da15ad6096ca014344c

# appsync__cognito-idp: an OPENID_CONNECT GraphQL API publishes uris.GRAPHQL on an appsync-api.<region>.amazonaws.com host the emulator does not serve, so iatTTL token-age rejection can never be exercised (1 cause)

> _Note: this issue was identified against the code base at commit `4ee504be713ad5d2fe853da15ad6096ca014344c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
One net-new behavioral root cause, grounded in the single non-owned failing check of the simparity-behavioral-aws-v1 replay under the appsync__cognito-idp heading (0/1 passed). Related OPEN issues that do NOT carry this replay key, named for the fixer so they are not closed as covering it: 20261109T150433Z-appsync-dynamodb-appsync-graphql-data-plane-unreachable-the-published-uris.md (same gate, appsync__dynamodb checks), 20261110T055930Z-appsync-graphql-api-endpoints-publish-aws-hostnames-the.md and 20261109T150433Z-appsync-4-net-new-causes-apicache-creating-state.md (published GraphQL/realtime endpoint URIs unreachable). None of them carries this check, so this check must be re-replayed even after all three are closed — it is the only check in the suite that grades the OPENID_CONNECT iatTTL token-age gate, and it is blocked before that gate is ever reached.

Cause 1 of 1: the GraphQL API endpoint the emulator publishes in GraphqlApi.uris["GRAPHQL"] is a real-AWS hostname, not an address the emulator answers on, so no client of the emulator can send a GraphQL document to an API it just created.
AWS: Smithy com.amazonaws.appsync#CreateGraphqlApi / #GetGraphqlApi -> com.amazonaws.appsync#GraphqlApi, member `uris` (target MapOfStringToString, doc "The URIs."); the model does not constrain the host, so recorded behavior only: AWS returns uris.GRAPHQL = https://<apiId>.appsync-api.<region>.amazonaws.com/graphql and SERVES the GraphQL data plane at exactly that address — a POST of {"query": "query P { ping }"} with an Authorization header carrying an OIDC id token is answered there (200 with {"data":{"ping":"pong"}} when the token is admitted; 401 with errors == [{"errorType":"UnauthorizedException","message":"Unauthorized"}] when openIDConnectConfig.iatTTL has elapsed since the token's `iat`). The requirement is that the published URI is the address the API is reachable at; the host, api id and port are per-environment values, the invariant is that they resolve to the server that published them.
Emulator: the emulator's own CreateGraphqlApi response advertises uris.GRAPHQL = https://<apiId>.appsync-api.us-west-2.amazonaws.com/graphql while it is reached on its own loopback listener, so routing the check's GraphQL POST to the published endpoint is refused before any request leaves: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<apiId>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. Everything before that succeeded: the user pool, app client and confirmed user were created, both OPENID_CONNECT APIs reached an ACTIVE schema with a NONE-backed UNIT resolver, and GetGraphqlApi retained openIDConnectConfig.iatTTL.
Why: a client that follows the documented discovery path — create the API, read uris.GRAPHQL, POST the GraphQL document there — cannot reach the emulator at all; it is sent to real AWS (or to nothing). Every data-plane behaviour behind that URI is consequently unobservable, including the OPENID_CONNECT iatTTL contract this check grades: a fresh token must be admitted, the SAME token aged past iatTTL must be refused 401 with exactly one UnauthorizedException whose message is the fixed text "Unauthorized", a twin API configured WITHOUT iatTTL must still admit that aged token, and a newly minted token must be admitted again.
Fix: the appsync crate's endpoint publication for GraphQL APIs — the component that fills GraphqlApi.uris (and the `dns` map) on CreateGraphqlApi/GetGraphqlApi/UpdateGraphqlApi must announce the emulator's own reachable base address for the API's GraphQL (and REALTIME) endpoint, and the emulator must route that path to its GraphQL data plane, where the OPENID_CONNECT authorizer applies openIDConnectConfig.iatTTL to the token's `iat`.
Traces: appsync__cognito-idp.deep-appsync-2-appsync-authz-oidc-iat-ttl-rejects-aged-token.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout.

[mismatch] appsync__cognito-idp.deep-appsync-2-appsync-authz-oidc-iat-ttl-rejects-aged-token
  File "context.py", line 588, in endpoint_route
    return self._published_target(url, "endpoint_url"), {}
  File "context.py", line 602, in _published_target
    raise PublishedUrlRejected(f"{message}: {problem[1]}", problem[0], value_class(value))
  simparity.published.PublishedUrlRejected: endpoint_url was given https://kdaqaavu4mfjxj6fbkb5rdfuqz.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:34209/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

(The api id `kdaqaavu4mfjxj6fbkb5rdfuqz`, the region segment, the loopback port 34209 and the user/pool names are per-run values; the defect is that the published host is an *.appsync-api.<region>.amazonaws.com name rather than the emulator's own address.)

What the check had already done successfully: cognito-idp CreateUserPool + CreateUserPoolClient (ALLOW_ADMIN_USER_PASSWORD_AUTH) + AdminCreateUser + AdminSetUserPassword; appsync CreateGraphqlApi(authenticationType="OPENID_CONNECT", openIDConnectConfig={issuer, clientId, iatTTL=8000}); StartSchemaCreation -> SUCCESS; CreateDataSource(type="NONE"); CreateResolver(Query.ping, APPSYNC_JS, runtime 1.0.0); GetGraphqlApi confirmed openIDConnectConfig.iatTTL == 8000; a twin API without iatTTL was built the same way. Both APIs returned a non-empty uris.GRAPHQL — on an AWS host.

Expected (AWS), once the published endpoint is reachable:
  POST <uris.GRAPHQL>  Authorization: <id token minted seconds ago>  {"query":"query P { ping }"}
    -> 200 {"data": {"ping": "pong"}}
  POST <uris.GRAPHQL>  Authorization: <same token, now >8 s past iat, still before exp>
    -> 401, no data, errors == [{"errorType": "UnauthorizedException", "message": "Unauthorized"}]
       ("UnauthorizedException"/"Unauthorized" are fixed text; the token and api id are request/state values)
  POST <twin api uris.GRAPHQL>  Authorization: <that same aged token>   -> 200 {"data": {"ping": "pong"}}
  POST <uris.GRAPHQL>  Authorization: <newly minted token>              -> 200 {"data": {"ping": "pong"}}
Actual (emulator):
  the POST is never sent — the endpoint the emulator itself published is not an address the emulator answers on, so the request is refused by the client's own egress check and none of the four legs above is observable.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. cognito-idp: create-user-pool, create-user-pool-client with ExplicitAuthFlows ALLOW_ADMIN_USER_PASSWORD_AUTH and GenerateSecret=false, admin-create-user, admin-set-user-password --permanent. Note the issuer https://cognito-idp.<region>.amazonaws.com/<poolId> as the emulator serves it.
3. appsync create-graphql-api --authentication-type OPENID_CONNECT --open-id-connect-config '{"issuer":"<issuer>","clientId":"<clientId>","iatTTL":8000}'.
4. Read graphqlApi.uris.GRAPHQL from that response (or from get-graphql-api).
   Observed: https://<apiId>.appsync-api.<region>.amazonaws.com/graphql — a real-AWS host the emulator does not listen on; a client cannot reach the API it just created. AWS publishes the address it actually serves.
5. start-schema-creation with `schema { query: Query } type Query { ping: String }`, create-data-source --type NONE, create-resolver for Query.ping with the APPSYNC_JS runtime returning 'pong'.
6. POST {"query":"query P { ping }"} to the published uris.GRAPHQL with header Authorization: <id token from admin-initiate-auth ADMIN_USER_PASSWORD_AUTH>.
   Observed: unreachable (the published host is not the emulator's). AWS: 200 {"data":{"ping":"pong"}} for a token inside iatTTL, and 401 with errors [{"errorType":"UnauthorizedException","message":"Unauthorized"}] once the same token is older than iatTTL (8 s), while a twin API created without iatTTL still answers 200 for it.
7. Re-replay the check id in trace_ids.
