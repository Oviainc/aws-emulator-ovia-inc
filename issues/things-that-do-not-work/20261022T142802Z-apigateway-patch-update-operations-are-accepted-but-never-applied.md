Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-api-key-enable-disable-cycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-api-key-enable-disable-cycle apigateway-api-key-import-update-lifecycle apigateway-api-key-lifecycle-and-bulk-import apigateway-api-key-lifecycle apigateway-api-key-lifecycle-with-tags apigateway-api-key-update-lifecycle apigateway-apikey-create-patch-update-and-lifecycle apigateway-apikey-update-lifecycle apigateway-resource-method-crud-lifecycle apigateway-resource-method-integration-full-lifecycle apigateway-stage-api-key-toggle apigateway-stage-restricted-api-key-toggle apigw-patch-apikey-description-op-support
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetApiKey|2xx->2xx|$.enabled
Issue-Key: apigateway|GetApiKey|2xx->2xx|$.stageKeys
Issue-Key: apigateway|GetApiKey|2xx->2xx|$.tags
Issue-Key: apigateway|GetApiKey|2xx->2xx|$.description
Issue-Key: apigateway|GetApiKey|2xx->2xx|$.name
Issue-Key: apigateway|GetResource|2xx->2xx|$.path
Issue-Key: apigateway|GetResource|2xx->2xx|$.pathPart
Issue-Key: apigateway|GetMethod|2xx->2xx|$.authorizationType
Issue-Key: apigateway|GetMethod|2xx->2xx|$.apiKeyRequired
Issue-Key: apigateway|GetApiKey|2xx->2xx|$.createdDate
Issue-Key: apigateway|GetApiKey|2xx->2xx|$.lastUpdatedDate
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# PATCH update operations are accepted but never applied

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApiKey

## Description
Nine causes, one shared fault: API Gateway's PATCH-style update operations (UpdateApiKey, UpdateResource, UpdateMethod) answer the update request successfully but do not mutate stored state, so the following read-back returns the pre-update entity. The replay only ever reports the mismatch on the read-back step, but the defect is in the update handler's application of patchOperations.

Cause 1 of 9: UpdateApiKey `replace /enabled`.
AWS: UpdateApiKey is PATCH /apikeys/{apiKey} taking patchOperations (Smithy: UpdateApiKey input {apiKey @httpLabel @required, patchOperations: ListOfPatchOperation}, output shape ApiKey, success 200); PatchOperation has members op (Op enum: add, remove, replace, move, copy, test), path, value, from. A `replace` of `/enabled` with value `false` must be applied to the ApiKey, so a subsequent GetApiKey returns `enabled: false`.
Emulator: GetApiKey still returns `enabled: true` after the patch (replay detail `$.enabled: value_mismatch (expected False, got True)`), HTTP 200 both sides.
Why: a client that disables an API key sees a success response while the key stays enabled — a security-relevant silent no-op; any client that reads back to confirm its own write diverges from AWS.
Fix: the UpdateApiKey handler's patchOperations application and the state write it performs.
Traces: fixture `apigateway-api-key-enable-disable-cycle`, step `task#4`.

Cause 2 of 9: UpdateApiKey replace of `/description` and `/enabled` on an imported key.
AWS: as Cause 1; both members are patchable and are reflected in ApiKey output.
Emulator: GetApiKey returns the original values — `$.description: value_mismatch (expected 'updated description', got 'initial description')`, `$.enabled: value_mismatch (expected False, got True)`.
Why: same silent no-op, here across two members at once, so even a multi-op PATCH is dropped wholesale.
Fix: UpdateApiKey handler / state.
Traces: fixture `apigateway-api-key-import-update-lifecycle`, step `task#4` (also affects apigateway-api-key-lifecycle-and-bulk-import#task#4).

Cause 3 of 9: UpdateApiKey replace of `/name`, `/description`, `/enabled`.
AWS: ApiKey.name, .description and .enabled are all returned from GetApiKey after the patch.
Emulator: all three come back pre-update — `$.name` still the original generated key name instead of the renamed one, `$.description: expected 'updated description', got 'initial description'`, `$.enabled: expected True, got False`.
Why: rename-then-read is a standard console/SDK flow and returns stale data.
Fix: UpdateApiKey handler / state.
Traces: fixture `apigateway-api-key-lifecycle`, step `task#4` (also apigateway-api-key-lifecycle-with-tags#task#8).

Cause 4 of 9: UpdateApiKey replace of `/name`, `/description`, `/enabled` (second recorded shape of the same no-op).
Emulator: `$.description`, `$.enabled` and `$.name` all pre-update on GetApiKey.
Why/Fix: as Cause 3.
Traces: fixture `apigateway-api-key-update-lifecycle`, step `task#3` (also apigateway-apikey-create-patch-update-and-lifecycle#task#4, apigateway-apikey-update-lifecycle#task#3).

Cause 5 of 9: UpdateResource replace of `/pathPart` is not applied.
AWS: Resource has members id, parentId, pathPart and path (Smithy: shape com.amazonaws.apigateway#Resource); replacing `/pathPart` renames the segment and AWS recomputes `path` from the new segment, so GetResource returns both the new pathPart and the new full path.
Emulator: GetResource returns the pre-rename segment and path (`$.pathPart` and `$.path` still the originally created segment, recording expects the renamed one).
Why: callers that rename a resource keep routing against a path AWS no longer serves, and derived `path` values never update.
Fix: the UpdateResource handler's patch application, including recomputation of the derived `path` member in state.
Traces: fixture `apigateway-resource-method-crud-lifecycle`, step `task#5` (also apigateway-resource-method-integration-full-lifecycle#task#25).

Cause 6 of 9: UpdateMethod replace of `/authorizationType` is not applied.
AWS: Method.authorizationType is a string whose documented values are NONE, AWS_IAM, CUSTOM and COGNITO_USER_POOLS (Smithy: com.amazonaws.apigateway#Method.authorizationType); a replace patch to AWS_IAM is visible on GetMethod.
Emulator: GetMethod returns `authorizationType: 'NONE'` where the recording has `'AWS_IAM'`.
Why: the method keeps open access after a client believes it switched the method to IAM auth.
Fix: the UpdateMethod handler's patch application / state.
Traces: fixture `apigateway-resource-method-crud-lifecycle`, step `task#9`.

Cause 7 of 9: UpdateMethod replace of `/apiKeyRequired` is not applied.
AWS: Method.apiKeyRequired is a NullableBoolean reflected by GetMethod after the patch.
Emulator: GetMethod returns `apiKeyRequired: false` where the recording has `true`.
Why: API-key enforcement a caller switched on is not in effect, and read-back misreports it.
Fix: UpdateMethod handler / state.
Traces: fixture `apigateway-resource-method-integration-full-lifecycle`, step `task#10`.

Cause 8 of 9: UpdateApiKey `/enabled` toggle not applied in the stage-association flow.
Emulator: GetApiKey returns `enabled: true` where the recording has `false` (same no-op as Cause 1 reached through a different fixture path, so it is a separate recorded unit to re-replay).
Why/Fix: as Cause 1.
Traces: fixture `apigateway-stage-api-key-toggle`, step `task#12` (also apigateway-stage-restricted-api-key-toggle#task#12).

Cause 9 of 9: UpdateApiKey `replace /description` with the op spelled out per the patch-operations contract.
AWS: a `replace` on `/description` sets ApiKey.description; AWS also advances lastUpdatedDate on a successful patch.
Emulator: GetApiKey returns the original description (`expected 'sim2real-probe replaced', got 'sim2real-probe original'`) and an unchanged lastUpdatedDate.
Why: the single most common PATCH a client issues against an API key is a no-op.
Fix: UpdateApiKey handler / state, including the lastUpdatedDate bump on a successful patch.
Traces: fixture `apigw-patch-apikey-description-op-support`, step `task#6`.

Note: the unrelated open issue `20261022T053512Z-apigateway-createapi-returns-http-500-instead-of-201-blocking.md` covers ApiGatewayV2 CreateApi and does not cover any cause here.

## Evidence
Representative (Cause 1): fixture apigateway-api-key-enable-disable-cycle step task#4, operation GetApiKey, expected HTTP 200 / actual HTTP 200, divergence_key `service=apigateway|GetApiKey|status=200->200|keys=$.enabled,$.stageKeys,$.tags`, detail `$.enabled: value_mismatch (expected False, got True)`.
Cause 2: apigateway-api-key-import-update-lifecycle#task#4 — `$.description: value_mismatch (expected 'updated description', got 'initial description')`, `$.enabled: value_mismatch (expected False, got True)`.
Cause 3: apigateway-api-key-lifecycle#task#4 — `$.description`, `$.enabled` (expected True, got False), `$.name` (expected the '-renamed' name, got the original).
Cause 4: apigateway-api-key-update-lifecycle#task#3 — `$.description`, `$.enabled` (expected False, got True), `$.name` (expected the '-updated' name).
Cause 5: apigateway-resource-method-crud-lifecycle#task#5 — `$.path: value_mismatch (expected '/hello', got '/<created-segment>')`, `$.pathPart: value_mismatch`.
Cause 6: apigateway-resource-method-crud-lifecycle#task#9 — `$.authorizationType: value_mismatch (expected 'AWS_IAM', got 'NONE')`.
Cause 7: apigateway-resource-method-integration-full-lifecycle#task#10 — `$.apiKeyRequired: value_mismatch (expected True, got False)`.
Cause 8: apigateway-stage-api-key-toggle#task#12 — `$.enabled: value_mismatch (expected False, got True)`.
Cause 9: apigw-patch-apikey-description-op-support#task#6 — `$.description: value_mismatch (expected 'sim2real-probe replaced', got 'sim2real-probe original')`.
In every cause the HTTP status matches the recording (200), so the divergence is purely the un-applied state change.
Smithy evidence: UpdateApiKey PATCH /apikeys/{apiKey} -> ApiKey (200); PatchOperation {op: Op enum [add, remove, replace, move, copy, test], path, value, from}; Resource {id, parentId, pathPart, path}; Method {authorizationType, apiKeyRequired: NullableBoolean}.
Run summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, state_reset_failures=0, steps_total=1857, steps_passed=332.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay wire fixture `apigateway-api-key-enable-disable-cycle` from trace set simparity-wire-aws-v1 and observe step `task#4`: the preceding UpdateApiKey (`replace /enabled` = false) returns success, but GetApiKey still reports `enabled: true`.
3. Replay each of the other eight bundled units and observe the same pattern of a successful update followed by a stale read-back: apigateway-api-key-import-update-lifecycle#task#4, apigateway-api-key-lifecycle#task#4, apigateway-api-key-update-lifecycle#task#3, apigateway-resource-method-crud-lifecycle#task#5, apigateway-resource-method-crud-lifecycle#task#9, apigateway-resource-method-integration-full-lifecycle#task#10, apigateway-stage-api-key-toggle#task#12, apigw-patch-apikey-description-op-support#task#6.
4. All nine units must read back the patched values before this issue is closed.
