Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.layer-function-update-rejected-for-a-layer-in-another-region lambda.layer-function-update-rejected-for-two-versions-of-one-layer lambda.layer-is-invisible-from-another-region lambda.layer-publish-rejected-for-more-than-fifteen-compatible-runtimes lambda.concurrent-function-update-conflict lambda.invoke-async-ec2-errors
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.layer-function-update-rejected-for-a-layer-in-another-region|_error#11|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda.layer-function-update-rejected-for-two-versions-of-one-layer|_error#9|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda.layer-is-invisible-from-another-region|_error#7|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda.layer-publish-rejected-for-more-than-fifteen-compatible-runtimes|run#12|AssertionError|assert msg == expected, f"expected the exact ValidationException message {expected!r}, got {msg!r}"
Issue-Key: lambda.concurrent-function-update-conflict|run#9|AssertionError|assert last_status == "InProgress", (
Issue-Key: lambda.invoke-async-ec2-errors|run#11|AssertionError|assert bad_message == "Could not parse request body into json: Could not parse payload into json: Unrecognized token 'not': was expecting (JSON String, Number,
Issue-Commit: 16aa7c4a4bcdb2e3d9f64cec0911f23896568089

# lambda: six net-new root causes — layer region/duplicate-version validation missing, layers not region-scoped, and three refusal messages diverge from AWS

> _Note: this issue was identified against the code base at commit `16aa7c4a4bcdb2e3d9f64cec0911f23896568089`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFunctionConfiguration

## Description
Six independent net-new root causes in the Lambda emulator, all grounded in failing behavioral checks of service `lambda`. Each must be closed before this issue is submitted. The open issue 20261025T022656Z-lambda-ten-control-plane-writes-are-accepted-that.md covers a different set of control-plane writes and does NOT cover any of the six below; 20261025T192150Z-lambda-invokewithresponsestream-refused-notimplemented-updatealias-putresourcepolicy-refusal-messages.md covers UpdateAlias/PutResourcePolicy messages, not these.

Cause 1 of 6: UpdateFunctionConfiguration accepts a layer ARN from a region other than the function's region.
AWS: Smithy — UpdateFunctionConfiguration.Layers targets LayerList; the operation models InvalidParameterValueException (errorKind client, httpError 400). AWS refuses a cross-region layer reference with HTTP 400 InvalidParameterValueException and the message "Layers are not in the same region as the function. Layers are expected to be in region <region>." Fixed text: "Layers are not in the same region as the function. Layers are expected to be in region " and the trailing "."; the single variable part is <region>, the region of the function being updated, taken from the request's endpoint/function ARN. The function's attached layer list is unchanged by the refusal.
Emulator: the call SUCCEEDS (2xx, no error raised at all) — the check's helper reports "the call succeeded; real AWS rejects it, so there is no error code" — so a foreign-region LayerVersionArn is attached to the function.
Why: a client that relies on AWS refusing cross-region layer references silently ends up with a function configuration that cannot exist in AWS; the deployment appears to succeed in the emulator and then fails at the first real deploy, and no InvalidParameterValueException handling path is ever exercised.
Fix: UpdateFunctionConfiguration's input validation in the lambda service crate (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/update_function_configuration.rs) — the Layers member needs a region check against the function's own region before the configuration is mutated.
Traces: lambda.layer-function-update-rejected-for-a-layer-in-another-region

Cause 2 of 6: UpdateFunctionConfiguration accepts two different versions of the SAME layer in one Layers list.
AWS: Smithy — UpdateFunctionConfiguration models InvalidParameterValueException (client, 400). AWS refuses a Layers list containing two version ARNs that share a layer with HTTP 400 InvalidParameterValueException and the message "Two different versions of the same layer are not allowed to be referenced in the same function. <layerVersionArnA> and <layerVersionArnB> are versions of the same layer." Fixed text: "Two different versions of the same layer are not allowed to be referenced in the same function. ", " and ", " are versions of the same layer."; the two variable parts are the two conflicting LayerVersionArns taken verbatim from the request's Layers list. The previously attached layer version stays attached.
Emulator: the call SUCCEEDS with no error (helper reports "the call succeeded; real AWS rejects it, so there is no error code"), so both versions of one layer are recorded on the function.
Why: the emulator permits a function configuration AWS rejects; clients cannot test the duplicate-layer refusal, and state diverges because the function now carries two versions of one layer.
Fix: UpdateFunctionConfiguration's input validation (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/update_function_configuration.rs) — group the requested layer version ARNs by their layer ARN and refuse any group with more than one member.
Traces: lambda.layer-function-update-rejected-for-two-versions-of-one-layer

Cause 3 of 6: the layer store is not region-scoped — a layer published in one region is visible and readable from another region.
AWS: Smithy — GetLayerVersion models ResourceNotFoundException (client, httpError 404) and binds GET /2018-10-31/layers/{LayerName}/versions/{VersionNumber}. Layers are regional resources: a layer published in region A does not appear in ListLayerVersions/ListLayers in region B, and GetLayerVersion for it in region B is refused with HTTP 404 ResourceNotFoundException whose message is the fixed text "The resource you requested does not exist." (no variable parts).
Emulator: GetLayerVersion against the other region SUCCEEDS (helper reports "the call succeeded; real AWS rejects it, so there is no error code"), i.e. the layer store is global rather than per-region.
Why: region isolation is a correctness property clients depend on — a multi-region deployment that forgets to republish a layer per region will pass against the emulator and fail in AWS, and the ResourceNotFoundException branch is untestable.
Fix: the layer state store must be keyed by region, and GetLayerVersion/ListLayerVersions/ListLayers must only resolve layers in the request's region (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/get_layer_version.rs and the layer state it reads).
Traces: lambda.layer-is-invisible-from-another-region

Cause 4 of 6: PublishLayerVersion's ValidationException message omits the rejected value, so it does not match AWS's constraint-violation wording.
AWS: Smithy — PublishLayerVersion.CompatibleRuntimes is @length(min:0, max:15). A request with 16 runtimes is refused with ValidationException and the message "1 validation error detected: Value '[<comma-space-joined runtimes as submitted>]' at 'compatibleRuntimes' failed to satisfy constraint: Member must have length less than or equal to 15". Fixed text: "1 validation error detected: Value '[", "]' at 'compatibleRuntimes' failed to satisfy constraint: Member must have length less than or equal to 15"; the variable part is the submitted CompatibleRuntimes list rendered from the request. The refused publish must create no layer version.
Emulator: returns ValidationException with the message "1 validation error detected: Value at 'compatibleRuntimes' failed to satisfy constraint: Member must have length less than or equal to 15" — the "Value '[...]' " segment naming the rejected value is missing entirely.
Why: AWS's validation messages are parsed and surfaced by SDK/CLI users and by test assertions; dropping the offending value makes the diagnostic unusable and any client string-matching AWS's shape fails.
Fix: the constraint-violation serializer used by PublishLayerVersion (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/publish_layer_version.rs and the shared validation-message builder) must interpolate the submitted member value into the "Value '...'" slot.
Traces: lambda.layer-publish-rejected-for-more-than-fifteen-compatible-runtimes

Cause 5 of 6: UpdateFunctionCode never opens an in-flight window, so a concurrent mutation is accepted instead of refused with ResourceConflictException.
AWS: Smithy — UpdateFunctionCode and UpdateFunctionConfiguration both return FunctionConfiguration carrying LastUpdateStatus (enum, values include InProgress/Successful/Failed), and both model ResourceConflictException (errorKind client, httpError 409). After UpdateFunctionCode, LastUpdateStatus is InProgress until the update settles; a second mutation issued during that window is refused with HTTP 409 ResourceConflictException whose message is "The operation cannot be performed at this time. An update is in progress for resource: <functionArn>" — fixed text "The operation cannot be performed at this time. An update is in progress for resource: " plus the one variable part, the function's ARN from emulator state. The refused update must have zero effect on the configuration.
Emulator: UpdateFunctionCode settles synchronously — three successive padded (~10 MB) UpdateFunctionCode calls each read back LastUpdateStatus='Successful' and never 'InProgress' ("no padded UpdateFunctionCode read back LastUpdateStatus=InProgress in 3 attempts (last 'Successful'), so no update was ever in flight to conflict with"), so the concurrent UpdateFunctionConfiguration is accepted rather than refused with 409.
Why: clients that implement update serialization and retry-on-409 around Lambda deploys get no signal here; a deploy tool's conflict-handling path is never exercised, and the emulator reports a lifecycle (no Pending/InProgress phase) that AWS never shows.
Fix: the function-update lifecycle in state plus the two update handlers (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/update_function_code.rs and .../update_function_configuration.rs) — record an in-flight update with LastUpdateStatus=InProgress and refuse overlapping mutations with ResourceConflictException.
Traces: lambda.concurrent-function-update-conflict

Cause 6 of 6: InvokeAsync's InvalidRequestContentException message is truncated to its first clause.
AWS: Smithy — InvokeAsync (deprecated, POST /2014-11-13/functions/{FunctionName}/invoke-async, success code 202) models InvalidRequestContentException (errorKind client, httpError 400). For a non-JSON InvokeArgs payload AWS returns that error with the message "Could not parse request body into json: Could not parse payload into json: Unrecognized token '<token>': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: <line>, column: <column>]". Fixed text: everything except the three variable parts — <token>, the first unparseable token of the submitted payload, and <line>/<column>, its position in that payload.
Emulator: returns InvalidRequestContentException with the message "Could not parse request body into json" only — the nested parser clause, the offending token and the source/line/column trailer are all absent.
Why: the message is the only place the caller learns WHY its payload was rejected; clients and tests that match AWS's wording (or parse the token/position out of it) break, and the divergence hides whether the emulator parsed the payload at all.
Fix: InvokeAsync's payload parsing and error construction (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke_async.rs) must build the full InvalidRequestContentException message from the parser's token and position; the same message is shared with Invoke's payload validation.
Traces: lambda.invoke-async-ec2-errors

## Evidence
All six from the behavioral suite replay (service lambda, 98/232 passed), none marked ALREADY FILED or OWNED.

Cause 1 — lambda.layer-function-update-rejected-for-a-layer-in-another-region [mismatch]:
  expected: HTTP 400 InvalidParameterValueException, message "Layers are not in the same region as the function. Layers are expected to be in region <region>."
  actual:   update_function_configuration(FunctionName=fn, Layers=[<foreign-region LayerVersionArn>]) returned success; AssertionError: "the call succeeded; real AWS rejects it, so there is no error code"

Cause 2 — lambda.layer-function-update-rejected-for-two-versions-of-one-layer [mismatch]:
  expected: HTTP 400 InvalidParameterValueException, message "Two different versions of the same layer are not allowed to be referenced in the same function. <arnV1> and <arnV2> are versions of the same layer."
  actual:   update_function_configuration(FunctionName=fn, Layers=[v1Arn, v2Arn]) returned success; AssertionError: "the call succeeded; real AWS rejects it, so there is no error code"

Cause 3 — lambda.layer-is-invisible-from-another-region [mismatch]:
  expected: get_layer_version(LayerName=..., VersionNumber=1) in a different region -> HTTP 404 ResourceNotFoundException, message "The resource you requested does not exist."
  actual:   the cross-region GetLayerVersion returned success; AssertionError: "the call succeeded; real AWS rejects it, so there is no error code"

Cause 4 — lambda.layer-publish-rejected-for-more-than-fifteen-compatible-runtimes [mismatch]:
  expected message: "1 validation error detected: Value '[nodejs18.x, nodejs20.x, ... , provided.al2]' at 'compatibleRuntimes' failed to satisfy constraint: Member must have length less than or equal to 15"
  actual message:   "1 validation error detected: Value at 'compatibleRuntimes' failed to satisfy constraint: Member must have length less than or equal to 15"
  (error code ValidationException matched; only the "Value '[...]'" segment is missing. The runtime names shown are that run's request values, not part of the requirement — the requirement is that the submitted list is interpolated.)

Cause 5 — lambda.concurrent-function-update-conflict [mismatch]:
  expected: after UpdateFunctionCode with a ~10 MB package, GetFunctionConfiguration reads LastUpdateStatus='InProgress'; a concurrent UpdateFunctionConfiguration(Timeout=20) is then refused HTTP 409 ResourceConflictException with message "The operation cannot be performed at this time. An update is in progress for resource: <functionArn>" and has zero effect
  actual:   AssertionError: "no padded UpdateFunctionCode read back LastUpdateStatus=InProgress in 3 attempts (last 'Successful'), so no update was ever in flight to conflict with"

Cause 6 — lambda.invoke-async-ec2-errors [mismatch]:
  expected message: "Could not parse request body into json: Could not parse payload into json: Unrecognized token 'not': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1]"
  actual message:   "Could not parse request body into json"
  AssertionError: "expected the exact InvalidRequestContentException message AWS returns, got 'Could not parse request body into json'"
  (error code InvalidRequestContentException matched; 'not' and line/column 1 are the values of that run's payload b"not-json-at-all", the template is the requirement.)

## Steps to Reproduce
Replay these behavioral checks against the emulator:
  - lambda.layer-function-update-rejected-for-a-layer-in-another-region
  - lambda.layer-function-update-rejected-for-two-versions-of-one-layer
  - lambda.layer-is-invisible-from-another-region
  - lambda.layer-publish-rejected-for-more-than-fifteen-compatible-runtimes
  - lambda.concurrent-function-update-conflict
  - lambda.invoke-async-ec2-errors

By hand:
1. Cause 1: PublishLayerVersion twice for one layer; CreateFunction with version 1 attached; call UpdateFunctionConfiguration with the version-2 ARN rewritten to a different region. Expect 400 InvalidParameterValueException and version 1 still attached; observe success.
2. Cause 2: with the same two versions, call UpdateFunctionConfiguration with Layers=[v1Arn, v2Arn]. Expect 400 InvalidParameterValueException; observe success.
3. Cause 3: PublishLayerVersion in region A, then call ListLayers/ListLayerVersions and GetLayerVersion(VersionNumber=1) against region B. Expect the layer absent and GetLayerVersion 404 ResourceNotFoundException "The resource you requested does not exist."; observe success.
4. Cause 4: PublishLayerVersion with 15 CompatibleRuntimes (succeeds), then with 16. Compare the ValidationException message to AWS's "1 validation error detected: Value '[...]' at 'compatibleRuntimes' ...".
5. Cause 5: CreateFunction and wait Active; UpdateFunctionCode with a ~10 MB padded zip; immediately GetFunctionConfiguration (expect LastUpdateStatus=InProgress) and immediately call UpdateFunctionConfiguration(Timeout=20) (expect 409 ResourceConflictException and Timeout unchanged).
6. Cause 6: InvokeAsync(FunctionName=<existing fn>, InvokeArgs=b"not-json-at-all") and compare the InvalidRequestContentException message to AWS's full nested-parser text.
