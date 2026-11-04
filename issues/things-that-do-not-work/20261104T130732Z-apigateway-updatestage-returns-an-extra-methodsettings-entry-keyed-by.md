Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-stage-method-settings-path-grammar
Issue-Step: task#16
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-stage-method-settings-path-grammar
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|UpdateStage|2xx->2xx|$.methodSettings.~1pets~1GET
Issue-Commit: a2a6e8306000a785cb6320db1cc126a5187ad764

# UpdateStage returns an extra methodSettings entry keyed by the un-normalised patch path

> _Note: this issue was identified against the code base at commit `a2a6e8306000a785cb6320db1cc126a5187ad764`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
UpdateStage's response (the Stage representation) carries a methodSettings entry whose key is the method-setting segment of the patch path copied verbatim, including the escaped `~1` separator between the resource path and the HTTP method, so a stage ends up with a map entry no AWS stage ever has.

Cause 1 of 1 - Stage.methodSettings key normalisation on the UpdateStage response.

AWS: Smithy `Stage$methodSettings` targets `MapOfMethodSettings` and its documentation states the keys are "method paths defined as {resource_path}/{http_method} for an individual method override, or /*/* for overriding all methods in the stage" (model: services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json; UpdateStage = PATCH /restapis/{restApiId}/stages/{stageName}, output shape Stage, 200). So the key is `<resource_path>/<http_method>`: only the slashes INSIDE the resource path are escaped as `~1` (the escaping the JSON-Patch path requires), while the separator before the HTTP method is a real `/`, and the wildcard form collapses to `*/*`. There is therefore exactly ONE map entry per method, whichever spelling the patch path used. The recorded 200 response for this step contains no entry under the fully-escaped token (`~1pets~1GET` in the recording, where `pets` is the recording's resource path and `GET` its method - those two values come from the request, the `~1`/`/` grammar is the fixed part).

Emulator: the same step answers 200 but its methodSettings map carries an ADDITIONAL entry keyed with both separators escaped (`~1pets~1GET` in the recording), holding a fully-populated MethodSetting value (metricsEnabled/dataTraceEnabled/throttlingBurstLimit/throttlingRateLimit/cachingEnabled/cacheTtlInSeconds/cacheDataEncrypted/requireAuthorizationForCacheControl/unauthorizedCacheControlHeaderStrategy). Hypothesis: the patch-path parser uses the raw first path segment as the map key instead of unescaping it to `<resource_path>/<http_method>`, so each distinct spelling of the same method creates its own entry.

Why: a client that writes a per-method setting with the fully-escaped spelling and then reads the stage cannot find its setting under the documented `{resource_path}/{http_method}` key, and a client that enumerates methodSettings sees two entries (or a phantom one) for a single method - so per-method throttling/caching state read back from the emulator has no AWS counterpart and settings-diffing clients loop re-applying the patch.

Fix: normalise the method-setting key in UpdateStage's patch-path parser when storing it in stage state, and in the Stage serializer, so every spelling of the same method resolves to the single documented key (and the wildcard spelling to `*/*`).

Related but NOT owning this cluster: open issue 20261029T200808Z-apigateway-api-gateway-patchoperations-grammar-op-enum-precedence-and.md, whose Cause 9 reports the same normalisation fault on the GetStage read path (replay keys `apigateway|GetStage|2xx->2xx|$.methodSettings.~1pets~1GET`); it carries no replay key for the UpdateStage response, and the replay reported `owned_by: []` for this cluster, so the UpdateStage leg is unowned and ungated by that issue.

Traces: apigateway-stage-method-settings-path-grammar#task#16 (UpdateStage, expected 200 / actual 200).

## Evidence
Replay cluster `service=apigateway|UpdateStage|status=200->200|keys=$.methodSettings.~1pets~1GET` (fixture apigateway-stage-method-settings-path-grammar, step task#16), verdict mismatch, expected_status 200, actual_status 200:

  $.methodSettings.~1pets~1GET: extra (expected None, got {'metricsEnabled': False, 'dataTraceEnabled': False, 'throttlingBurstLimit': 5000, 'throttlingRateLimit': 9.0, 'cachingEnabled': False, 'cacheTtlInSeconds': 300, 'cacheDataEncrypted': False, 'requireAuthorizationForCacheControl': True, 'unauthorizedCacheControlHeaderStrategy': 'SUCCEED_WITH_RESPONSE_HEADER'})

Expected: the Stage returned by UpdateStage has no methodSettings key in which the separator before the HTTP method is escaped; the only key for that method is the documented `<resource_path>/<http_method>` form (`~1pets/GET` in the recording), plus `*/*` for the stage-wide override. Actual: that documented key AND a second, fully-escaped key for the same method. The key grammar is the requirement; `pets`/`GET` and the numeric setting values above are this recording's request/state values, not part of it.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus().
2. Locate the divergence cluster for fixture `apigateway-stage-method-settings-path-grammar`, step `task#16` (operation UpdateStage).
3. Equivalently by hand: create a REST API with a resource `/pets` and a GET method, create a stage, then PATCH /restapis/{restApiId}/stages/{stageName} with patchOperations whose method-setting paths address that method (both the `/~1pets/GET/...` and the fully-escaped `/~1pets~1GET/...` spellings).
4. Inspect the 200 response's `methodSettings` map: AWS has one entry per method under `<resource_path>/<http_method>` (and `*/*` for the wildcard); the emulator additionally returns an entry keyed with the separator escaped.
