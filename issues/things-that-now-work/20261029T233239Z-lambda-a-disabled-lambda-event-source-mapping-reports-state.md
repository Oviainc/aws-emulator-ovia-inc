Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-esm-scaling-config-roundtrip
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-esm-scaling-config-roundtrip
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateEventSourceMapping|2xx->2xx|$.FunctionResponseTypes
Issue-Key: lambda|CreateEventSourceMapping|2xx->2xx|$.State
Issue-Key: lambda|GetEventSourceMapping|2xx->2xx|$.FunctionResponseTypes
Issue-Key: lambda|GetEventSourceMapping|2xx->2xx|$.State
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# A disabled Lambda event source mapping reports State Creating and omits FunctionResponseTypes

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Shared fault: the event-source-mapping serializer reports the wrong lifecycle state for a mapping created with Enabled=false and omits the empty FunctionResponseTypes list. Two net-new clusters.

AWS: Smithy EventSourceMappingConfiguration (the CreateEventSourceMapping/GetEventSourceMapping output) declares `State` (String) and `FunctionResponseTypes` (FunctionResponseTypeList). Recorded behavior: a mapping created with Enabled=false settles directly into `State: "Disabled"` with `StateTransitionReason: "USER_INITIATED"`, and `FunctionResponseTypes` is rendered as the empty list when none were requested — on both the create response and the subsequent read.

Emulator: both responses report `State: "Creating"` and omit `FunctionResponseTypes` entirely.

Why: a caller that waits for a mapping to leave a transitional state before proceeding waits forever on a mapping that AWS already considers Disabled, and a caller iterating `FunctionResponseTypes` gets a missing key instead of an empty list.

Fix: the event-source-mapping state machine must resolve Enabled=false to the Disabled state (and not leave it Creating) and the serializer must render FunctionResponseTypes as an empty list.

Cause 1 of 2: CreateEventSourceMapping, 202 — fixture lambda-esm-scaling-config-roundtrip step task#7; `State` `Creating` instead of `Disabled` and `FunctionResponseTypes` omitted.
Cause 2 of 2: GetEventSourceMapping, 200 — fixture lambda-esm-scaling-config-roundtrip step task#8; the same two defects on read-back, so the state never converges.

## Evidence
Both clusters' detail: `$.State: value_mismatch (expected 'Disabled', got 'Creating')` and `$.FunctionResponseTypes: missing (expected [], got None)`; expected status == actual status (202 and 200). Smithy: EventSourceMappingConfiguration declares State and FunctionResponseTypes.

## Steps to Reproduce
Replay the corpus. Fixture lambda-esm-scaling-config-roundtrip step task#7: CreateEventSourceMapping with Enabled=false and a ScalingConfig, then step task#8: GetEventSourceMapping — both report State `Creating`.
