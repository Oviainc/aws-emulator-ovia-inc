Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__sqs.aws-proxy-subtype-sqs-deletemessage-required-parameters-refused-else-deletes-the-message apigatewayv2__sqs.aws-proxy-subtype-sqs-purgequeue-required-queueurl-refused-else-empties-the-queue apigatewayv2__sqs.aws-proxy-subtype-sqs-receivemessage-required-queueurl-refused-else-reads-the-queue apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-delivers-to-the-queue
Issue-Key: apigatewayv2__sqs.aws-proxy-subtype-sqs-deletemessage-required-parameters-refused-else-deletes-the-message|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: apigatewayv2__sqs.aws-proxy-subtype-sqs-purgequeue-required-queueurl-refused-else-empties-the-queue|_error#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: apigatewayv2__sqs.aws-proxy-subtype-sqs-receivemessage-required-queueurl-refused-else-reads-the-queue|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-delivers-to-the-queue|run#18|PublishedUrlRejected|url = f"{ctx.stage_url(api['ApiEndpoint'], '$default')}/send"
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigatewayv2__sqs: 2 net-new causes — CreateIntegration accepts SQS AWS_PROXY subtypes with their required parameters unmapped, and the published ApiEndpoint is an AWS execute-api host the emulator does not serve

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegration

## Description
Two net-new root causes behind the four SQS integration-subtype checks. Both must be closed.

Cause 1 of 3... (corrected) Cause 1 of 2: an AWS_PROXY integration declaring an SQS subtype with an empty RequestParameters map is created (201) instead of being refused with BadRequestException (3 checks).
AWS: Smithy (service-com-amazonaws-apigatewayv2-apigatewayv2): CreateIntegration — POST /v2/apis/{ApiId}/integrations, 201; members IntegrationType (required), IntegrationSubtype, RequestParameters : IntegrationParameters; modeled errors BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429). For a first-class AWS service subtype API Gateway validates at create time that the subtype's required parameters are all present in the parameter mapping (or that passthrough is enabled) and otherwise answers 400 BadRequestException with "Operation: <IntegrationSubtype> requires enabling passthrough, or defining all of the following parameters in the parameter mapping: [<RequiredParam>, ...]" — fixed sentence, with the subtype name and the bracketed required-parameter list taken from the request and the subtype definition (SQS-DeleteMessage: [ReceiptHandle, QueueUrl]; SQS-PurgeQueue: [QueueUrl]; SQS-ReceiveMessage: [QueueUrl]).
Emulator: each call returns HTTP 201 with the integration created and RequestParameters {}, raising no error.
Why: a misconfiguration AWS refuses at deploy time is accepted here and only misbehaves at invoke time, so clients lose the guardrail and templates validated against the emulator fail in production.
Fix: the AWS_PROXY subtype required-parameter validation in the apigatewayv2 crate's CreateIntegration/UpdateIntegration handler (the per-subtype parameter table and its BadRequestException).
Traces: apigatewayv2__sqs.aws-proxy-subtype-sqs-deletemessage-required-parameters-refused-else-deletes-the-message, apigatewayv2__sqs.aws-proxy-subtype-sqs-purgequeue-required-queueurl-refused-else-empties-the-queue, apigatewayv2__sqs.aws-proxy-subtype-sqs-receivemessage-required-queueurl-refused-else-reads-the-queue.

Cause 2 of 2: the HTTP API's published ApiEndpoint is an AWS public execute-api hostname the emulator does not serve, so the SQS-SendMessage subtype never delivers to the queue (1 check).
AWS: Smithy: CreateApi/GetApi output member ApiEndpoint, documented as "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com" — the address the API is served on; a POST to a route whose AWS_PROXY integration carries IntegrationSubtype 'SQS-SendMessage' with QueueUrl/MessageBody mapped puts the message on the queue.
Emulator: the published value is "https://<api-id>.execute-api.<region>.amazonaws.com"; replay refuses it with simparity.published.PublishedUrlRejected ("its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>"), so no request is made and nothing is delivered.
Why: the documented discovery path sends clients off the emulator, so an HTTP-API-fronted queue cannot be driven at all.
Fix: the ApiEndpoint construction in the apigatewayv2 crate's CreateApi/GetApi handlers plus the execute-api front-door listener that must serve the published address.
Related but not covering: 20261029T193114Z-apigatewayv2-sqs-an-http-api-aws-proxy-integration.md reports SQS-SendMessage never delivering and owns another replay key; it does not name the published host.
Traces: apigatewayv2__sqs.aws-proxy-subtype-sqs-sendmessage-delivers-to-the-queue.

## Evidence
Cause 1 — expected: CreateIntegration(IntegrationType='AWS_PROXY', IntegrationSubtype='SQS-DeleteMessage', RequestParameters={}) -> 400 BadRequestException "Operation: SQS-DeleteMessage requires enabling passthrough, or defining all of the following parameters in the parameter mapping: [ReceiptHandle, QueueUrl]" (and the QueueUrl-only variants for SQS-PurgeQueue / SQS-ReceiveMessage). Actual: HTTP 201 with the integration created — "expected the call to be rejected, but it succeeded: {... 'IntegrationSubtype': 'SQS-DeleteMessage', 'IntegrationType': 'AWS_PROXY', 'RequestParameters': {}, ...}".
Cause 2 — expected: POST <ApiEndpoint>/<route> with the SQS-SendMessage subtype lands the message on the queue. Actual: simparity.published.PublishedUrlRejected at context.py:572 — "endpoint_url was given https://<api-id>.execute-api.us-west-2.amazonaws.com ... its host is not one of the emulator's own hosts".

## Steps to Reproduce
1. Create a queue and a role granting the relevant sqs action; CreateApi(ProtocolType='HTTP').
2. CreateIntegration with IntegrationType='AWS_PROXY', IntegrationSubtype='SQS-DeleteMessage' (then 'SQS-PurgeQueue', 'SQS-ReceiveMessage'), PayloadFormatVersion='1.0', CredentialsArn=<role>, RequestParameters={} — each returns 201 instead of 400 BadRequestException.
3. For cause 2: create the SQS-SendMessage integration with QueueUrl/MessageBody mapped, a route and a $default stage, read ApiEndpoint and POST to the route — the published host is not served.
4. Replay the four apigatewayv2__sqs checks in trace_ids.
