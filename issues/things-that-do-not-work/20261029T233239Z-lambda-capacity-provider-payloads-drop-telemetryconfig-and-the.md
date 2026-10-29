Issue-Source: trace-sweep-divergence
Issue-Trace: lam-err-capacity-provider-duplicate-create
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lam-err-capacity-provider-duplicate-create lambda-capacity-provider-scaling-value-precision-round-trip
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateCapacityProvider|2xx->2xx|$.CapacityProvider.TelemetryConfig
Issue-Key: lambda|GetCapacityProvider|2xx->2xx|$.CapacityProvider.TelemetryConfig
Issue-Key: lambda|CreateCapacityProvider|4xx->4xx|$.Type
Issue-Related: 20261029T233239Z-lambda-capacity-provider-operations-accept-nonexistent-providers-and.md
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda capacity-provider payloads drop TelemetryConfig and the duplicate-create refusal is reworded

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityProvider

## Description
Shared fault: the CapacityProvider serializer drops the TelemetryConfig member it was created with, and the duplicate-name refusal does not use AWS's wording. Three net-new clusters.

AWS: Smithy CapacityProvider declares `TelemetryConfig` (target CapacityProviderTelemetryConfig, "The telemetry configuration for the capacity provider, including logging settings"), so a provider created with a LoggingConfig echoes it back on CreateCapacityProvider and GetCapacityProvider. A second CreateCapacityProvider for a name already in the account is refused 409 ResourceConflictException (Smithy: httpError 409, members {Type, message}) with message `A capacity provider with ARN <capacityProviderArn> already exists. Capacity provider names must be unique within your account.` — both sentences are fixed text; `<capacityProviderArn>` is the ARN of the existing provider from state.

Emulator: `CapacityProvider.TelemetryConfig` is absent from both responses even though the create request carried a LoggingConfig, and the 409 message is the emulator's own phrasing `Capacity provider already exists: <arn>` with no `Type` member.

Why: a client that reads back its telemetry/logging settings sees them unset, so a configured log group looks lost; a client matching on AWS's duplicate-name text does not match.

Fix: the capacity-provider serializer must render TelemetryConfig from state; CreateCapacityProvider's conflict path must use AWS's message template and the error serializer must emit `Type`.

Cause 1 of 3: CreateCapacityProvider, 202 — fixture lam-err-capacity-provider-duplicate-create step task#6; response omits `CapacityProvider.TelemetryConfig` (the recording returns the LoggingConfig with its LogGroup and SystemLogLevel as supplied).
Cause 2 of 3: GetCapacityProvider, 200 — fixture lam-err-capacity-provider-duplicate-create step task#7; same member omitted on read-back.
Cause 3 of 3: CreateCapacityProvider, 409 — fixture lam-err-capacity-provider-duplicate-create step task#8; message reworded and `Type: "User"` missing.

## Evidence
Cluster details: task#6 and task#7 `$.CapacityProvider.TelemetryConfig: missing (expected {'LoggingConfig': {'LogGroup': ..., 'SystemLogLevel': 'INFO'}}, got None)`; task#8 `$.Type: missing (expected 'User', got None)` and `$.message: value_mismatch (expected 'A capacity provider with ARN <arn> already exists. Capacity provider names must be unique within your account.', got 'Capacity provider already exists: <arn>')`. Smithy: CapacityProvider.TelemetryConfig; ResourceConflictException httpError 409 with Type/message.

## Steps to Reproduce
Replay the corpus. Fixture lam-err-capacity-provider-duplicate-create step task#6: CreateCapacityProvider with a TelemetryConfig.LoggingConfig and read the 202 body; step task#8: repeat the create with the same name and read the 409 body.
