Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__logs.appsync-field-log-level-all-delivers-tracing-for-the-response-request-id
Issue-Key: appsync__logs.appsync-field-log-level-all-delivers-tracing-for-the-response-request-id|_post_json#10|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# appsync: the GraphQL API's published uris endpoint is a real-AWS appsync-api.<region>.amazonaws.com host the emulator does not serve (1 cause)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Cause 1 of 1: the endpoint AppSync publishes for a GraphQL API names an AWS production host instead of the address the emulator is reached on.
AWS: GraphqlApi.uris is 'The URIs' of the API (Smithy appsync GraphqlApi member uris: MapOfStringToString; see also member dns), and the GRAPHQL entry is the address a client posts queries to — in a recorded run the client reaches the API on exactly the host it was handed.
Emulator: it publishes 'https://<api-id>.appsync-api.<region>.amazonaws.com/graphql', a host it does not serve, so the harness refuses the value outright: 'simparity.published.PublishedUrlRejected: endpoint_url was given https://<api-id>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<emulator host>:<port>/<resource path>'. The host, not the port or id, is the divergence.
Why: a client that honours the published uris/GRAPHQL value sends its queries to real AWS (or nowhere); no GraphQL request can be issued, so field-level log levels, tracing and the x-amzn-RequestId correlation this check grades can never be observed.
Fix: the appsync GraphqlApi uris/dns serializer (and the state that mints the API's endpoint) must publish the emulator's own host and port for the API's resource path.
Traces: appsync__logs.appsync-field-log-level-all-delivers-tracing-for-the-response-request-id.

## Evidence
Expected: uris['GRAPHQL'] is an address on one of the emulator's own hosts, which the client then reaches. Actual: 'endpoint_url was given https://olq3ff6sshodyo7xlwjxkalb5e.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts' (raised in context.py _published_target). The api id, region and local port in that line are per-run values; the real-AWS host suffix is the defect.

## Steps to Reproduce
Replay appsync__logs.appsync-field-log-level-all-delivers-tracing-for-the-response-request-id: CreateGraphqlApi with logConfig fieldLogLevel ALL, read uris['GRAPHQL'] from CreateGraphqlApi/GetGraphqlApi and POST a query to it.
