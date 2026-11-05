Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-export-import-reimport-roundtrip
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-export-import-reimport-roundtrip apigatewayv2-import-api-basepath-split apigatewayv2-import-api-failonwarnings-rejected apigatewayv2-import-reimport-error-recovery
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|ImportApi|2xx->4xx|$.apiEndpoint
Issue-Key: apigateway|ImportApi|2xx->4xx|$.apiId
Issue-Key: apigateway|ImportApi|2xx->4xx|$.apiKeySelectionExpression
Issue-Key: apigateway|ImportApi|2xx->4xx|$.createdDate
Issue-Key: apigateway|ImportApi|2xx->4xx|$.description
Issue-Key: apigateway|ImportApi|2xx->4xx|$.disableExecuteApiEndpoint
Issue-Key: apigateway|ImportApi|2xx->4xx|$.importInfo
Issue-Key: apigateway|ImportApi|2xx->4xx|$.ipAddressType
Issue-Key: apigateway|ImportApi|2xx->4xx|$.name
Issue-Key: apigateway|ImportApi|2xx->4xx|$.protocolType
Issue-Key: apigateway|ImportApi|2xx->4xx|$.routeSelectionExpression
Issue-Key: apigateway|ImportApi|2xx->4xx|$.tags
Issue-Key: apigateway|ImportApi|2xx->4xx|$.version
Issue-Key: apigateway|ImportApi|2xx->4xx|$.warnings
Issue-Key: apigateway|ImportApi|2xx->4xx|status
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigateway ImportApi and ReimportApi reject specs AWS imports with warnings

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportApi

## Description
AWS's OpenAPI import is lenient: a path whose `x-amazon-apigateway-integration` is not a proxy integration (or is missing a required integration field) does not fail the import - AWS creates the api, skips that integration, and reports the problem in the `warnings` and `importInfo` output members. The emulator treats the same spec as fatal and answers 400, so no api is created at all. Three causes, each with its own fixture step.

Cause 1 of 3: ImportApi with a non-proxy integration on an HTTP api.
AWS: 200 with the full api body, plus `warnings` containing an entry of the form `Unable to create integration for resource at path '<method> <path>': Currently, an API with a protocol type of HTTP may only be associated with proxy integrations (AWS_PROXY, HTTP_PROXY). Ignoring.` and `importInfo` containing one `paths.<path>.<method>.x-amazon-apigateway-integration: key [<key>] will be ignored by API Gateway` entry per ignored vendor key. The fixed text is everything outside the angle brackets; the method, path and key names come from the submitted spec. The response also carries the server defaults `ipAddressType: ipv4`, `routeSelectionExpression: $request.method $request.path`, `apiKeySelectionExpression: $request.header.x-api-key`, `tags: {}` and `disableExecuteApiEndpoint: false`.
Emulator: 400 with no body - the import fails and no api exists.
Why: a client importing a real-world OpenAPI document (which routinely contains non-proxy or partially specified integrations) gets no api at all, instead of an api plus warnings it can inspect.
Fix: the ImportApi spec translator - downgrade per-path integration problems to `warnings`/`importInfo` and continue the import.
Traces: apigatewayv2-export-import-reimport-roundtrip#task#4.

Cause 2 of 3: ImportApi with failOnWarnings not set and an integration missing IntegrationType.
AWS: 200 with the api body and `warnings` containing `Unable to create integration for resource at path '<method> <path>': IntegrationType is a required parameter for creating integration.. Ignoring.` (the doubled period is AWS's text), plus the same server defaults as cause 1.
Emulator: 400 with no body.
Why: the failOnWarnings contract is inverted - a warning becomes a hard failure even when the caller did not ask for that.
Fix: the ImportApi failOnWarnings gate and the per-path integration translator.
Traces: apigatewayv2-import-api-basepath-split#task#8.

Cause 3 of 3: ImportApi of a valid initial spec in the error-recovery flow.
AWS: 200 with the api body, `warnings` and `importInfo` exactly as in cause 1.
Emulator: 400 with no body, so the recovery flow has nothing to reimport.
Why/Fix: as cause 1 - the same translator rejects the spec outright.
Traces: apigatewayv2-import-reimport-error-recovery#task#1.

## Evidence
Replay verdict `mismatch`, `status: expected 200, got 400` on all three clusters, with every modeled output member reported `missing`, including `$.warnings: missing (expected ["Unable to create integration for resource at path 'GET /probe': Currently, an API with a protocol type of HTTP may only be associated with proxy integrations (AWS_PROXY, HTTP_PROXY). Ignoring."])` and `$.importInfo: missing (expected ['paths./probe.get.x-amazon-apigateway-integration: key [requestTemplates] will be ignored by API Gateway', ...])`.

## Steps to Reproduce
Replay the corpus and PUT /v2/apis with an OpenAPI body whose GET /probe path carries a non-proxy x-amazon-apigateway-integration (fixture apigatewayv2-export-import-reimport-roundtrip step task#4). The emulator answers 400; AWS answers 200 with warnings/importInfo.
