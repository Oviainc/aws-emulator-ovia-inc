Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-models-and-request-validators
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-models-and-request-validators apigateway-models-request-validators apigateway-account-cloudwatch-role-update apigw-patch-account-cloudwatchrolearn-op-support apigateway-account-configure-cloudwatch-role apigateway-account-settings-and-resource-tagging apigateway-account-settings-cloudwatch-role-and-throttle-lifecycle apigateway-account-settings-update
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetModel|2xx->4xx|$.contentType
Issue-Key: apigateway|GetModel|2xx->4xx|$.description
Issue-Key: apigateway|GetModel|2xx->4xx|$.name
Issue-Key: apigateway|GetModel|2xx->4xx|$.schema
Issue-Key: apigateway|GetModel|2xx->4xx|status
Issue-Key: apigateway|GetModels|2xx->2xx|$.item[id="{{empty_model_id}}"]
Issue-Key: apigateway|GetModels|2xx->2xx|$.item[id="{{error_model_id}}"]
Issue-Key: apigateway|GetModels|2xx->2xx|$.item[name="Empty"]
Issue-Key: apigateway|GetModels|2xx->2xx|$.item[name="Error"]
Issue-Key: apigateway|GetModelTemplate|2xx->2xx|$.value
Issue-Key: apigateway|DeleteRequestValidator|4xx->2xx|$
Issue-Key: apigateway|DeleteRequestValidator|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|DeleteRequestValidator|4xx->2xx|status
Issue-Key: apigateway|DeleteRequestValidator|2xx->4xx|$
Issue-Key: apigateway|DeleteRequestValidator|2xx->4xx|status
Issue-Key: apigateway|UpdateAccount|4xx->2xx|$.message
Issue-Key: apigateway|UpdateAccount|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|UpdateAccount|4xx->2xx|status
Issue-Key: apigateway|UpdateAccount|2xx->4xx|$.apiKeyVersion
Issue-Key: apigateway|UpdateAccount|2xx->4xx|$.features
Issue-Key: apigateway|UpdateAccount|2xx->4xx|$.throttleSettings
Issue-Key: apigateway|UpdateAccount|2xx->4xx|status
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# API Gateway REST-API defaults, delete contracts and UpdateAccount patch (8 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetModel

## Description
A new REST API is not created with API Gateway's default models, DeleteRequestValidator ignores method references, and UpdateAccount's /cloudwatchRoleArn patch is inverted - it accepts the role AWS refuses and refuses the role AWS accepts. Smithy: DeleteRequestValidator succeeds with 202 and models ConflictException @httpError 409; UpdateAccount returns Account {cloudwatchRoleArn, throttleSettings, features, apiKeyVersion} and models BadRequestException @httpError 400.

Cause 1 of 8 - every REST API is created with two default models. AWS: CreateRestApi seeds the models `Empty` (contentType application/json, description 'This is a default empty schema model', schema the draft-04 'Empty Schema' object) and `Error` (description 'This is a default error schema model', schema the draft-04 'Error Schema' with a `message` string property), so GetModel for either answers 200. Emulator: 404 - they do not exist. Why: imported/console-created APIs that reference the Empty/Error models cannot be configured, and method responses referencing them break. Fix: seed the default models in CreateRestApi (and in the import paths). Trace: apigateway-models-and-request-validators#setup#2 (2 steps).

Cause 2 of 8 - GetModels therefore omits both defaults from the collection. Closes with cause 1. Trace: apigateway-models-and-request-validators#task#7.

Cause 3 of 8 - the same omission in the second fixture (separate replay key); that recording also shows AWS's Model ids are 6-character lowercase alphanumeric strings. Trace: apigateway-models-request-validators#task#5.

Cause 4 of 8 - GetModelTemplate's generated mapping template. Recorded behavior only: AWS fills a string property's value with `foo`, the emulator writes the literal `string`. Why: the generated template a client copies differs from AWS's. Fix: the model-template generator's leaf placeholders. Trace: apigateway-models-and-request-validators#task#10.

Cause 5 of 8 - DeleteRequestValidator while a method still references the validator. AWS: 409 ConflictException `Cannot delete Request Validator '<validatorName>' referenced in method: <HTTPMETHOD>/<resourcePath>` (fixed text; the validator name and the referencing method come from state). Emulator: 202 - it deletes the validator. Why: the emulator lands in a state AWS forbids, with a method pointing at a validator that no longer exists. Fix: DeleteRequestValidator's reference check. Trace: apigateway-models-and-request-validators#task#19 (2 steps).

Cause 6 of 8 - consequence in that fixture: the later DeleteRequestValidator answers 404 NotFoundException `Invalid Request Validator identifier specified` instead of 202 with an empty body. Closes with cause 5. Trace: apigateway-models-and-request-validators#task#21 (2 steps).

Cause 7 of 8 - UpdateAccount replacing /cloudwatchRoleArn with a role API Gateway cannot assume. AWS: 400 BadRequestException `The role ARN does not have required permissions configured. Please grant trust permission for API Gateway and add the required role policy.` (fixed text). Emulator: 200 - it stores the role. Why: a client's validation of its logging role silently passes. Fix: UpdateAccount's cloudwatchRoleArn validation. Trace: apigateway-account-cloudwatch-role-update#task#2 (2 steps).

Cause 8 of 8 - UpdateAccount replacing /cloudwatchRoleArn with an acceptable role. AWS: 200 with the Account representation - apiKeyVersion, features (recorded ['UsagePlans']) and throttleSettings {burstLimit, rateLimit} present. Emulator: 400. Why: the account's CloudWatch logging role cannot be configured at all. Fix: the same handler - accept a well-formed assumable role and return the full Account. Trace: apigateway-account-configure-cloudwatch-role#task#2 (5 steps).

The recorded ids, role ARNs and limit values are that run's instances, not requirements. No open issue owns these clusters; the open 'API Gateway deployed-stage invoke, account logging role and log delivery' issue concerns log delivery on the data plane, not UpdateAccount's patch contract.

## Evidence
Cause 1: status expected 200 with $.name 'Empty', $.contentType 'application/json', $.description 'This is a default empty schema model' and the draft-04 Empty Schema; got 404.
Causes 2/3: $.item[name="Empty"] and $.item[name="Error"] missing from GetModels.
Cause 4: $.value expected the template with "name" : "foo", got "name" : "string".
Cause 5: status expected 409 with x-amzn-errortype ConflictException and "Cannot delete Request Validator '<name>' referenced in method: POST//widgets", got 202 with an empty body.
Cause 6: status expected 202 with an empty body, got 404 NotFoundException 'Invalid Request Validator identifier specified'.
Cause 7: status expected 400 with x-amzn-errortype BadRequestException and the quoted role-permission message, got 200.
Cause 8: status expected 200 with $.apiKeyVersion, $.features ['UsagePlans'] and $.throttleSettings {burstLimit 5000, rateLimit 10000.0}, got 400.

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus).
2. Causes 1-3: POST /restapis, then GET /restapis/{id}/models and GET /restapis/{id}/models/Empty (expect the seeded Empty/Error models).
3. Cause 5/6: create a request validator, attach it to a method, then DELETE it (expect 409 ConflictException).
4. Causes 7/8: PATCH /account with op=replace path=/cloudwatchRoleArn for (a) a role without the API Gateway trust policy (expect 400) and (b) an acceptable role (expect 200 with the full Account).
