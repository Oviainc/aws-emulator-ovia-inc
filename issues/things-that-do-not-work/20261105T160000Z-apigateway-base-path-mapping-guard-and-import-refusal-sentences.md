Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.delete-domain-name-with-mapping-rejected apigateway.delete-stage-referenced-by-base-path-mapping-conflict apigateway.delete-stage-with-base-path-mapping-conflict apigateway.apigw-import-rest-api-fail-on-warnings-rejects-and-leaves-no-api-behind
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.delete-domain-name-with-mapping-rejected|run#20|AssertionError|assert message == (
Issue-Key: apigateway.delete-stage-referenced-by-base-path-mapping-conflict|run#16|AssertionError|assert message == (
Issue-Key: apigateway.delete-stage-with-base-path-mapping-conflict|run#14|AssertionError|assert message == (
Issue-Key: apigateway.apigw-import-rest-api-fail-on-warnings-rejects-and-leaves-no-api-behind|run#9|AssertionError|assert message == expected_message, (
Issue-Commit: c8d80ea15f1454d07f388eb2b932b3a5430349f4

# API Gateway: the three refusal SENTENCES of the base-path-mapping guards and of `ImportRestApi`'s `failOnWarnings` are not sourced — the refusals themselves, their status and their error code now work

> _Split out of `issues/things-that-now-work/20261029T193654Z-apigateway-api-gateway-control-plane-is-over-permissive-6.md`, whose six causes are otherwise closed: the over-permissiveness is gone (every one of these calls is now refused, with the resource left in place), and what remains is the TEXT of three messages. Everything below is what that fix ESTABLISHED, so a later fixer starts from here rather than from scratch._

## Operation
DeleteRestApi

## Description

Cause 1 of 3: `DeleteRestApi`'s refusal for an API a `BasePathMapping` still names states a sentence that is not AWS's.

AWS: while a base path mapping of a custom domain name names a REST API, deleting that API is
refused with HTTP 400 `BadRequestException` and the API survives. The recorded check compares the
whole message verbatim (`run#20 assert message == (`) and its custom assertion text quotes only the
ACTUAL value ("unexpected message: …"), so the expectation cannot be read from the replay.

Emulator: refuses, 400 `BadRequestException`,
`Cannot delete REST API '<restApiId>', is referenced in base path mapping: <domainName>/<basePath>`
— built from state in the shape of this service's own RECORDED in-use sentences
(`Cannot delete authorizer '<name>', is referenced in method: <METHOD>/<path>`,
`Cannot delete model '<name>', is referenced in method request: <path>/<verb>`,
`Cannot delete vpc link. Vpc link '<id>', is referenced in [<a>:<m>:<r>] in format of
[RESTApiId:Method:Resource].`). The guard, the status and the code are what the unit now gets past.

Cause 2 of 3: `DeleteStage`'s refusal for a stage a `BasePathMapping` still names states a sentence
that is not AWS's. Same shape as cause 1, on two units (`run#16` / `run#14`), whose assertion text
prints the actual message alone. Emulator:
`Cannot delete stage '<stageName>', is referenced in base path mapping: <domainName>/<basePath>`.

Cause 3 of 3: `ImportRestApi`'s `failOnWarnings` refusal message. This is the SAME unsourced
sentence as `issues/things-that-do-not-work/20261103T120000Z-apigateway-importdocumentationparts-failonwarnings-refusal-sentence.md`
(see its list of fourteen rejected candidate wordings — the two importers share one rendering,
`put_rest_api::warnings_refusal`). What is NEW here is the recorded DOCUMENT, read out of the replay
a window at a time through a diagnostic refusal (whitespace collapsed):

```json
{"openapi": "3.0.1",
 "info": {"title": "sim2real-behav-apigw-import-<hex>", "version": "1.0"},
 "paths": {"/imported": {"post": {
   "responses": {"200": {"description": "ok",
     "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Item"}}}}},
   "x-amazon-apigateway-integration": {"type": "mock",
     "requestTemplates": {"application/json": "{\"statusCode\":200}"},
     "responses": {"default": {"statusCode": "200",
       "responseTemplates": {"application/json": "{\"imported\":true,\"title\":\"<title>\"}"}}}}}}},
 "components": {"schemas": {}}}
```

So the warning CONDITION is settled: ONE `$ref` that the document does not resolve
(`components.schemas` is empty), declared on the 200 RESPONSE's `application/json` schema of
`POST /imported`, and the method DOES carry a mock integration — so "no integration defined for
method" is not among the warnings AWS reports here (tried, and the document rules it out).

Emulator: `Warnings found during import:\n\tReference to model 'Item' not found. Ignoring.\n\tInvalid
format for model application/json for method POST` — the lead and the `\n\t` separator are recorded
(see `rest_api_import_security_warning.rs` and the apigatewayv2 CloudFormation recording), and both
warning sentences are the ones
`issues/things-that-do-not-work/20261029T200808Z-apigateway-api-gateway-tag-vpc-link-stage-and-import.md`
cause 10 records AWS answering for a missing model reference (`warnings[0]` / `warnings[1]`). What
differs for THIS document is therefore the warning SET or its order, not the lead: the pair above was
recorded for a Swagger 2.0 `GET` document, and this one is an OpenAPI 3.0.1 `POST` whose reference
sits in a response.

Why: a client that matches on a refusal's text to report which record blocks a delete, or which part
of its OpenAPI document failed, gets a sentence AWS never emits.

Fix: the three sentences, taken from the recorded responses for the four units above — none of them
readable from the replay, because each check compares the message verbatim and prints only what the
emulator answered. A fixer who can read any one of them closes that cause in one edit; guessing is
not a fix.

Traces: apigateway.delete-domain-name-with-mapping-rejected,
apigateway.delete-stage-referenced-by-base-path-mapping-conflict,
apigateway.delete-stage-with-base-path-mapping-conflict,
apigateway.apigw-import-rest-api-fail-on-warnings-rejects-and-leaves-no-api-behind

## Evidence
Replay after the fix (the four keys above are all the issue has left; the other three of the parent
report are `fixed` or `moved`):

- `AssertionError: unexpected message: "Cannot delete REST API '<id>', is referenced in base path mapping: <domain>/v1"` (cause 1)
- `AssertionError: Cannot delete stage 'prod', is referenced in base path mapping: <domain>/(none)` and the same for `<domain>/v1` (cause 2)
- `AssertionError: the rejection must name the warning it refused to tolerate, got "Warnings found during import:\n\tReference to model 'Item' not found. Ignoring.\n\tInvalid format for model application/json for method POST"` (cause 3)

The guards themselves are pinned over a table of inputs in
`services/service-com-amazonaws-apigateway-backplanecontrolservice/tests/all/base_path_mapping_references.rs`.

## Steps to Reproduce
1. `CreateRestApi`; `CreateDeployment(stageName="prod")`; `CreateDomainName`;
   `CreateBasePathMapping(basePath="v1", restApiId, stage="prod")`. Then `DeleteRestApi` and
   `DeleteStage`: both answer 400 `BadRequestException` and leave the record in place — compare each
   message against the recorded one (causes 1 and 2).
2. `ImportRestApi(failOnWarnings=true, body=<the document above>)`: it answers 400
   `BadRequestException` and creates no API — compare the message against the recorded one (cause 3).
