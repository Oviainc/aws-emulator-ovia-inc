Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__lambda.appsync-batch-invoke-merges-sibling-fields-into-one-invocation appsync__lambda.appsync-batch-invoke-partial-item-error-isolates-the-failing-sibling appsync__lambda.appsync-lambda-authorizer-denial-blocks-resolver-rejected appsync__lambda.appsync-lambda-authorizer-identity-validation-expression-rejects-before-invoke appsync__lambda.appsync-lambda-authorizer-result-ttl-caches-exactly-one-invocation appsync__lambda.appsync-pipeline-direct-lambda-functions-thread-prev-result appsync__lambda.deep-appsync-2-appsync-authz-denied-fields-nulls-only-the-denied-field appsync__lambda.xsvc-appsync-3-direct-lambda-resolver-error-propagates-exception-type appsync__lambda.xsvc-appsync-3-direct-lambda-resolver-receives-full-context-event appsync__lambda.xsvc-appsync-3-service-role-without-lambda-invoke-permission-denied
Issue-Key: appsync__lambda.appsync-batch-invoke-merges-sibling-fields-into-one-invocation|_post_json#21|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__lambda.appsync-batch-invoke-partial-item-error-isolates-the-failing-sibling|_post_json#22|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__lambda.appsync-lambda-authorizer-denial-blocks-resolver-rejected|_post_json#21|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__lambda.appsync-lambda-authorizer-identity-validation-expression-rejects-before-invoke|_post_json#19|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__lambda.appsync-lambda-authorizer-result-ttl-caches-exactly-one-invocation|_post_json#21|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__lambda.appsync-pipeline-direct-lambda-functions-thread-prev-result|_post_json#16|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__lambda.deep-appsync-2-appsync-authz-denied-fields-nulls-only-the-denied-field|_post_json#18|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__lambda.xsvc-appsync-3-direct-lambda-resolver-error-propagates-exception-type|_post_json#13|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__lambda.xsvc-appsync-3-direct-lambda-resolver-receives-full-context-event|_post_json#21|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__lambda.xsvc-appsync-3-service-role-without-lambda-invoke-permission-denied|_post_json#18|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Commit: 6c534d81bd78f33a29d39ee989f5affa38b22780

# appsync__lambda: the GraphQL API's published uris.GRAPHQL is a real-AWS appsync-api.<region>.amazonaws.com host the emulator does not serve, so no Lambda-resolver / Lambda-authorizer behaviour can be exercised (1 cause)

> _Note: this issue was identified against the code base at commit `6c534d81bd78f33a29d39ee989f5affa38b22780`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Cause 1 of 1: the GraphQL data-plane endpoint AppSync publishes for an API is not an address the emulator serves, so every check that POSTs a GraphQL document to the API it just created is refused before any assertion about Lambda resolvers or Lambda authorizers can run. Failing checks: appsync__lambda.appsync-batch-invoke-merges-sibling-fields-into-one-invocation, appsync__lambda.appsync-batch-invoke-partial-item-error-isolates-the-failing-sibling, appsync__lambda.appsync-lambda-authorizer-denial-blocks-resolver-rejected, appsync__lambda.appsync-lambda-authorizer-identity-validation-expression-rejects-before-invoke, appsync__lambda.appsync-lambda-authorizer-result-ttl-caches-exactly-one-invocation, appsync__lambda.appsync-pipeline-direct-lambda-functions-thread-prev-result, appsync__lambda.deep-appsync-2-appsync-authz-denied-fields-nulls-only-the-denied-field, appsync__lambda.xsvc-appsync-3-direct-lambda-resolver-error-propagates-exception-type, appsync__lambda.xsvc-appsync-3-direct-lambda-resolver-receives-full-context-event, appsync__lambda.xsvc-appsync-3-service-role-without-lambda-invoke-permission-denied.

AWS: Smithy (com.amazonaws.appsync): GraphqlApi$uris targets MapOfStringToString, documented "The URIs"; GraphqlApi also carries a dns map, "The DNS records for the API". CreateGraphqlApi/GetGraphqlApi return that map, and its GRAPHQL entry is the API's GraphQL data-plane endpoint: on real AWS a client takes that value verbatim, POSTs {"query": ...} to it with the API's auth header (x-api-key, Authorization bearer token for a Lambda authorizer, or SigV4) and gets an HTTP 200 GraphQL envelope back. Everything these checks assert — a BatchInvoke merging N sibling field resolutions into one Lambda invocation, a per-element {data, errorMessage, errorType} envelope isolating one failing sibling, a Lambda authorizer's isAuthorized=false producing 401/field-level denial, an identityValidationExpression gate refusing a non-matching token before the authorizer function runs, authorizer result-TTL caching, a PIPELINE resolver threading ctx.prev.result between Direct Lambda functions, and the Lambda:IllegalArgument errorType when the data-source service role lacks lambda:InvokeFunction — is only observable through a request to that published endpoint.

Emulator: the value published for uris.GRAPHQL is a literal real-AWS hostname, https://<apiId>.appsync-api.<region>.amazonaws.com/graphql (recorded examples include o7oxlpokxuem4apkpjszj4c4xv, g6fwrs4m3gw4rbkqa3rrcnwf6t, fhkgsyvjvbwbljacugzctx3hnz — the id is per-run, the host shape is the defect). The emulator does not serve that host, so the request is refused with simparity.published.PublishedUrlRejected: "endpoint_url was given https://<apiId>.appsync-api.<region>.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The announced address is the emulator's own output, so this is the emulator's divergence, not the runner's: it advertises an endpoint other than the one it is reachable on. All ten checks fail identically at the first data-plane request.

Why: a client that does what the AppSync docs say — create an API, read uris["GRAPHQL"], and send GraphQL to it — never reaches the emulator at all. Connections either go nowhere or, in a networked environment, leave for real AWS. No AppSync data-plane behaviour (resolvers, authorizers, batching, pipelines, field-level authorization) can be exercised against the emulator, and nothing the control plane accepted can be verified end to end.

Fix: the uris (and dns) projection shared by CreateGraphqlApi, GetGraphqlApi, UpdateGraphqlApi and ListGraphqlApis must be built from the emulator's own listener address (host:port it is reached on, or a per-API listener) instead of a synthesized appsync-api.<region>.amazonaws.com host, and the GraphQL data plane must be served at the published path. This is the same root cause already filed for other service combinations (20261110T055930Z-appsync-graphql-api-endpoints-publish-aws-hostnames-the.md, 20261110T155738Z-appsync-dynamodb-lambda-the-published-graphql-endpoint-is.md, 20261109T150433Z-appsync-dynamodb-appsync-graphql-data-plane-unreachable-the-published-uris.md); none of those issues carries these ten appsync__lambda checks, so they are filed here.

Traces: the ten failing check ids listed above, each reporting PublishedUrlRejected on the endpoint_url taken from the API's uris.GRAPHQL.

## Evidence
Expected (AWS): POST to the value of GraphqlApi$uris["GRAPHQL"] returned by CreateGraphqlApi reaches the API's GraphQL data plane and answers HTTP 200 with a GraphQL response envelope (data/errors), so the Lambda resolver and Lambda authorizer contracts are observable.

Actual (emulator): uris.GRAPHQL is published as https://<apiId>.appsync-api.<region>.amazonaws.com/graphql, a host the emulator does not serve. Every data-plane request is refused before it is sent:
  simparity.published.PublishedUrlRejected: endpoint_url was given https://o7oxlpokxuem4apkpjszj4c4xv.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:33535/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
(the api id, the 127.0.0.1 port and the region are per-run values; the fixed part is that the published host is an appsync-api.<region>.amazonaws.com name rather than the emulator's own host:port). All 10 appsync__lambda checks fail with this identical refusal.

## Steps to Reproduce
1. CreateGraphqlApi with authenticationType API_KEY (or AWS_LAMBDA with a lambdaAuthorizerConfig) and a schema exposing one field backed by a Direct Lambda data source.
2. Read uris["GRAPHQL"] from the CreateGraphqlApi (or GetGraphqlApi) response.
3. POST {"query": "{ ... }"} to that URL with the API's auth header.
4. Observe the published host is <apiId>.appsync-api.<region>.amazonaws.com — not an address the emulator serves — so the request never reaches the emulator's GraphQL data plane (PublishedUrlRejected / unreachable host).
5. Re-replay the 10 check ids in trace_ids to confirm a fix.
