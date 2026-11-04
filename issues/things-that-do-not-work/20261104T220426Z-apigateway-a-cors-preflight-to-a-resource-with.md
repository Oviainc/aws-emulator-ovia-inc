Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-cors-preflight-served-from-mock-options-header-mappings
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.apigw-cors-preflight-served-from-mock-options-header-mappings|_poll#11|Timeout|time.sleep(interval_s)
Issue-Commit: d9cf74aafe764d4a8806a821a1189d14d75bb9bb

# apigateway: a CORS preflight to a resource with a MOCK OPTIONS method is never answered, so the declared Access-Control-* integration-response mappings are never served

> _Note: this issue was identified against the code base at commit `d9cf74aafe764d4a8806a821a1189d14d75bb9bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
OPTIONS preflight on a deployed stage (execute-api) with a MOCK integration

## Description
One net-new root cause.

=== Cause 1 of 1: an OPTIONS preflight on a deployed stage resource whose OPTIONS method is a MOCK integration is never answered 200 with its declared Access-Control-* headers ===
AWS: recorded behavior only — the execute-api data plane is not described by the API Gateway Smithy model (the control-plane shapes involved are PutMethod with httpMethod OPTIONS and authorizationType NONE, PutIntegration with type MOCK and a requestTemplate returning {"statusCode": 200}, PutMethodResponse with responseParameters declaring method.response.header.Access-Control-Allow-Origin / -Methods / -Headers, and PutIntegrationResponse mapping those header values). Recorded behavior: a preflight request — OPTIONS on the resource path carrying Origin, Access-Control-Request-Method and Access-Control-Request-Headers — is answered 200 by the MOCK integration with exactly the Access-Control-* response headers the integration response declares, each header's value being the value declared in the integration-response mapping for that method (nothing is synthesised from the request), and with an empty body, because the MOCK integration response template is empty. The real GET on the same resource is served the MOCK body and carries the Access-Control-Allow-Origin value its own integration response maps. A sibling resource with no OPTIONS method does not answer the preflight at all.
Emulator: the preflight is never answered 200. The check polls OPTIONS <invoke-url>/cors with those three preflight headers until it sees 200 and the predicate is never satisfied, so the run ends with "Error: Check timed out after 60.0s" before any header, body or sibling-resource assertion is reached — i.e. the stage serves no response at all for the OPTIONS method, rather than serving a wrong one.
Why: a browser client gets no preflight response, so every cross-origin call to the API is blocked by the user agent before the real request is ever sent. The declared Access-Control-Allow-Origin/-Methods/-Headers contract is unobservable, and the negative half of the contract (a resource without an OPTIONS method must not answer a preflight) cannot be distinguished from this blanket non-answer.
Fix: the execute-api data-plane request router must match the OPTIONS method on a deployed resource and dispatch it to the MOCK integration handler, and the integration-response serializer must apply the method/integration response header mappings (Access-Control-*) and the empty MOCK response template.
Traces: apigateway.apigw-cors-preflight-served-from-mock-options-header-mappings. Related open issue 20261023T210711Z-apigateway-lambda-four-integration-authorizer-behaviour-checks-hang.md covers a MOCK CORS preflight under the apigateway__lambda heading, but it does not carry this check's replay key.

## Evidence
Suite output for apigateway.apigw-cors-preflight-served-from-mock-options-header-mappings: "Error: Check timed out after 60.0s".

The check builds a REST API with a /cors resource carrying a MOCK GET (body {"cors":"ok"}) and a MOCK OPTIONS, plus a sibling /nocors resource with only a GET, deploys a stage, and then polls:
  OPTIONS <invoke-url>/cors with headers Origin: https://app.sim2real.example, Access-Control-Request-Method: GET, Access-Control-Request-Headers: x-sim2real-probe
until status == 200 ("OPTIONS preflight <url> to be answered").

Expected (AWS): 200 with exactly
  access-control-allow-origin: <the Origin value declared in the integration-response mapping>
  access-control-allow-methods: <the declared method list, here GET,OPTIONS>
  access-control-allow-headers: <the declared header list, here Content-Type,X-Sim2real-Probe>
and an empty response body; then GET <invoke-url>/cors returns 200 with the MOCK body and the mapped Access-Control-Allow-Origin; and the preflight against /nocors is not answered 200.

Actual (emulator): the 200 never arrives — the poll loop exhausts and the check is killed at the suite's 60 s cap, so none of the header/body/sibling assertions is ever reached.

## Steps to Reproduce
1. CreateRestApi; take the root resource and CreateResource pathPart=cors and pathPart=nocors.
2. On /cors add a MOCK GET: PutMethod(httpMethod=GET, authorizationType=NONE), PutIntegration(type=MOCK, requestTemplates={'application/json': '{"statusCode": 200}'}), PutMethodResponse(statusCode=200, responseParameters declaring method.response.header.Access-Control-Allow-Origin), PutIntegrationResponse mapping that header.
3. On /cors add a MOCK OPTIONS the same way, with PutMethodResponse/PutIntegrationResponse declaring and mapping method.response.header.Access-Control-Allow-Origin, -Allow-Methods and -Allow-Headers.
4. On /nocors add only a MOCK GET.
5. CreateDeployment to a stage.
6. Send OPTIONS <invoke-url>/cors with Origin, Access-Control-Request-Method: GET and Access-Control-Request-Headers: x-sim2real-probe.
Observe that no 200 preflight response is ever returned (the request is not answered), instead of a 200 with exactly the three declared Access-Control-* headers and an empty body.
