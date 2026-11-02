Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-event-invoke-config-recursion-and-runtime-management
Issue-Step: task#105
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-event-invoke-config-recursion-and-runtime-management lambda-recursion-and-runtime-management-config
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|PutRuntimeManagementConfig|4xx->2xx|$.Type
Issue-Key: lambda|PutRuntimeManagementConfig|4xx->2xx|$.message
Issue-Key: lambda|PutRuntimeManagementConfig|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|PutRuntimeManagementConfig|4xx->2xx|status
Issue-Key: lambda|GetRuntimeManagementConfig|2xx->2xx|$.RuntimeVersionArn
Issue-Key: lambda|GetRuntimeManagementConfig|2xx->2xx|$.UpdateRuntimeOn
Issue-Key: lambda|PutRuntimeManagementConfig|4xx->4xx|$.Type
Issue-Key: lambda|PutRuntimeManagementConfig|4xx->4xx|$.message
Issue-Key: lambda|PutRuntimeManagementConfig|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|PutRuntimeManagementConfig|4xx->4xx|status
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda runtime management config is not persisted and its in-progress conflict is not raised

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRuntimeManagementConfig

## Description
Shared fault: PutRuntimeManagementConfig neither raises the in-progress-update conflict nor persists what it accepted. Three net-new clusters.

AWS: Smithy PutRuntimeManagementConfig (PUT /2021-07-20/functions/{FunctionName}/runtime-management-config, 200) takes UpdateRuntimeOn (required) and an optional RuntimeVersionArn, and binds ResourceConflictException (httpError 409, {Type, message}) and InvalidParameterValueException (400). Recorded behavior: while a function update is in flight the call is refused 409 ResourceConflictException, x-amzn-errortype `ResourceConflictException`, message `The operation cannot be performed at this time. An update is in progress for resource: <functionArn>` — fixed text with the function ARN from state; that conflict is evaluated BEFORE the parameter-combination checks. GetRuntimeManagementConfig then reports the stored `UpdateRuntimeOn` (`Manual`) and the stored `RuntimeVersionArn`.

Emulator: the in-progress conflict is never raised — one call answers 200 and another is refused 400 InvalidParameterValueException `RuntimeVersionArn can only be specified when UpdateRuntimeOn is Manual, not Auto` (the parameter check running where the conflict check belongs) — and GetRuntimeManagementConfig reports `UpdateRuntimeOn: "Auto"` with no RuntimeVersionArn, i.e. the accepted configuration was never stored.

Why: a client that sets a function to Manual runtime management and reads it back sees Auto, so a pinned runtime silently reverts; and a client that retries on 409 never sees the conflict, so it proceeds against a function that is still updating.

Fix: PutRuntimeManagementConfig's handler must check the function's in-flight-update state first and raise ResourceConflictException with AWS's message, then persist UpdateRuntimeOn/RuntimeVersionArn into function state so GetRuntimeManagementConfig can serve it.

Cause 1 of 3: PutRuntimeManagementConfig — fixture lambda-event-invoke-config-recursion-and-runtime-management step task#105; expected 409 ResourceConflictException with the in-progress message, emulator answered 200.
Cause 2 of 3: GetRuntimeManagementConfig, 200 — fixture lambda-event-invoke-config-recursion-and-runtime-management step task#3; `UpdateRuntimeOn` reported `Auto` instead of `Manual` and `RuntimeVersionArn` omitted — the Put was not persisted.
Cause 3 of 3: PutRuntimeManagementConfig — fixture lambda-recursion-and-runtime-management-config step task#10; expected 409 with the in-progress message, emulator answered 400 InvalidParameterValueException about the RuntimeVersionArn/UpdateRuntimeOn combination, so the checks run in the wrong order.

## Evidence
Cluster details: task#105 `status: expected 409, got 200` with `$.message: missing (expected 'The operation cannot be performed at this time. An update is in progress for resource: <arn>')` and `x-amzn-errortype: missing (expected 'ResourceConflictException')`; task#3 `$.RuntimeVersionArn: missing (expected 'arn:aws:lambda:us-west-2::runtime:a5e88e82e3fdc7cd', got None)` and `$.UpdateRuntimeOn: value_mismatch (expected 'Manual', got 'Auto')`; recursion task#10 `status: expected 409, got 400` with the message value_mismatch quoted above. Smithy: PutRuntimeManagementConfig binds ResourceConflictException (409).

## Steps to Reproduce
Replay the corpus. Fixture lambda-event-invoke-config-recursion-and-runtime-management step task#3: PutRuntimeManagementConfig with UpdateRuntimeOn=Manual and a RuntimeVersionArn, then GetRuntimeManagementConfig — it reports Auto with no ARN. Step task#105: call it while an update is in progress — expect 409, observe 200.
