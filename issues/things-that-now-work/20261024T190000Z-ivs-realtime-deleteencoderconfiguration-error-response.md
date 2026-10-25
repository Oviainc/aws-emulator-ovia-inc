Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 38a5710b90801481080ff726caa37ba94cf84d5f

# DeleteEncoderConfiguration deletes a configuration a running Composition still uses instead of answering ConflictException

_Filed: 2026-10-24T19:00:00Z_
_Operation: DeleteEncoderConfiguration | Class: error-response_

> _Note: this issue was identified against the code base at commit `38a5710b90801481080ff726caa37ba94cf84d5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation's own model documentation is "Deletes an EncoderConfiguration resource. Ensures that no Compositions are using this template; otherwise, returns an error", and the operation declares ConflictException (HTTP 409). The emulator never performs that check: the handler removes the record from `store.encoder_configurations` as soon as the ARN is found, with no look at `store.compositions`.

Expected: when any composition held by the store references the encoder configuration ARN — through `destinations[].channel.encoderConfigurationArn` or `destinations[].s3.encoderConfigurationArns` — DeleteEncoderConfiguration answers HTTP 409 ConflictException and the configuration stays readable through GetEncoderConfiguration.

Actual: HTTP 200 with an empty body; the configuration is gone while the composition that was started against it is still ACTIVE/STARTING, so a later GetComposition reports a composition whose encoder configuration no longer exists and GetEncoderConfiguration answers 404.

The in-code comment at the deletion site justifies the omission with "a Composition is created only by StartComposition, still a scaffold with its own gap issue, so this store holds no composition" — that precondition no longer holds: StartComposition is implemented, validates the referenced encoder configuration ARNs against the store and persists them on CompositionRecord.destinations, so the records that make the refusal true now exist.

## Evidence

Spec: `smithy_query(query="doc", shape="DeleteEncoderConfiguration")` -> "Ensures that no Compositions are using this template; otherwise, returns an error"; the operation models ConflictException (HTTP 409) (OPERATIONS.md, DeleteEncoderConfiguration errors).

Handler: services/.../src/ops/delete_encoder_configuration.rs lines 148-165 — the only branch is `store.encoder_configurations.remove(&input.arn).is_none()` -> ResourceNotFoundException; no composition lookup. Its comment explicitly defers the conflict check until compositions exist, while src/ops/start_composition.rs lines 327-357 already resolves and stores encoder-configuration ARNs on every destination (crate::state::CompositionRecord.destinations).

Tests: `grep -n 'DeleteEncoderConfiguration|ConflictException' tests/` shows tests/all/encoder_configurations.rs asserting only 200 on delete, 404 for an unknown ARN and 400 for a malformed ARN; no case starts a composition and then deletes its encoder configuration, so no test asserts the current (or the correct) behavior.

## Steps to Reproduce

1. CreateEncoderConfiguration -> note `encoderConfiguration.arn`
2. CreateStage -> note `stage.arn`
3. StartComposition with that stageArn and destinations `[{ "channel": { "channelArn": "arn:aws:ivs:us-east-1:123456789012:channel/live", "encoderConfigurationArn": <encoder ARN> } }]` -> 200, composition STARTING
4. DeleteEncoderConfiguration with `{ "arn": <encoder ARN> }` while that composition is still running
5. Observe HTTP 200 and an empty body (expected HTTP 409 ConflictException because a composition is using the template)
6. GetEncoderConfiguration on the same ARN -> 404, while GetComposition still reports the composition that references it
