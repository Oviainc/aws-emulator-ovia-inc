Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-integration-positive apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-missing-permission-negative apigateway__dynamodb__lambda__logs.ddb-esm-tumbling-window-aggregation-positive
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-integration-positive|run#14|PublishedUrlRejected|url = f"{ctx.stage_url(api['ApiEndpoint'], '$default')}/echo?msg=hello123"
Issue-Key: apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-missing-permission-negative|_poll_http_get_any_terminal#24|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: apigateway__dynamodb__lambda__logs.ddb-esm-tumbling-window-aggregation-positive|_poll_until#15|AssertionError|raise AssertionError(
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# HTTP API ApiEndpoint is published as <id>.execute-api.<region>.amazonaws.com, and DynamoDB ESM tumbling windows never emit a final aggregate invocation (2 causes)

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApi

## Description
Cause 1 of 2: the API's published endpoint names an AWS production host, not the address the emulator is reached on.
AWS: an HTTP API's ApiEndpoint is the DNS name a client actually connects to; every recorded invoke of the deployed API succeeds against the endpoint the control plane reports. Smithy (apigatewayv2) Api.ApiEndpoint is the invoke URL of the API.
Emulator: the endpoint is announced as 'https://<api-id>.execute-api.us-west-2.amazonaws.com' (and '<...>/hit'), a host the emulator does not serve; a client that honours the published value cannot reach the API at all. The replay refuses it with 'endpoint_url was given https://<api-id>.execute-api.us-west-2.amazonaws.com ... its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>'.
Why: the invoke URL is the one value a client must trust; announcing an address other than the one the emulator answers on makes every deployed HTTP API unusable even though the control-plane calls all succeed.
Fix: the ApiEndpoint serializer/state for apigatewayv2 (it must publish the emulator's own host and port for the API's execute-api listener).
Traces: apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-integration-positive, apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-missing-permission-negative.

Cause 2 of 2: a DynamoDB event-source mapping with a tumbling window never performs the window's final invocation.
AWS: an event source mapping carrying TumblingWindowInSeconds invokes the function once per window with the accumulated state and then performs a final invocation whose payload carries isFinalInvokeForWindow, so the handler can write the window total (Smithy lambda CreateEventSourceMapping.TumblingWindowInSeconds).
Emulator: no invocation ever writes a total above a single record — 'a tumbling-window final invocation writes a total above one record not met within 60.0s. Last result: []'. The window state is never aggregated and no final invocation is made.
Why: a client relying on tumbling-window aggregation sees per-record results only and never the window aggregate, so stream aggregation pipelines silently produce wrong output.
Fix: the event-source-mapping poller/state for DynamoDB streams in the Lambda crate (window accumulation and the final isFinalInvokeForWindow invocation).
Traces: apigateway__dynamodb__lambda__logs.ddb-esm-tumbling-window-aggregation-positive.

## Evidence
Cause 1 — expected: the published endpoint is reachable on the emulator. Actual: 'simparity.published.PublishedUrlRejected: endpoint_url was given https://mp67t4dbd4.execute-api.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts'.
Cause 2 — expected: a final windowed invocation writes an aggregate above one record. Actual: 'AssertionError: a tumbling-window final invocation writes a total above one record not met within 60.0s. Last result: []'.

## Steps to Reproduce
Replay apigateway__dynamodb__lambda__logs.apigw-http-api-lambda-integration-positive and .apigw-http-api-lambda-missing-permission-negative (both read ApiEndpoint and then invoke it), and .ddb-esm-tumbling-window-aggregation-positive (creates an ESM with TumblingWindowInSeconds, writes records and waits for the final aggregate invocation).
