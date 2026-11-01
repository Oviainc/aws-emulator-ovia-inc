Issue-Source: trace-sweep-divergence
Issue-Trace: lam-err-create-function-service-limits
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lam-err-create-function-service-limits lam-err-update-function-configuration-service-limits
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateFunction|4xx->2xx|$.message
Issue-Key: lambda|CreateFunction|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|CreateFunction|4xx->2xx|status
Issue-Key: lambda|CreateFunction|4xx->4xx|$.message
Issue-Key: lambda|CreateFunction|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.Description
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.MemorySize
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.RuntimeVersionConfig.RuntimeVersionArn
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.Timeout
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->2xx|$.message
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->2xx|status
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->4xx|$.message
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|UpdateFunctionConfiguration|4xx->4xx|status
Issue-Key: lambda|GetFunctionConfiguration|2xx->2xx|$.LastUpdateStatus
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Architectures
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.CodeSha256
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.CodeSize
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Description
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.EphemeralStorage
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.FunctionArn
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.FunctionName
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Handler
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.LastModified
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.LastUpdateStatus
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.LastUpdateStatusReason
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.LastUpdateStatusReasonCode
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.LoggingConfig
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.MemorySize
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.PackageType
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.RevisionId
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Role
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Runtime
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.RuntimeVersionConfig
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.SnapStart
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.State
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Timeout
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.TracingConfig
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|$.Version
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->4xx|status
Issue-Related: 20261029T233239Z-lambda-createfunction-refuses-a-request-aws-accepts-breaking.md
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda does not enforce the Timeout and MemorySize account limits on create or update

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Shared fault: CreateFunction/UpdateFunctionConfiguration accept Timeout and MemorySize values beyond the enforced service limits, so the over-limit value is persisted and every later read and mutation of that function diverges. Ten net-new clusters.

AWS: Smithy models Timeout @range 1..5400 and MemorySize @range 128..32768, but the recorded service additionally enforces the account limits and refuses with InvalidParameterValueException (httpError 400, {Type, message}): for Timeout, message `Value '<value>' at 'timeout' failed to satisfy constraint: Member must have value less than or equal to 900` — fixed text with the rejected value, the member path `timeout` and the limit 900 as variables, and NO `N validation error detected: ` prefix; for MemorySize, message `'MemorySize' value failed to satisfy constraint: Member must have value less than or equal to 10240` — fixed text with the member name and the 10240 limit. Because the over-limit requests are refused, the function keeps its previous configuration and the positive-control create then succeeds 201 and reads back Timeout 900 / MemorySize 3008.

Emulator: MemorySize over the limit is accepted (201/200) and persisted; the Timeout refusal exists but prepends `1 validation error detected: ` to AWS's text; and because the rejected mutations were applied, the follow-on steps diverge — the control create collides 409, the function reads back the over-limit values, and UpdateFunctionConfiguration reports 409 `An update is in progress for resource: <arn>` where AWS answered the request.

Why: a client that depends on Lambda refusing an impossible configuration gets a function it cannot actually run, and a client matching AWS's refusal text does not match because of the inserted prefix; the stale `InProgress` LastUpdateStatus then blocks subsequent updates.

Fix: CreateFunction's and UpdateFunctionConfiguration's validation must enforce the Timeout<=900 and MemorySize<=10240 account limits before mutating state, and must render AWS's message text without the `N validation error detected: ` prefix for these two checks.

Cause 1 of 10: CreateFunction — fixture lam-err-create-function-service-limits step task#2; MemorySize 20480 expected 400 with the MemorySize constraint message, emulator answered 201 and created the function.
Cause 2 of 10: CreateFunction, 400 — fixture lam-err-create-function-service-limits step task#1; Timeout 5400 is refused but the message carries the extra `1 validation error detected: ` prefix.
Cause 3 of 10: CreateFunction — fixture lam-err-create-function-service-limits step task#4; the positive control expected 201 with the full FunctionConfiguration, emulator answered 409 because cause 1 already created the function.
Cause 4 of 10: GetFunctionConfiguration, 200 — fixture lam-err-create-function-service-limits step task#101; reads back Description ``, MemorySize 20480 and Timeout 3 instead of the control's description, 3008 and 900.
Cause 5 of 10: GetFunctionConfiguration, 200 — fixture lam-err-create-function-service-limits step task#5; same three members diverge.
Cause 6 of 10: UpdateFunctionConfiguration — fixture lam-err-update-function-configuration-service-limits step task#4; Timeout 5400 expected 400, emulator answered 200 and applied it.
Cause 7 of 10: UpdateFunctionConfiguration — fixture lam-err-update-function-configuration-service-limits step task#5; MemorySize over limit expected 400 with the MemorySize message, emulator answered 409 `The operation cannot be performed at this time. An update is in progress for resource: <arn>`.
Cause 8 of 10: GetFunctionConfiguration, 200 — fixture lam-err-update-function-configuration-service-limits step task#6; `LastUpdateStatus` stuck at `InProgress` instead of `Successful` and Timeout 5400 instead of 3.
Cause 9 of 10: UpdateFunctionConfiguration — fixture lam-err-update-function-configuration-service-limits step task#7; expected 200 with the updated FunctionConfiguration, emulator answered 409 (consequence of the stuck in-progress state).
Cause 10 of 10: GetFunctionConfiguration, 200 — fixture lam-err-update-function-configuration-service-limits step task#8; MemorySize 128 and Timeout 5400 instead of 3008 and 900.

## Evidence
Cluster details: create task#2 `status: expected 400, got 201` with `$.message: missing (expected "'MemorySize' value failed to satisfy constraint: Member must have value less than or equal to 10240")`; create task#1 `$.message: value_mismatch (expected "Value '5400' at 'timeout' failed to satisfy constraint: Member must have value less than or equal to 900", got "1 validation error detected: Value '5400' ...")`; create task#4 `status: expected 201, got 409`; update task#4 `status: expected 400, got 200`; update task#5 `status: expected 400, got 409`; update task#6 `$.LastUpdateStatus: value_mismatch (expected 'Successful', got 'InProgress')`; update task#7 `status: expected 200, got 409`; the Get clusters show the persisted over-limit MemorySize/Timeout. Smithy: Timeout @range 1..5400, MemorySize @range 128..32768, InvalidParameterValueException httpError 400 — the 900/10240 ceilings are the recorded service limits.

## Steps to Reproduce
Replay the corpus. Fixture lam-err-create-function-service-limits step task#2: CreateFunction with MemorySize 20480 — expect 400, observe 201. Step task#1: CreateFunction with Timeout 5400 and compare the message against AWS's (no `1 validation error detected: ` prefix).
