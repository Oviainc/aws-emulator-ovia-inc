Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-account-settings-and-resource-tagging
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-account-settings-and-resource-tagging apigateway-account-settings-update apigateway-account-update-error-paths apigateway-account-update-invalid-patch-path apigateway-account-update-invalid-role-error apigw-patch-account-cloudwatchrolearn-op-support apigateway-stage-access-log-settings-asymmetric-patch apigateway-stage-cache-cluster-flags-and-patch-op-grammar apigateway-stage-canary-settings-promote-and-remove apigateway-stage-method-settings-path-grammar apigw-patch-deployment-stage-invalid-path apigw-patch-documentation-invalid-path apigw-patch-model-validator-authorizer-invalid-path apigw-patch-restapi-resource-invalid-path apigw-patch-usageplan-usage-invalid-path
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|UpdateAccount|4xx->4xx|$.message
Issue-Key: apigateway|UpdateStage|4xx->4xx|$.message
Issue-Key: apigateway|UpdateDeployment|4xx->4xx|$.message
Issue-Key: apigateway|UpdateDocumentationPart|4xx->4xx|$.message
Issue-Key: apigateway|UpdateDocumentationVersion|4xx->4xx|$.message
Issue-Key: apigateway|UpdateModel|4xx->4xx|$.message
Issue-Key: apigateway|UpdateRequestValidator|4xx->4xx|$.message
Issue-Key: apigateway|UpdateAuthorizer|4xx->4xx|$.message
Issue-Key: apigateway|UpdateResource|4xx->4xx|$.message
Issue-Key: apigateway|UpdateUsagePlan|4xx->4xx|$.message
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# API Gateway patch-path refusals omit the op and the allowed-path list (10 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccount

## Description
Every Update* operation that takes `patchOperations` (Smithy: UpdateAccount/UpdateStage/UpdateDeployment/UpdateDocumentationPart/UpdateDocumentationVersion/UpdateModel/UpdateRequestValidator/UpdateAuthorizer/UpdateResource/UpdateUsagePlan all take ListOfPatchOperation and model BadRequestException @httpError 400) refuses an unsupported patch path with a single emulator-authored sentence instead of AWS's template. The status (400) and error code (BadRequestException) are already right everywhere below; only the message text diverges, and the message is a requirement.

AWS (common to causes 1 and 3-10): 400 BadRequestException whose message is the fixed template `Invalid patch path  '<path>' specified for op '<op>'. Must be one of: [<allowed paths>]` - note the two spaces after `path`. Variable parts: `<path>` and `<op>` are taken verbatim from the offending PatchOperation in the request; `<allowed paths>` is the fixed, per-resource list of patchable paths. Emulator (all of them): `Invalid patch path <path>` - the op and the allowed-path list are dropped. Why: a client (or the AWS CLI/console) that parses the refusal to show which paths are patchable gets no list, and an operator cannot tell which of several patch operations in the request was rejected. Fix: the shared patchOperations validator used by these handlers must render AWS's template and own a per-resource table of allowed paths.

Cause 1 of 10 - UpdateAccount. Allowed list `[/cloudwatchRoleArn]`. Expected `Invalid patch path  '/notARealPath' specified for op 'replace'. Must be one of: [/cloudwatchRoleArn]`; got `Invalid patch path /notARealPath`. Trace: apigateway-account-settings-and-resource-tagging#task#5 (8 steps in cluster).

Cause 2 of 10 - UpdateStage, accessLogSettings sub-path. AWS uses a DIFFERENT template here: `Invalid accessLogSettings path. Must be one of : [/accessLogSettings/destinationArn, /accessLogSettings/format]` (fixed text, no variable part). Emulator: `Invalid patch path /accessLogSettings`. Trace: apigateway-stage-access-log-settings-asymmetric-patch#task#8 (8 steps).

Cause 3 of 10 - UpdateDeployment. Allowed list `[/description]`. Trace: apigw-patch-deployment-stage-invalid-path#task#6.

Cause 4 of 10 - UpdateDocumentationPart. Allowed list `[/properties]`. Trace: apigw-patch-documentation-invalid-path#task#4.

Cause 5 of 10 - UpdateDocumentationVersion. Allowed list `[/description]`. Trace: apigw-patch-documentation-invalid-path#task#7.

Cause 6 of 10 - UpdateModel. Allowed list `[/description, /schema]`. Trace: apigw-patch-model-validator-authorizer-invalid-path#task#5.

Cause 7 of 10 - UpdateRequestValidator. Allowed list `[/name, /validateRequestParameters, /validateRequestBody]`. Trace: apigw-patch-model-validator-authorizer-invalid-path#task#8.

Cause 8 of 10 - UpdateAuthorizer. Allowed list `[/name, /type, /authType, /authorizerUri, /authorizerCredentials, /identitySource, /identityValidationExpression, /authorizerResultTtlInSeconds]`. Trace: apigw-patch-model-validator-authorizer-invalid-path#task#10.

Cause 9 of 10 - UpdateResource. Allowed list `[/parentId, /pathPart]`. Trace: apigw-patch-restapi-resource-invalid-path#task#5.

Cause 10 of 10 - UpdateUsagePlan. Allowed list `[/name, /description, /productCode, /quota/period, /quota/limit, /quota/offset, /throttle/rateLimit, /throttle/burstLimit, /apiStages/<apidId:stageName>/throttle/<resourcePath>/<httpMethod>/rateLimit, /apiStages/<apidId:stageName>/throttle/<resourcePath>/<httpMethod>/burstLimit, /apiStages/<apidId:stageName>/throttle]` (AWS's own list contains the typo `apidId`; reproduce it). Trace: apigw-patch-usageplan-usage-invalid-path#task#8.

Not covered by an open issue: the replay reported no owner for any of these clusters. The open issues 'API Gateway refusal contracts, part 1/2/3' cover other operations' refusal text, not the patchOperations path template.

## Evidence
Replay of simparity-wire-aws-v1, verdict=mismatch, status 400 expected and 400 received in all 10 clusters; only $.message diverges.
Cause 1: expected "Invalid patch path  '/notARealPath' specified for op 'replace'. Must be one of: [/cloudwatchRoleArn]", got 'Invalid patch path /notARealPath'.
Cause 2: expected 'Invalid accessLogSettings path. Must be one of : [/accessLogSettings/destinationArn, /accessLogSettings/format]', got 'Invalid patch path /accessLogSettings'.
Causes 3-10: expected "Invalid patch path  '/sim2realNoSuchField' specified for op 'replace'. Must be one of: [...]" with the per-resource list quoted in the description, got 'Invalid patch path /sim2realNoSuchField'.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. For cause 1: PATCH /account with patchOperations=[{op:replace, path:/notARealPath, value:x}] and compare $.message against the template above (fixture apigateway-account-settings-and-resource-tagging, step task#5).
3. Repeat per cause with the fixture#step listed for it; each cluster's other members are listed in the replay output's `affected`.
