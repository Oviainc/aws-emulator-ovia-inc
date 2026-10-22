Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-documentation-version-invalid-update
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-documentation-version-invalid-update apigateway-gateway-response-customization-lifecycle apigateway-gateway-response-error-paths apigateway-gateway-response-lifecycle apigateway-gateway-responses-lifecycle apigateway-gatewayresponse-error-invalid-statuscode apigw-patch-op-enum-validation-vs-service
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|UpdateDocumentationVersion|4xx->4xx|$.message
Issue-Key: apigateway|PutGatewayResponse|4xx->4xx|$.message
Issue-Key: apigateway|UpdateRestApi|4xx->4xx|$.message
Issue-Key: apigateway|UpdateApiKey|4xx->4xx|$.message
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# Validation error messages omit the input shape prefix

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDocumentationVersion

## Description
Four causes, one shared fault: the emulator's input-validation messages are built with the right template but the wrong member path and the wrong rendering of the constraint. These messages ARE a requirement because they are constructed from the request — AWS names the offending value, the fully qualified member path rooted at the operation's input shape, and the constraint it failed — so the rule, not the recorded text, is what must match. The status (400) and the error class are already correct in every cause; only the constructed path/constraint rendering diverges.

AWS's rule, visible identically across all four recordings: `1 validation error detected: Value '<value>' at '<inputShapeLowerCamel>.<member path>' failed to satisfy constraint: Member must satisfy <constraint>`, where the member path is rooted at the operation input (e.g. `updateDocumentationVersionInput.patchOperations.1.member.op`), enum constraints are rendered as `enum value set: [add, remove, move, test, replace, copy]`, and pattern constraints are rendered as the bare regular expression with no anchors.

Cause 1 of 4: UpdateDocumentationVersion with an invalid patch `op`.
AWS: PatchOperation.op targets the Op enum (Smithy: com.amazonaws.apigateway#Op). AWS answers 400 with the member path `updateDocumentationVersionInput.patchOperations.1.member.op` and the enum value set listed as `[add, remove, move, test, replace, copy]`.
Emulator: 400 with the path `patchOperations.1.member.op` — the input-shape prefix is dropped — and the enum set listed in the model's declaration order `[add, remove, replace, move, copy, test]`.
Why: the message is the only machine-readable indication of WHICH member failed for these REST-JSON validation errors; clients and test suites that parse the path to map the error back to a request field (or that assert on it) cannot resolve `patchOperations.1.member.op` to an operation, and the differing value-set ordering breaks exact-match assertions.
Fix: the shared request-validation message builder — prefix the member path with the operation's input shape name in lowerCamel form and render enum value sets in AWS's recorded order.
Traces: fixture `apigateway-documentation-version-invalid-update`, step `task#4`.

Cause 2 of 4: PutGatewayResponse with an out-of-range statusCode.
AWS: 400 with path `putGatewayResponseInput.statusCode` and constraint `Member must satisfy regular expression pattern: [1-5]\d\d` — the pattern is emitted unanchored.
Emulator: path `statusCode` (no input-shape prefix) and the pattern emitted with anchors, `^[1-5]\d\d$`.
Why: as Cause 1, plus a client echoing the pattern to a user shows a regex AWS never emits.
Fix: the same validation message builder — carry the input-shape prefix and emit the pattern as modeled, without added anchors.
Traces: fixture `apigateway-gateway-response-customization-lifecycle`, step `task#3`; 5 affected steps across the gateway-response fixtures.

Cause 3 of 4: UpdateRestApi with an invalid patch `op`.
AWS: path `updateRestApiInput.patchOperations.1.member.op`, enum set `[add, remove, move, test, replace, copy]`.
Emulator: path `patchOperations.1.member.op`, enum set `[add, remove, replace, move, copy, test]`.
Why/Fix: as Cause 1 — recorded separately because it proves the prefix is derived per operation input shape, not a single constant.
Traces: fixture `apigw-patch-op-enum-validation-vs-service`, step `task#3`.

Cause 4 of 4: UpdateApiKey with an invalid patch `op`.
AWS: path `updateApiKeyInput.patchOperations.1.member.op`, same enum set rendering.
Emulator: path `patchOperations.1.member.op`, model-order enum set.
Why/Fix: as Cause 3.
Traces: fixture `apigw-patch-op-enum-validation-vs-service`, step `task#7`.

## Evidence
Representative (Cause 1): fixture apigateway-documentation-version-invalid-update step task#4, operation UpdateDocumentationVersion, expected HTTP 400 / actual HTTP 400, divergence_key `service=apigateway|UpdateDocumentationVersion|status=400->400|keys=$.message`, detail `$.message: value_mismatch (expected "1 validation error detected: Value 'invalid_op' at 'updateDocumentationVersionInput.patchOperations.1.member.op' failed to satisfy constraint: Member must satisfy enum value set: [add, remove, move, test, replace, copy]", got "1 validation error detected: Value 'invalid_op' at 'patchOperations.1.member.op' failed to satisfy constraint: Member must satisfy enum value set: [add, remove, replace, move, copy, test]")`.
Cause 2: apigateway-gateway-response-customization-lifecycle#task#3 — expected `... at 'putGatewayResponseInput.statusCode' ... regular expression pattern: [1-5]\d\d`, got `... at 'statusCode' ... pattern: ^[1-5]\d\d$`; affected_count=5.
Cause 3: apigw-patch-op-enum-validation-vs-service#task#3 — expected path `updateRestApiInput.patchOperations.1.member.op`, got `patchOperations.1.member.op`.
Cause 4: apigw-patch-op-enum-validation-vs-service#task#7 — expected path `updateApiKeyInput.patchOperations.1.member.op`, got `patchOperations.1.member.op`.
Smithy evidence: PatchOperation.op targets the Op enum whose declared values are [add, remove, replace, move, copy, test] (the emulator's order) while AWS renders the value set as [add, remove, move, test, replace, copy]; UpdateDocumentationVersion/UpdateRestApi/UpdateApiKey input shapes are UpdateDocumentationVersionRequest/UpdateRestApiRequest/UpdateApiKeyRequest, each carrying patchOperations.
Status (400) and error class match in all four causes; the divergence is the request-derived message path and constraint rendering only.
Run summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, state_reset_failures=0.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay wire fixture `apigateway-documentation-version-invalid-update` step `task#4` (PATCH with op='invalid_op') and compare the 400 body's `message`: the member path lacks the `updateDocumentationVersionInput.` prefix and the enum value set is in a different order.
3. Replay `apigateway-gateway-response-customization-lifecycle` step `task#3` (statusCode='999'): the path lacks `putGatewayResponseInput.` and the pattern is emitted anchored.
4. Replay `apigw-patch-op-enum-validation-vs-service` steps `task#3` and `task#7` for the UpdateRestApi and UpdateApiKey prefixes.
5. All four units must produce the recorded member path and constraint rendering before this issue is closed.
