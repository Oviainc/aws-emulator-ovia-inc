Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__dynamodb.appsync-dynamodb-unit-resolver-round-trips-item
Issue-Key: appsync__dynamodb.appsync-dynamodb-unit-resolver-round-trips-item|run#6|PublishedUrlRejected|graphql_uri, route_headers = ctx.endpoint_route(uris["GRAPHQL"])
Issue-Commit: c9a86881ef96f093104e2802f866de6afbf03a24

# AppSync GraphQL data plane unreachable: the published uris.GRAPHQL names an AWS host the emulator does not serve, so no DynamoDB unit-resolver round trip can run

> _Note: this issue was identified against the code base at commit `c9a86881ef96f093104e2802f866de6afbf03a24`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Cause 1 of 1: the GraphQL endpoint the emulator publishes for a GraphQL API is an AWS hostname, not an address the emulator answers on, so the whole GraphQL data plane (here: a DynamoDB unit resolver round trip) is unreachable for a client that dials the endpoint the API gave it.
AWS: GraphqlApi$uris (Smithy doc: "The URIs.") and GraphqlApi$dns publish the host the API's GraphQL endpoint is actually served at — on AWS https://<host-label>.appsync-api.<region>.amazonaws.com/graphql. For an emulator the requirement is that the published address is one it is reached on; this service already does that for an Event API, where state::event_api_dns publishes the HTTP/realtime sockets the API is served on.
Emulator: CreateGraphqlApi publishes https://<label>.appsync-api.<region>.amazonaws.com/graphql, whose host is none of the emulator's own hosts. A client that POSTs its GraphQL query there is refused before any request reaches the emulator: "its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener".
Why: every data-plane behavior of a GraphQL API — queries, mutations, resolver execution against DynamoDB, api-key and SigV4 authorization — is unobservable and unusable, because the only address the API advertises is a public AWS hostname nothing on the host resolves to the emulator.
Fix: the GraphQL API endpoint publication (state::api_dns in services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/state.rs, lines ~2989-3000, called from ops/create_graphql_api.rs line 264) must publish the host:port the emulator serves that API's GraphQL endpoint on, as event_api_dns already does for Event APIs. Same root cause as cause 3 of the issue filed for service 'appsync' (appsync.op-contract-create-graphql-api) and cause 2 of the appsync__cloudwatch issue; repeated here so this check carries a replay key under its own service heading.
Traces: appsync__dynamodb.appsync-dynamodb-unit-resolver-round-trips-item.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (bootstrap_signal false — the emulator serves).

[mismatch] appsync__dynamodb.appsync-dynamodb-unit-resolver-round-trips-item:
  simparity.published.PublishedUrlRejected: endpoint_url was given https://<label>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

(The host label, region and port are per-run values; the requirement is only that the published host is one the emulator answers on.)

## Steps to Reproduce
1. Clone the repository and boot the emulator built from it.
2. Replay simparity-behavioral-aws-v1; appsync__dynamodb.appsync-dynamodb-unit-resolver-round-trips-item fails before any GraphQL request is sent.
3. By hand: CreateGraphqlApi(authenticationType=API_KEY), CreateApiKey, StartSchemaCreation with an SDL carrying getPost/putPost, create a DynamoDB data source and a unit resolver, then read graphqlApi.uris.GRAPHQL and POST {"query": "..."} with x-api-key to that URI.
4. The published URI is https://<label>.appsync-api.<region>.amazonaws.com/graphql, which the emulator does not serve; it must publish the emulator's own host:port for that API's GraphQL endpoint.
