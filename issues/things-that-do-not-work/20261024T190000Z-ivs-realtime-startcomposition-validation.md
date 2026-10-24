Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 38a5710b90801481080ff726caa37ba94cf84d5f

# StartComposition accepts two destinations of the same kind (two channel or two s3) instead of ValidationException

_Filed: 2026-10-24T19:00:00Z_
_Operation: StartComposition | Class: validation_

> _Note: this issue was identified against the code base at commit `38a5710b90801481080ff726caa37ba94cf84d5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A composition may carry at most one channel destination and at most one S3 destination. The model states the rule on `Composition$destinations`: "Array of Destination objects. A Composition can contain either one destination (channel or s3) or two (one channel and one s3)." The handler enforces only the per-element rule (exactly one of `channel`/`s3` inside each destination) and the modelled list bounds (`@length(1..2)`, `@uniqueItems`), so two DISTINCT destinations of the same kind pass.

Expected: a request whose two destinations are both `channel` (e.g. two different channelArns) or both `s3` (e.g. two different storageConfigurationArns) is refused with HTTP 400 ValidationException and starts no composition.

Actual: HTTP 200 — a composition is minted and persisted with two same-kind destinations, each getting its own destination id and (for s3) its own recording prefix, and GetComposition reports a resource shape the service documents as impossible.

## Evidence

Spec: `smithy_query(query="shape", shape="Composition")` -> `destinations` doc "A Composition can contain either one destination (channel or s3) or two (one channel and one s3)"; `@uniqueItems` only rejects identical elements, not two distinct same-kind ones.

Handler: services/.../src/ops/start_composition.rs lines 213-232 loops over `input.destinations` and refuses a destination that names neither or both of `channel`/`s3`, but there is no cross-element check; lines 382-406 then mint one DestinationRecord per element unconditionally.

Tests: tests/all/compositions.rs covers one channel, one s3, "both destinations the model allows at once" (one channel + one s3, line ~389), three destinations (400 by list length, line ~856) and a destination naming both/neither target (400). No case sends two channel destinations or two s3 destinations, so the gap is untested in either direction.

## Steps to Reproduce

1. CreateStage -> note `stage.arn`
2. StartComposition with `{ "stageArn": <stage ARN>, "destinations": [{ "channel": { "channelArn": "arn:aws:ivs:us-east-1:123456789012:channel/a" } }, { "channel": { "channelArn": "arn:aws:ivs:us-east-1:123456789012:channel/b" } }] }`
3. Observe HTTP 200 and a composition carrying two channel destinations (expected HTTP 400 ValidationException)
4. Repeat with two `s3` destinations naming two different storage configurations (each created first) -> also HTTP 200
5. GetComposition on the returned ARN reports both same-kind destinations
