Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__stepfunctions.aws-proxy-subtype-stepfunctions-stopexecution-required-executionarn-refused-else-aborts-the-run apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startexecution-subtype-execution-input-is-the-posted-body apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startexecution-without-states-permission-denied-starts-nothing apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startsyncexecution-subtype-returns-the-state-machine-output
Issue-Key: apigatewayv2__stepfunctions.aws-proxy-subtype-stepfunctions-stopexecution-required-executionarn-refused-else-aborts-the-run|_error#15|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startexecution-subtype-execution-input-is-the-posted-body|_v2_invoke_url#6|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{p}"
Issue-Key: apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startexecution-without-states-permission-denied-starts-nothing|_v2_invoke_url#9|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{p}"
Issue-Key: apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startsyncexecution-subtype-returns-the-state-machine-output|_v2_invoke_url#5|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{p}"
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigatewayv2__stepfunctions: 2 net-new causes — CreateIntegration accepts a StepFunctions-StopExecution subtype with ExecutionArn unmapped, and the published ApiEndpoint is an AWS execute-api host the emulator does not serve

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegration

## Description
Two net-new root causes behind the four Step Functions integration-subtype checks. Both must be closed.

Cause 1 of 2: an AWS_PROXY integration declaring IntegrationSubtype 'StepFunctions-StopExecution' with an empty RequestParameters map is created (201) instead of being refused (1 check).
AWS: Smithy (service-com-amazonaws-apigatewayv2-apigatewayv2): CreateIntegration — POST /v2/apis/{ApiId}/integrations, 201; members IntegrationType (required), IntegrationSubtype, RequestParameters : IntegrationParameters; modeled errors BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429). API Gateway validates a first-class subtype's required parameters at create time and otherwise answers 400 BadRequestException with "Operation: <IntegrationSubtype> requires enabling passthrough, or defining all of the following parameters in the parameter mapping: [<RequiredParam>, ...]" — fixed sentence, with the subtype name and the bracketed required list from the request and subtype definition (StepFunctions-StopExecution: [ExecutionArn]).
Emulator: the call returns HTTP 201 with the integration created and RequestParameters {}.
Why: a configuration AWS refuses at deploy time is accepted, so the failure surfaces only at invoke time (or silently does nothing) and templates validated against the emulator break in production.
Fix: the AWS_PROXY subtype required-parameter validation in the apigatewayv2 crate's CreateIntegration/UpdateIntegration handler.
Traces: apigatewayv2__stepfunctions.aws-proxy-subtype-stepfunctions-stopexecution-required-executionarn-refused-else-aborts-the-run.

Cause 2 of 2: the HTTP API's published ApiEndpoint is an AWS public execute-api hostname the emulator does not serve, so the StartExecution/StartSyncExecution subtypes are never invoked (3 checks).
AWS: Smithy: CreateApi/GetApi output member ApiEndpoint, documented as "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com" — the address the API is served on. A POST to a route whose AWS_PROXY integration carries IntegrationSubtype 'StepFunctions-StartExecution' starts an execution whose input is the posted body; 'StepFunctions-StartSyncExecution' returns the state machine's output to the caller; without a states:StartExecution grant on the integration's CredentialsArn no execution is started.
Emulator: the published value is "https://<api-id>.execute-api.<region>.amazonaws.com"; replay refuses it with simparity.published.PublishedUrlRejected ("its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>"), so no request is made and no execution is started.
Why: the only documented discovery path for the API sends clients off the emulator, so an HTTP-API-fronted state machine cannot be driven at all.
Fix: the ApiEndpoint construction in the apigatewayv2 crate's CreateApi/GetApi handlers plus the execute-api front-door listener that must serve the published address.
Related but not covering: 20261024T215142Z-apigatewayv2-sfn-http-api-routes-using-the-first.md reports these subtypes never answering under service apigatewayv2__sfn and owns other replay keys.
Traces: apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startexecution-subtype-execution-input-is-the-posted-body, apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startexecution-without-states-permission-denied-starts-nothing, apigatewayv2__stepfunctions.xsvc-apigatewayv2-1-startsyncexecution-subtype-returns-the-state-machine-output.

## Evidence
Cause 1 — expected: CreateIntegration(IntegrationType='AWS_PROXY', IntegrationSubtype='StepFunctions-StopExecution', RequestParameters={}) -> 400 BadRequestException "Operation: StepFunctions-StopExecution requires enabling passthrough, or defining all of the following parameters in the parameter mapping: [ExecutionArn]". Actual: HTTP 201 with the integration created — "expected the call to be rejected, but it succeeded: {... 'IntegrationSubtype': 'StepFunctions-StopExecution', 'IntegrationType': 'AWS_PROXY', 'RequestParameters': {}, ...}".
Cause 2 — expected: POST <ApiEndpoint>/<route> starts an execution whose input is the posted body / returns the sync output / starts nothing without the states grant. Actual: simparity.published.PublishedUrlRejected at context.py:572 — "endpoint_url was given https://<api-id>.execute-api.us-west-2.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts".

## Steps to Reproduce
1. Create a state machine and a role granting states:StartExecution/StopExecution; CreateApi(ProtocolType='HTTP').
2. CreateIntegration with IntegrationType='AWS_PROXY', IntegrationSubtype='StepFunctions-StopExecution', RequestParameters={} — it returns 201 instead of 400 BadRequestException.
3. For cause 2: create the StartExecution/StartSyncExecution integrations with their parameters mapped, a route and a $default stage, read ApiEndpoint and POST to the route — the published host is not served by the emulator.
4. Replay the four apigatewayv2__stepfunctions checks in trace_ids.
