Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch.http-api-count-and-4xx-metrics-carry-the-apiid-stage-dimensions apigatewayv2__cloudwatch.http-api-missing-invoke-permission-rejected-as-5xx-metric apigatewayv2__cloudwatch.http-api-route-throttle-zero-rejected-429-counted-in-4xx apigatewayv2__cloudwatch.obs-apigatewayv2-2-http-api-detailed-metrics-opt-in-adds-method-resource-series apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-actions-disabled-notification-blocked apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns
Issue-Key: apigatewayv2__cloudwatch.http-api-count-and-4xx-metrics-carry-the-apiid-stage-dimensions|_v2_invoke_url#14|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{path.lstrip('/')}"
Issue-Key: apigatewayv2__cloudwatch.http-api-missing-invoke-permission-rejected-as-5xx-metric|_v2_invoke_url#11|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage)}/{path.lstrip('/')}"
Issue-Key: apigatewayv2__cloudwatch.http-api-route-throttle-zero-rejected-429-counted-in-4xx|_v2_invoke_url#14|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage)}/{path.lstrip('/')}"
Issue-Key: apigatewayv2__cloudwatch.obs-apigatewayv2-2-http-api-detailed-metrics-opt-in-adds-method-resource-series|_v2_invoke_url#16|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage)}/{path.lstrip('/')}"
Issue-Key: apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-actions-disabled-notification-blocked|_v2_invoke_url#23|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{path.lstrip('/')}"
Issue-Key: apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns|_v2_invoke_url#22|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{path.lstrip('/')}"
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigatewayv2__cloudwatch: no HTTP API metric can be produced — the published ApiEndpoint is a real-AWS execute-api host the emulator does not serve, so no request is ever counted

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Cause 1 of 1: every HTTP API metric check stops at endpoint resolution because the api's published invoke address is an AWS public execute-api hostname the emulator does not serve, so not one request reaches the stage and no CloudWatch series (Count, 4XX, 5XX, detailed per-route/method series, alarms) is ever emitted.

AWS: Smithy (service-com-amazonaws-apigatewayv2-apigatewayv2): CreateApi/GetApi output member ApiEndpoint (jsonName apiEndpoint), documented as "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com" — the address the API is served on. On AWS, each request through that address publishes AWS/ApiGateway metrics with the ApiId and Stage dimensions (and Method/Resource/Route when detailed metrics are enabled), a route throttle of 0 produces 429s counted in 4xx, a missing lambda:InvokeFunction grant produces 500s counted in 5xx, and a 5XX alarm transitions and notifies its configured actions.
Emulator: the published value is "https://<api-id>.execute-api.<region>.amazonaws.com", a host the emulator does not listen on; replay refuses it with simparity.published.PublishedUrlRejected ("its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener"). All six checks abort before issuing a request, so the metric and alarm contracts are entirely ungradeable.
Why: observability of an emulated API is unreachable for exactly the reason the data plane is: the only address a client is given points outside the emulator. A dashboard or alarm built against the emulator never receives a datapoint.
Fix: the ApiEndpoint construction in the apigatewayv2 crate's CreateApi/GetApi/ImportApi handlers and the execute-api front-door listener that must serve the published address; the metric emission path in the data-plane handler is only reachable once a request lands.
Related but not covering: 20261107T064927Z-apigatewayv2-cloudwatch-the-http-api-5xx-alarm-pipeline.md and 20261029T182452Z-cloudwatch-apigatewayv2-http-api-returns-its-own-generic-message-internal.md describe the same pipeline not settling but own other replay keys and do not name the published host.
Traces: apigatewayv2__cloudwatch.http-api-count-and-4xx-metrics-carry-the-apiid-stage-dimensions, apigatewayv2__cloudwatch.http-api-missing-invoke-permission-rejected-as-5xx-metric, apigatewayv2__cloudwatch.http-api-route-throttle-zero-rejected-429-counted-in-4xx, apigatewayv2__cloudwatch.obs-apigatewayv2-2-http-api-detailed-metrics-opt-in-adds-method-resource-series, apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-actions-disabled-notification-blocked, apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns.

## Evidence
Expected: requests against the api's published endpoint produce AWS/ApiGateway datapoints dimensioned by ApiId/Stage (plus Method/Resource under detailed metrics), 429s counted in 4XXError, 500s in 5XXError, and the 5XX alarm firing its actions. Actual: all six checks fail at context.py:572 with simparity.published.PublishedUrlRejected "endpoint_url was given https://<api-id>.execute-api.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>" — zero requests, zero datapoints.

## Steps to Reproduce
1. CreateApi(ProtocolType='HTTP') with an integration, route and $default stage (DefaultRouteSettings DetailedMetricsEnabled=true for the detailed case).
2. Read ApiEndpoint and issue requests against it — the host is not one the emulator serves.
3. Call cloudwatch GetMetricData/GetMetricStatistics for AWS/ApiGateway with ApiId/Stage dimensions: no datapoints exist.
4. Replay the six apigatewayv2__cloudwatch checks listed in trace_ids.
