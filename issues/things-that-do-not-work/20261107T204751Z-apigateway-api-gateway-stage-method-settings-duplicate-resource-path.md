Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-stage-method-settings-duplicate-spelling
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-stage-method-settings-duplicate-spelling apigw-patch-deployment-stage-invalid-path
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|UpdateStage|4xx->2xx|$.message
Issue-Key: apigateway|UpdateStage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|UpdateStage|4xx->2xx|status
Issue-Key: apigateway|GetStage|2xx->2xx|$.methodSettings./pets/GET
Issue-Commit: 3971b4aa426682b1b07d577a2cfc2e75e38df2fe

# API Gateway stage method settings: duplicate resource-path spellings are accepted (200) instead of refused, and the normalised methodSettings entry is missing (2 causes)

> _Note: this issue was identified against the code base at commit `3971b4aa426682b1b07d577a2cfc2e75e38df2fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Two net-new root causes, both apigateway, both in the UpdateStage method-settings patch path: the duplicate-spelling guard is absent, and the method setting that IS applied is not exposed under its normalised key.

Cause 1 of 2: UpdateStage accepts a patch that sets method settings for the same resource path + HTTP method under two different spellings, where AWS refuses the whole update with 400.
AWS: Smithy: UpdateStage (PATCH /restapis/{restApiId}/stages/{stageName}) takes `patchOperations` (ListOfPatchOperation) and returns Stage {methodSettings: MapOfMethodSettings, ...}; its modeled errors include BadRequestException (httpError 400). A stage's methodSettings map is keyed by "<resourcePath>/<httpMethod>", and a patch path names that key with the resource path's slashes escaped as `~1`. Two patch entries that denote the SAME resource path and HTTP method under different spellings (one escaped, one not) are a duplicate, and AWS refuses the update rather than silently collapsing or duplicating it. The refusal is 400 with the message: fixed text "Update Stage failed due to different method settings with the same resource path and http method. Please remove one of the duplicate method settings for method paths: ", then the duplicate method paths exactly as spelled in the request, comma-separated, then the fixed text " for accountId: ", the caller's account id, the fixed text " and restApiId: ", and the restApiId — i.e. the only variable parts are the request's own method-path spellings and the account id / restApiId from the request and state. The recorded response also carries x-amzn-errortype (shape not asserted by the fixture). (The precedence and exact sentence are recorded behavior; the model supplies the 400 BadRequestException classification.)
Emulator: the call is answered 200 with a Stage body and no error at all — `status: expected 400, got 200`, `$.message: missing`, `$headers.x-amzn-errortype: missing`. The conflicting patch is applied instead of rejected.
Why: a client that submits a contradictory method-settings patch (easy to do, since the escaped and unescaped spellings look like two different keys) believes the update succeeded. Which of the two conflicting settings actually takes effect is then unspecified, so throttling, caching and logging on that method silently differ from what the client configured — and a client that treats the 400 as its validation signal never learns its deployment template is wrong.
Fix: UpdateStage's patch validation — detect patch operations whose method-setting keys normalise to the same "<resourcePath>/<httpMethod>" and refuse the request with 400 BadRequestException and the sentence above before any state mutation.
Traces: apigateway-stage-method-settings-duplicate-spelling#task#10 (also apigw-patch-deployment-stage-invalid-path#task#11).

Cause 2 of 2: GetStage's methodSettings map has no entry under the normalised "<resourcePath>/<httpMethod>" key after a method-settings patch.
AWS: Smithy: GetStage -> Stage, member `methodSettings` is MapOfMethodSettings of MethodSetting {metricsEnabled, loggingLevel, dataTraceEnabled, throttlingBurstLimit, throttlingRateLimit, cachingEnabled, cacheTtlInSeconds, cacheDataEncrypted, requireAuthorizationForCacheControl, unauthorizedCacheControlHeaderStrategy}. A method setting patched for a resource path and verb is readable under the NORMALISED key "<resourcePath>/<httpMethod>" (slashes unescaped), and its value carries the patched fields plus AWS's defaults for every field the patch did not set (so the whole MethodSetting structure is present, not just the touched members).
Emulator: GetStage answers 200 but `$.methodSettings.<resourcePath>/<httpMethod>` is `missing (... got None)` — the normalised entry is absent, so the stage reports no method setting for a method whose settings were patched. This is the read-side companion of the already-open issue 20261104T130732Z-apigateway-updatestage-returns-an-extra-methodsettings-entry-keyed-by.md ("UpdateStage returns an extra methodSettings entry keyed by the un-normalised patch path"), which covers the EXTRA un-normalised key on the UpdateStage response; it does not cover GetStage having no normalised entry, so this cause is net-new and must be closed on its own.
Why: a client cannot read back the throttling/caching/logging configuration it just applied. Anything that reconciles desired vs actual stage state (CloudFormation/CDK drift detection, idempotent re-apply, a console render) concludes the setting was never applied and re-applies or reports drift forever.
Fix: the stage state's method-settings keying plus the GetStage/UpdateStage serializer — store and emit method settings under the normalised "<resourcePath>/<httpMethod>" key with the full MethodSetting structure defaulted.
Traces: apigateway-stage-method-settings-duplicate-spelling#task#9 (also #task#11).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — fixture apigateway-stage-method-settings-duplicate-spelling, step task#10, UpdateStage, expected 400 / actual 200:
  status: expected 400, got 200
  $.message: missing (expected 'Update Stage failed due to different method settings with the same resource path and http method. Please remove one of the duplicate method settings for method paths: ~1pets/GET, /pets/GET for accountId: <accountId> and restApiId: <restApiId>', got None)
  $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  Fixed text: everything except the two method-path spellings (taken verbatim from the request's patchOperations), the accountId and the restApiId (taken from the caller and the request). Also affected: apigw-patch-deployment-stage-invalid-path#task#11.

Cause 2 — fixture apigateway-stage-method-settings-duplicate-spelling, step task#9, GetStage, expected 200 / actual 200:
  $.methodSettings./pets/GET: missing (expected {'cacheDataEncrypted': False, 'cacheTtlInSeconds': 300, 'cachingEnabled': False, 'dataTraceEnabled': False, 'metricsEnabled': False, 'requireAuthorizationForCacheControl': True, 'throttlingBurstLimit': 50, 'throttlingRateLimit': <shaped-not-asserted>, 'unauthorizedCacheControlHeaderStrategy': 'SUCCEED_WITH_RESPONSE_HEADER'}, got None)
  The requirement is the presence of the normalised key with the patched values and defaults for the untouched members, not the particular numbers from this recording. Also affected: step task#11 of the same fixture.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. CreateRestApi, add a resource and a method (e.g. GET on /pets), CreateDeployment and a stage.
3. Cause 2: UpdateStage with patchOperations setting method-setting members under the escaped method path (e.g. op=replace, path=/~1pets~1GET/throttling/burstLimit). Then call GetStage — methodSettings has no entry keyed by the normalised '<resourcePath>/<httpMethod>'.
4. Cause 1: UpdateStage with two patch operations for the SAME resource path and verb spelled differently (one escaped '~1pets/GET', one unescaped '/pets/GET'). The emulator answers 200 and applies the patch; AWS answers 400 BadRequestException with the duplicate-method-settings sentence.
5. Or replay the fixture steps directly: apigateway-stage-method-settings-duplicate-spelling#task#10 and #task#9.
