Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-import-configure-deploy-export-sdk
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-import-configure-deploy-export-sdk apigateway-integration-and-responses-workflow apigateway-integration-request-template-base64-blob-round-trip apigateway-multiple-methods-same-resource apigateway-resource-method-integration-full-lifecycle apigateway-rest-api-update-list-and-sdk-generation apigateway-stage-api-key-toggle apigateway-stage-restricted-api-key-toggle apigateway-tag-stage-workflow-and-cleanup apigateway-put-integration-aws-proxy-crud apigw-patch-integration-type-and-method-family apigateway-import-rest-api-with-basepath-parameter apigateway-create-resource-hierarchy apigw-patch-model-validator-authorizer-invalid-path
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|PutIntegration|2xx->2xx|$.responseTransferMode
Issue-Key: apigateway|GetIntegration|2xx->2xx|$.responseTransferMode
Issue-Key: apigateway|UpdateIntegration|2xx->2xx|$.responseTransferMode
Issue-Key: apigateway|GetResources|2xx->2xx|$.item[id="7qygd7aaaa"].resourceMethods.POST.methodIntegration.responseTransferMode
Issue-Key: apigateway|GetResources|2xx->2xx|$.item[id="7qygd7aaaa"].resourceMethods.PUT.methodIntegration.responseTransferMode
Issue-Key: apigateway|UpdateMethod|2xx->2xx|$.methodIntegration.responseTransferMode
Issue-Key: apigateway|GetResources|2xx->2xx|$.item[*]
Issue-Key: apigateway|PutIntegrationResponse|2xx->2xx|$.responseTemplates.application/json
Issue-Key: apigateway|CreateResource|2xx->2xx|$.id
Issue-Key: apigateway|UpdateModel|2xx->2xx|$.id
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# API Gateway integration/resource representation drops responseTransferMode and alters ids (9 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegration

## Description
Integration and Resource representations omit a modeled member, add one AWS omits, mis-serialise an empty response template, and mint identifiers in the wrong shape. Smithy: Integration carries responseTransferMode (ResponseTransferMode enum) on both PutIntegration's and GetIntegration's output; Integration.connectionType is ConnectionType; IntegrationResponse.responseTemplates is MapOfStringToString.

Causes 1-5 share one fault: responseTransferMode is never serialised. AWS reports responseTransferMode='BUFFERED' on every integration it returns (it is the default transfer mode), wherever an Integration is embedded. Emulator omits the member. Why: a client that reads an integration and PUTs it back loses the transfer mode, and SDK/CLI output differs from AWS's. Fix: the Integration serialiser (default the stored value to BUFFERED).
  Cause 1 of 9 - PutIntegration response. Trace: apigateway-import-configure-deploy-export-sdk#task#6 (11 steps in cluster).
  Cause 2 of 9 - GetIntegration. Trace: apigateway-integration-and-responses-workflow#task#6 (6 steps).
  Cause 3 of 9 - UpdateIntegration. Trace: apigateway-integration-and-responses-workflow#task#7 (3 steps).
  Cause 4 of 9 - GetResources, resourceMethods.<M>.methodIntegration. Trace: apigateway-multiple-methods-same-resource#task#10.
  Cause 5 of 9 - UpdateMethod, methodIntegration. Trace: apigw-patch-integration-type-and-method-family#task#11.

Cause 6 of 9 - GetResources for an imported API: besides the missing responseTransferMode, the embedded methodIntegration carries connectionType='INTERNET', which AWS does not emit for a MOCK integration. Why: a client comparing an imported API against AWS's shape sees a member AWS leaves unset. Fix: only serialise connectionType when the integration actually has one. Trace: apigateway-import-rest-api-with-basepath-parameter#task#2.

Cause 7 of 9 - PutIntegrationResponse with an empty response template. AWS serialises the map entry's value as null when the template body is empty; the emulator emits the empty string. Why: a client round-tripping the response template writes back `""` instead of an unset template. Fix: IntegrationResponse serialisation of empty template values. Trace: apigateway-import-configure-deploy-export-sdk#task#7.

Cause 8 of 9 - CreateResource identifier shape. AWS mints REST-API Resource ids as 6-character lowercase alphanumeric strings; the emulator mints 10-character strings whose last three characters are a fixed literal. The particular id in any run is not a requirement - its shape is, and the fixture asserts the shape. Why: a client (or a fixture) that validates or length-bounds a resource id rejects the emulator's. Fix: the id generator used for Resource. Trace: apigateway-create-resource-hierarchy#task#2 (3 steps).

Cause 9 of 9 - UpdateModel identifier shape: Model ids are likewise 6-character lowercase alphanumeric at AWS and 10-character at the emulator. Fix: the id generator used for Model. Trace: apigw-patch-model-validator-authorizer-invalid-path#task#7.

No open issue owns these clusters; 'API Gateway control-plane state and representation' covers other operations.

## Evidence
Causes 1-5: $.responseTransferMode (or $.methodIntegration.responseTransferMode / $.item[..].resourceMethods.<M>.methodIntegration.responseTransferMode) missing, expected 'BUFFERED'; status matched (201/200).
Cause 6: $.item[1] diverges - expected methodIntegration with responseTransferMode 'BUFFERED' and no connectionType, got one with connectionType 'INTERNET' and no responseTransferMode.
Cause 7: $.responseTemplates.application/json type_mismatch - expected null, got ''.
Cause 8/9: $.id shape assertion failed - expected a 6-character identifier, got a 10-character one.

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus).
2. Causes 1-5: PUT/GET/PATCH an integration and GET the resource tree (apigateway-import-configure-deploy-export-sdk#task#6, apigateway-integration-and-responses-workflow#task#6/#task#7).
3. Cause 7: PUT an integration response whose responseTemplates value is empty (apigateway-import-configure-deploy-export-sdk#task#7).
4. Causes 8/9: POST /restapis/{id}/resources and PATCH a model, then check the returned id's length/charset.
