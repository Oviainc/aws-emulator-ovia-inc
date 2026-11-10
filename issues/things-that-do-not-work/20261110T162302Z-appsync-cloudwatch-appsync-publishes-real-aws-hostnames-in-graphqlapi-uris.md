Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cloudwatch.appsync-enhanced-metrics-publish-under-api-id-resolver-dimensions appsync__cloudwatch.obs-appsync-1-resolver-datasource-metrics-opt-in-matrix appsync__cloudwatch.obs-appsync-2-graphql-realtime-lifecycle-counters appsync__cloudwatch.obs-appsync-3-4xx-error-alarm-closed-loop appsync__cloudwatch.obs-appsync-3-unauthorized-burst-on-one-api-does-not-breach-sibling-api-alarm appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters
Issue-Key: appsync__cloudwatch.appsync-enhanced-metrics-publish-under-api-id-resolver-dimensions|_post_json#9|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__cloudwatch.obs-appsync-1-resolver-datasource-metrics-opt-in-matrix|_post_json#23|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__cloudwatch.obs-appsync-2-graphql-realtime-lifecycle-counters|run#26|PublishedUrlRejected|ws = _WS(*ctx.endpoint_route(_realtime_url(realtime_uri, auth), websocket=True))
Issue-Key: appsync__cloudwatch.obs-appsync-3-4xx-error-alarm-closed-loop|_post_json#25|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__cloudwatch.obs-appsync-3-unauthorized-burst-on-one-api-does-not-breach-sibling-api-alarm|_post_json#10|PublishedUrlRejected|routed_url, route_headers = ctx.endpoint_route(url)
Issue-Key: appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters|_poll_metrics#44|Timeout|time.sleep(POLL_INTERVAL_S)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# AppSync publishes real-AWS hostnames in GraphqlApi$uris (and the realtime/event endpoints), so no AppSync data-plane traffic or AWS/AppSync metric can be produced (2 causes)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
Two net-new root causes behind 6 failing behavioral checks in this service heading. A fixer must close BOTH before submitting.

Cause 1 of 5 checks (Cause 1 of 2): the published GraphQL/realtime endpoint names a host the emulator does not serve.
AWS: CreateGraphqlApi/GetGraphqlApi return GraphqlApi$uris (Smithy: com.amazonaws.appsync#GraphqlApi member 'uris', MapOfStringToString, keys GRAPHQL and REALTIME) — the address a client must call to execute queries and to open the realtime subscription socket. In an emulator that address must be an address the emulator itself answers on; announcing any other host makes the data plane unreachable.
Emulator: uris.GRAPHQL is published as https://<apiid>.appsync-api.us-west-2.amazonaws.com/graphql and uris.REALTIME as wss://<apiid>.appsync-realtime-api.us-west-2.amazonaws.com/graphql — real AWS hostnames. The harness refuses the egress with simparity.published.PublishedUrlRejected: 'endpoint_url was given https://<apiid>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener'. (The host/port of a given run is environment-specific; the requirement is only that the announced host be one the emulator serves.)
Why: no query, mutation or subscription can ever be executed against an emulated AppSync API, so the whole AWS/AppSync metric family (Requests, Latency, GraphQLError, 4XXError, 5XXError, ActiveConnections/ActiveSubscriptions, and the enhanced per-Resolver/DataSource/Operation series) is unobservable and every alarm over it is dead.
Fix: the appsync API-creation/projection path (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice) must mint uris.GRAPHQL / uris.REALTIME (and the Event API's HTTP/REALTIME endpoints) from the emulator's own listener address, and serve the GraphQL POST and the realtime WebSocket there.
Why this is not covered by an existing issue: 20261110T055930Z-appsync-graphql-api-endpoints-publish-aws-hostnames-the.md and 20261109T150433Z-appsync-cloudwatch-observability-activesubscriptions-never-returns-to-0.md describe the same family but carry different replay keys; the five checks below are unowned by any open issue.
Traces: appsync__cloudwatch.appsync-enhanced-metrics-publish-under-api-id-resolver-dimensions, appsync__cloudwatch.obs-appsync-1-resolver-datasource-metrics-opt-in-matrix, appsync__cloudwatch.obs-appsync-2-graphql-realtime-lifecycle-counters, appsync__cloudwatch.obs-appsync-3-4xx-error-alarm-closed-loop, appsync__cloudwatch.obs-appsync-3-unauthorized-burst-on-one-api-does-not-breach-sibling-api-alarm.

Cause 2 of 2: Event API namespace lifecycle counters never appear.
AWS: an Event API's channel-namespace traffic publishes the AWS/AppSync lifecycle counters (connect/publish/subscribe outcome series) dimensioned on the API id, and they become visible within about a minute of the traffic. Recorded behavior only.
Emulator: the check connects to the Event API endpoint and polls for those counters until its own 600s budget expires, with no counters ever appearing; the run reports only the expired budget.
Why: a client cannot observe Event API health at all — no lifecycle counter, so no alarm over one can fire.
Fix: the Event API data plane and its metric emission in services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice, writing into the cloudwatch metric store (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801).
Traces: appsync__cloudwatch.obs-appsync-2-event-api-namespace-lifecycle-counters.

## Evidence
Cause 1 — expected: uris.GRAPHQL/uris.REALTIME resolve to an address the emulator serves, so the GraphQL POST and the realtime WebSocket succeed; actual: simparity.published.PublishedUrlRejected: 'endpoint_url was given https://<apiid>.appsync-api.us-west-2.amazonaws.com/graphql (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener' — identically for the wss://<apiid>.appsync-realtime-api.us-west-2.amazonaws.com/graphql realtime URL.
Cause 2 — expected: AWS/AppSync Event-API lifecycle counters for the created channel namespace within ~1 minute of the traffic; actual: 'Error: Check timed out after 600.0s' with no counters observed.

## Steps to Reproduce
1. appsync CreateGraphqlApi (API_KEY auth) and CreateApiKey; read uris.GRAPHQL and uris.REALTIME from the response -> both name *.amazonaws.com hosts the emulator does not serve.
2. POST a GraphQL query to uris.GRAPHQL (or open the subscription socket at uris.REALTIME) -> the address is not the emulator's, so the request cannot reach it.
3. Poll cloudwatch GetMetricStatistics for Namespace AWS/AppSync (Requests/GraphQLError/4XXError, Dimensions=[{GraphQLAPIId}]) -> nothing is ever published.
4. For cause 2: CreateApi + CreateChannelNamespace for an Event API, drive connect/publish/subscribe against its published endpoint and poll the AWS/AppSync lifecycle counters -> none appear within 600s.
