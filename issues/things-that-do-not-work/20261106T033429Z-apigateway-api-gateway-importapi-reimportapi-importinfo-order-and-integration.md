Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-export-import-reimport-roundtrip
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-export-import-reimport-roundtrip apigatewayv2-import-reimport-error-recovery apigatewayv2-import-api-basepath-split apigatewayv2-import-api-failonwarnings-rejected
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|ImportApi|2xx->2xx|$.importInfo[*]
Issue-Key: apigateway|ReimportApi|2xx->2xx|$.importInfo[*]
Issue-Key: apigateway|ImportApi|2xx->2xx|$.warnings[*]
Issue-Key: apigateway|ReimportApi|2xx->2xx|$.warnings[*]
Issue-Commit: 15ef1adb3fb7a9db5ce7069e30860cdce3c8580a

# API Gateway ImportApi/ReimportApi importInfo order and integration-type warning text diverge from AWS

> _Note: this issue was identified against the code base at commit `15ef1adb3fb7a9db5ce7069e30860cdce3c8580a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportApi

## Description
The OpenAPI import path of the HTTP-API control plane (replay service token `apigateway`; operations modeled in com.amazonaws.apigatewayv2) returns the right status and the right number of advisory strings, but the strings themselves diverge from AWS: the `importInfo` entries come back in a different order, and the warning emitted for an unrecognised `x-amazon-apigateway-integration` type is emulator-authored text instead of AWS's sentence. FOUR causes, all four must be closed.

Cause 1 of 4: ImportApi returns its importInfo entries in the wrong order.
AWS: Smithy (com.amazonaws.apigatewayv2#ImportApi): PUT /v2/apis, output member `importInfo` targets __listOf__string — a list, so element order is part of the response. Element content/order is recorded behavior only: for an operation whose x-amazon-apigateway-integration object carries both of the ignored keys, the recorded 200 lists the `passthroughBehavior` advisory BEFORE the `responses` advisory, each worded `paths.<path>.<method>.x-amazon-apigateway-integration: key [<keyName>] will be ignored by API Gateway` (fixed text `: key [` ... `] will be ignored by API Gateway`; the variable parts are the JSON pointer of the extension object and the ignored key name taken from the submitted document).
Emulator: emits the same two advisories transposed — `importInfo[1]` is the `[responses]` advisory and `importInfo[2]` is the `[passthroughBehavior]` advisory.
Why: a client that reads importInfo positionally (or asserts/diffs the list, as an infrastructure pipeline comparing import reports across runs does) sees a different report for the same document, and any check pinned to the first/second advisory reads the wrong key.
Fix: the OpenAPI import translation that collects ignored-extension-key advisories for ImportApi/ReimportApi — emit them in the order AWS reports (the order the keys are processed in the integration extension), not the order of the emulator's own key table.
Traces: apigatewayv2-export-import-reimport-roundtrip#task#4 (also apigatewayv2-import-reimport-error-recovery#task#1).

Cause 2 of 4: ReimportApi returns its importInfo entries in the wrong order (same fault, separate operation and replay key).
AWS: Smithy (com.amazonaws.apigatewayv2#ReimportApi): PUT /v2/apis/{ApiId}, output member `importInfo` targets __listOf__string; recorded behavior only — the recorded 200 lists the `passthroughBehavior` advisory before the `responses` advisory, same wording template as cause 1.
Emulator: the two advisories are transposed.
Why: as cause 1 — and because reimport is the update path, the divergence is visible on every subsequent document revision too.
Fix: as cause 1 (shared import translation; ReimportApi must be fixed with ImportApi).
Traces: apigatewayv2-export-import-reimport-roundtrip#task#5 (also apigatewayv2-import-reimport-error-recovery#task#3).

Cause 3 of 4: ImportApi's warning for an unrecognised integration type is emulator-authored text, not AWS's sentence.
AWS: Smithy (com.amazonaws.apigatewayv2#ImportApi): output member `warnings` targets __listOf__string; the wording is recorded behavior only. When an operation's x-amazon-apigateway-integration declares a type AWS does not recognise, AWS does not report the bad enum value — it treats the integration as having no IntegrationType and warns `Unable to create integration for resource at path '<method> <path>': IntegrationType is a required parameter for creating integration.. Ignoring.` Fixed text: `Unable to create integration for resource at path '`, `': IntegrationType is a required parameter for creating integration.. Ignoring.` (including the doubled period AWS emits); the only variable part is the method and path, taken from the submitted document.
Emulator: returns `Unable to create integration for resource at path '<method> <path>': invalid integration type '<submitted type>'. Valid values are: AWS, AWS_PROXY, HTTP, HTTP_PROXY, MOCK.. Ignoring.` — a different diagnosis (bad enum rather than missing required parameter) plus an emulator-authored valid-values enumeration AWS never emits.
Why: the warning list is the documented contract of an import; clients that match on AWS's sentence (and the `failOnWarnings=true` flows that surface these strings to the operator) get text AWS never produces, and the message mis-states the rule — AWS's rule is that an unrecognised type leaves the required IntegrationType unset.
Fix: the warning generation in the OpenAPI integration-extension translation for ImportApi/ReimportApi — map an unrecognised integration type onto AWS's missing-IntegrationType warning instead of authoring an invalid-enum message.
Traces: apigatewayv2-import-api-basepath-split#task#8 (also apigatewayv2-import-api-failonwarnings-rejected#task#2).

Cause 4 of 4: ReimportApi's warning for an unrecognised integration type carries the same emulator-authored text (separate operation and replay key).
AWS: Smithy (com.amazonaws.apigatewayv2#ReimportApi): output member `warnings` targets __listOf__string; recorded behavior only — the recorded 200 carries the identical `IntegrationType is a required parameter for creating integration.. Ignoring.` sentence described in cause 3.
Emulator: the `invalid integration type '<type>'. Valid values are: ...` text again.
Why: as cause 3.
Fix: as cause 3; ReimportApi shares the translation and must be verified separately.
Traces: apigatewayv2-import-api-failonwarnings-rejected#task#4.

## Evidence
All four clusters have verdict `mismatch` with expected status 200 and actual status 200 — only the advisory strings diverge.

Cause 1 — key `service=apigateway|ImportApi|status=200->200|keys=$.importInfo[1],$.importInfo[2]`:
  `$.importInfo[1]: value_mismatch (expected 'paths./probe.get.x-amazon-apigateway-integration: key [passthroughBehavior] will be ignored by API Gateway', got 'paths./probe.get.x-amazon-apigateway-integration: key [responses] will be ignored by API Gateway')`
  `$.importInfo[2]: value_mismatch (expected 'paths./probe.get.x-amazon-apigateway-integration: key [responses] will be ignored by API Gateway', got 'paths./probe.get.x-amazon-apigateway-integration: key [passthroughBehavior] will be ignored by API Gateway')`
Cause 2 — key `service=apigateway|ReimportApi|status=200->200|keys=$.importInfo[1],$.importInfo[2]`: identical transposition detail.
Cause 3 — key `service=apigateway|ImportApi|status=200->200|keys=$.warnings[0]`:
  `$.warnings[0]: value_mismatch (expected "Unable to create integration for resource at path 'GET /pets': IntegrationType is a required parameter for creating integration.. Ignoring.", got "Unable to create integration for resource at path 'GET /pets': invalid integration type 'BOGUS'. Valid values are: AWS, AWS_PROXY, HTTP, HTTP_PROXY, MOCK.. Ignoring.")`
Cause 4 — key `service=apigateway|ReimportApi|status=200->200|keys=$.warnings[0]`: identical warning-text detail.

## Steps to Reproduce
Replay the wire corpus (trace set simparity-wire-aws-v1).
Causes 1 and 2: fixture apigatewayv2-export-import-reimport-roundtrip, steps task#4 (ImportApi) and task#5 (ReimportApi) — PUT an OpenAPI document whose `paths./probe.get.x-amazon-apigateway-integration` object carries both a `passthroughBehavior` and a `responses` key, then compare the returned `importInfo` list element by element: the two advisories are transposed relative to the recorded AWS response. Also reproduced by apigatewayv2-import-reimport-error-recovery steps task#1 and task#3.
Causes 3 and 4: fixture apigatewayv2-import-api-basepath-split step task#8 (ImportApi) and apigatewayv2-import-api-failonwarnings-rejected step task#4 (ReimportApi) — PUT an OpenAPI document whose `GET /pets` operation declares an x-amazon-apigateway-integration with an unrecognised `type` (e.g. `BOGUS`) and read `warnings[0]`: the emulator reports an invalid-enum message with a valid-values list instead of AWS's `IntegrationType is a required parameter for creating integration.. Ignoring.` sentence. Also reproduced by apigatewayv2-import-api-failonwarnings-rejected step task#2.
