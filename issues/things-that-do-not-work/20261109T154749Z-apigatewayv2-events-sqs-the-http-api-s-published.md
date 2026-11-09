Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-subtype-routes-to-custom-bus-rule-queue apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-without-events-permission-denied-delivers-nothing
Issue-Key: apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-subtype-routes-to-custom-bus-rule-queue|run#24|PublishedUrlRejected|url = f"{ctx.stage_url(api_endpoint, '$default')}/orders"
Issue-Key: apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-without-events-permission-denied-delivers-nothing|run#22|PublishedUrlRejected|allowed_url = f"{ctx.stage_url(api_endpoint, '$default')}/allowed"
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigatewayv2__events__sqs: the HTTP API's published ApiEndpoint is a real-AWS execute-api host the emulator does not serve, so the EventBridge-PutEvents integration subtype is never exercised

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Cause 1 of 1: both PutEvents-subtype checks stop at endpoint resolution because the api's published invoke address is an AWS public execute-api hostname the emulator does not serve, so neither the custom-bus delivery nor the denied-permission path is driven.

AWS: Smithy (service-com-amazonaws-apigatewayv2-apigatewayv2): CreateApi/GetApi output member ApiEndpoint (jsonName apiEndpoint), documented as "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com" — the address the API is actually served on. A request to a route whose AWS_PROXY integration carries IntegrationSubtype 'EventBridge-PutEvents' puts the event on the named bus (and a rule then delivers it to the target queue); with the integration's CredentialsArn lacking events:PutEvents, API Gateway answers the client an error and nothing is delivered.
Emulator: the value published is "https://<api-id>.execute-api.<region>.amazonaws.com", a host the emulator does not listen on; replay refuses to follow it with simparity.published.PublishedUrlRejected ("its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener"). No request is issued, so the subtype's delivery and denial semantics reach no verdict.
Why: a client can only reach the API through ApiEndpoint; published as an amazonaws.com name it leaves the emulator, so an event-driven estate fronted by an HTTP API cannot be driven at all.
Fix: the ApiEndpoint construction in the apigatewayv2 crate's CreateApi/GetApi/ImportApi handlers and the execute-api front-door listener that must serve the published address.
Related but not covering: 20261029T070719Z-apigatewayv2-events-sqs-an-http-api-route-with.md reports the same trajectory never answering but owns other replay keys and does not name the published host.
Traces: apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-subtype-routes-to-custom-bus-rule-queue, apigatewayv2__events__sqs.xsvc-apigatewayv2-1-putevents-without-events-permission-denied-delivers-nothing.

## Evidence
Expected: POST <ApiEndpoint>/<route> with an AWS_PROXY EventBridge-PutEvents integration puts the event on the custom bus and the rule's SQS target receives it; without the events:PutEvents grant nothing is delivered. Actual: simparity.published.PublishedUrlRejected at context.py:572 — "endpoint_url was given https://<api-id>.execute-api.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>"; no request is made.

## Steps to Reproduce
1. Create a custom EventBridge bus, a rule targeting an SQS queue, and an IAM role granting events:PutEvents.
2. CreateApi(ProtocolType='HTTP') + CreateIntegration(IntegrationType='AWS_PROXY', IntegrationSubtype='EventBridge-PutEvents', CredentialsArn=<role>) + route + $default stage.
3. Read ApiEndpoint and POST the event body to the route — the published host is not served by the emulator.
4. Replay both apigatewayv2__events__sqs checks in trace_ids.
