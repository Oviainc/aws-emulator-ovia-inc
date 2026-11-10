Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__dynamodb.appsync-batch-get-item-resolver-preserves-arity-and-appends-per-key-errors appsync__dynamodb.appsync-concurrent-conditional-mutations-single-winner appsync__dynamodb.appsync-iam-signed-mutation-reaches-realtime-subscriber-and-query-reads-it-back appsync__dynamodb.appsync-unsigned-request-to-iam-auth-api-rejected
Issue-Key: appsync__dynamodb.appsync-batch-get-item-resolver-preserves-arity-and-appends-per-key-errors|_post_json#18|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__dynamodb.appsync-concurrent-conditional-mutations-single-winner|_post_json#17|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__dynamodb.appsync-iam-signed-mutation-reaches-realtime-subscriber-and-query-reads-it-back|_connect_realtime#4|PublishedUrlRejected|url, route_headers = ctx.endpoint_route(f"{realtime_url}?{query}", websocket=True)
Issue-Key: appsync__dynamodb.appsync-unsigned-request-to-iam-auth-api-rejected|_post_raw#16|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# appsync__dynamodb: CreateGraphqlApi publishes a real appsync-api.<region>.amazonaws.com GRAPHQL URI, so the emulator's own GraphQL data plane is unaddressable

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Cause 1 of 1: the GraphqlApi `uris` map (and the endpoint the checks derive from it) names an AWS-owned host instead of the address the emulator is reached on.

AWS: CreateGraphqlApi / GetGraphqlApi return `graphqlApi.uris`, a map whose `GRAPHQL` (and `REALTIME`) entry is the endpoint a client POSTs GraphQL documents to; on AWS that endpoint is the service's own reachable host. Smithy: `GraphqlApi.uris` is a `MapOfStringToString` documented only as "The URIs." — the model constrains neither host nor scheme, so the requirement is recorded/structural rather than model-stated: the published URI must be an address of the server that published it. For an emulator the only such addresses are its own listener host:port (here `http(s)://127.0.0.1:<port>/<resource path>`, or `http(s)://<own host>:<port>/<path>` on a per-resource listener).

Emulator: publishes `https://<apiId>.appsync-api.us-west-2.amazonaws.com/graphql` — a host it does not serve and cannot be reached on. The harness refuses the value with `simparity.published.PublishedUrlRejected: endpoint_url was given https://<apiId>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts`. The apiId in the host differs per run and is not part of the requirement; the defect is the DOMAIN, which is constant.

Why: every client of the emulator's AppSync is dead on arrival. A caller that does the normal thing — create an API, read `uris.GRAPHQL`, POST a query or mutation there — is sent to the public internet (or to a name that does not resolve), so no resolver ever runs: unit resolvers, BatchGetItem arity/per-key errors, IAM-signed and unsigned request handling, and realtime subscriptions are all unobservable through the documented path.

Fix: the GraphqlApi URI projection in the appsync service crate — the `uris` map (GRAPHQL and REALTIME) and any `dns` entries must be rendered from the emulator's own listener host and port, the same way other services' published endpoints are, instead of from an `appsync-api.<region>.amazonaws.com` template. Note: open issue 20261109T150433Z-appsync-dynamodb-appsync-graphql-data-plane-unreachable-the-published-uris.md describes the same published-URI defect and carries the replay key for appsync-dynamodb-unit-resolver-round-trips-item only; the four checks listed here were not covered by it and are filed with their own replay keys.

Traces: the four `appsync__dynamodb` checks listed in trace_ids, each of which fails at the moment it tries to address the published endpoint.

## Evidence
Expected: `uris.GRAPHQL` (and REALTIME) on an emulator host — e.g. http(s)://127.0.0.1:<emulator port>/<resource path>, reachable from the client that created the API.
Actual (appsync__dynamodb.appsync-batch-get-item-resolver-preserves-arity-and-appends-per-key-errors):
  File "context.py", line 588, in endpoint_route -> _published_target
  simparity.published.PublishedUrlRejected: endpoint_url was given https://bbfo55r6bnoszc6cm56x5zvpij.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:34095/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
The same PublishedUrlRejected, with a different per-run apiId and port, is the sole failure of appsync-concurrent-conditional-mutations-single-winner, appsync-iam-signed-mutation-reaches-realtime-subscriber-and-query-reads-it-back and appsync-unsigned-request-to-iam-auth-api-rejected.

## Steps to Reproduce
1. CreateGraphqlApi (any authenticationType, e.g. API_KEY or AWS_IAM).
2. Read `graphqlApi.uris["GRAPHQL"]` from the response (or from GetGraphqlApi).
3. Observe the host is `<apiId>.appsync-api.<region>.amazonaws.com`, not the emulator's own host:port, and that POSTing a GraphQL document to it does not reach the emulator.
Replay the four checks in trace_ids to confirm.
