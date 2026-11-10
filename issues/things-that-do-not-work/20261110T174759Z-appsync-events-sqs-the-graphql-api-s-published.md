Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__events__sqs.eventbridge-datasource-batch-entries-route-independently-in-order appsync__events__sqs.eventbridge-datasource-non-object-detail-rejects-whole-batch appsync__events__sqs.eventbridge-datasource-role-scoped-to-other-bus-denied appsync__events__sqs.eventbridge-datasource-role-without-put-events-denied appsync__events__sqs.eventbridge-datasource-update-repoints-mutations-to-new-bus appsync__events__sqs.xsvc-appsync-1-eventbridge-datasource-mutation-routes-to-matching-rule-only
Issue-Key: appsync__events__sqs.eventbridge-datasource-batch-entries-route-independently-in-order|_graphql#27|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__events__sqs.eventbridge-datasource-non-object-detail-rejects-whole-batch|_graphql#23|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__events__sqs.eventbridge-datasource-role-scoped-to-other-bus-denied|_graphql#22|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__events__sqs.eventbridge-datasource-role-without-put-events-denied|_graphql#22|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__events__sqs.eventbridge-datasource-update-repoints-mutations-to-new-bus|_graphql#24|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__events__sqs.xsvc-appsync-1-eventbridge-datasource-mutation-routes-to-matching-rule-only|_graphql#24|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Commit: ef9cd1f472238b59a7ccf5edc608de1fa0253c97

# appsync__events__sqs: the GraphQL API's published uris.GRAPHQL is an appsync-api.<region>.amazonaws.com host the emulator does not serve, so no EventBridge-datasource mutation can be driven (6 checks)

> _Note: this issue was identified against the code base at commit `ef9cd1f472238b59a7ccf5edc608de1fa0253c97`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
One net-new root cause (1 of 1) behind all six appsync__events__sqs checks.

Cause 1 of 1: CreateGraphqlApi/GetGraphqlApi publish a GRAPHQL endpoint on a real-AWS hostname instead of on an address the emulator serves, so every check that must POST a GraphQL mutation at an EventBridge data source is refused before a single request is made. Nothing about the AppSync-to-EventBridge path (batch entry routing, non-object detail rejection, role-scoped denials, data-source repointing, rule matching) can be exercised while the endpoint is unaddressable.

AWS: Smithy (service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice): CreateGraphqlApi/GetGraphqlApi return GraphqlApi, whose member uris ("The URIs.") maps endpoint kind -> URI, with the GRAPHQL entry being the address a client POSTs GraphQL documents to. On AWS that host answers the API's GraphQL data plane, and a mutation wired to an EventBridge data source performs PutEvents on the configured bus (eventBridgeConfig.eventBusArn), returning failedCount/eventIds.

Emulator: the published value is "https://<api-id>.appsync-api.<region>.amazonaws.com/graphql" — a host the emulator does not own and is not reached on. Replay refuses it with simparity.published.PublishedUrlRejected: "endpoint_url was given https://<api-id>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". No mutation is ever sent, so no event reaches EventBridge or the subscriber queue.

Why: the only documented discovery path for an AppSync API sends every client off the emulator. A customer that reads uris.GRAPHQL (as the console, amplify and every SDK sample do) cannot reach the API at all, and the whole AppSync data plane — including its EventBridge data sources — is unusable even though the control plane answers.

Fix: the uris construction in the appsync crate's CreateGraphqlApi/GetGraphqlApi/UpdateGraphqlApi handlers (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice), which must publish the emulator's own host and port for the API's GraphQL listener, together with the data-plane listener that serves that address.

Related but not covering: open issues 20261110T055930Z-appsync-graphql-api-endpoints-publish-aws-hostnames-the.md, 20261110T153046Z-appsync-logs-appsync-the-graphql-api-s-published-uris-endpoint.md and 20261109T150433Z-appsync-dynamodb-appsync-graphql-data-plane-unreachable-the-published-uris.md report the same published-host defect for other service groupings; none carries the replay keys of the six appsync__events__sqs checks listed here.

Traces: the six check ids in trace_ids.

## Evidence
Expected: GetGraphqlApi/CreateGraphqlApi publish a uris.GRAPHQL the client can POST to; a mutation backed by an AMAZON_EVENTBRIDGE data source returns {"failedCount":0,"eventIds":[...]} and the event envelope lands on the rule's SQS target.
Actual: uris.GRAPHQL = "https://<api-id>.appsync-api.<region>.amazonaws.com/graphql"; replay raises simparity.published.PublishedUrlRejected at context.py:602 — "its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>". All six checks fail at that point with no mutation issued (the api-id, region and port differ per run; the defect is that the host is not the emulator's).

## Steps to Reproduce
1. CreateGraphqlApi (any auth type), CreateApiKey, StartSchemaCreation with a schema whose mutation is backed by an AMAZON_EVENTBRIDGE data source (CreateDataSource with eventBridgeConfig.eventBusArn), CreateResolver for that mutation.
2. Read uris['GRAPHQL'] from GetGraphqlApi and POST the mutation document to it.
3. Observe the host is <api-id>.appsync-api.<region>.amazonaws.com, which the emulator does not serve, so the request never reaches the emulator.
4. Replay the six appsync__events__sqs checks in trace_ids.
