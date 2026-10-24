Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047af81d647cd24bcec0ef3f34fd68e55311879a

# DeleteStage leaves the deleted stage's ARN on associated IngestConfigurations

_Filed: 2026-10-24T02:38:07Z_
_Operation: DeleteStage | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047af81d647cd24bcec0ef3f34fd68e55311879a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's own documentation for DeleteStage states: "Shuts down and deletes the specified stage (disconnecting all participants). This operation also removes the stageArn from the associated IngestConfiguration, if there are participants using the IngestConfiguration to publish to the stage."

Expected: after a stage is deleted, an IngestConfiguration that was created with that stage's ARN no longer reports it — `IngestConfiguration$stageArn` is reset to the "no associated stage" value (the empty string, which is the first alternative of the member's own pattern `^^$|^arn:aws:ivs:...:stage/...$$` and the value this emulator already uses as `crate::state::NO_ASSOCIATED_STAGE_ARN` for a configuration created without a stage).

Actual: `DeleteStage` removes only the `stages` entry and the stage's `participant_tokens` entry. Nothing touches `store.ingest_configurations`, so `GetIngestConfiguration` keeps reporting `stageArn` = the ARN of a stage that no longer exists (and `GetStage` on that same ARN answers 404). The emulator thus hands a client a dangling association that real IVS RealTime never reports, and because the only other writer of that field (`UpdateIngestConfiguration`) is still a scaffold, a client has no way to clear it.

## Evidence

Spec: smithy_query(query="doc", shape="DeleteStage") returns the sentence "This operation also removes the stageArn from the associated IngestConfiguration ..."; smithy_query(query="shape", shape="IngestConfiguration") shows `stageArn` is @required with pattern `^^$|^arn:aws:ivs:[a-z0-9-]+:[0-9]+:stage/[a-zA-Z0-9-]+$$` (the empty string is the modeled "no stage" value, as src/state.rs NO_ASSOCIATED_STAGE_ARN documents).
Handler: services/.../src/ops/delete_stage.rs, handle() lines 142-161 — `store.stages.remove(&input.arn)` then `store.participant_tokens.remove(&input.arn)` and nothing else; the ingest-configuration map is never consulted. src/ops/create_ingest_configuration.rs persists the requested `stage_arn` verbatim into `IngestConfigurationRecord.stage_arn`, and src/state.rs `IngestConfigurationRecord::to_ingest_configuration` reports it unchanged.
Tests: no test covers this — grep for `DeleteStage` across services/.../tests shows no assertion about ingest configurations (tests/all/ingest_configurations.rs never calls DeleteStage), and tests/all/stages.rs delete_stage_removes_the_stage_and_its_tokens asserts only the stage and its tokens are gone.

## Steps to Reproduce

1. POST /CreateStage with {} and note the returned stage ARN S.
2. POST /CreateIngestConfiguration with {"ingestProtocol":"RTMPS","insecureIngest":false,"redundantIngest":false,"stageArn":S} and note the returned ingest-configuration ARN I.
3. POST /DeleteStage with {"arn":S} — 200.
4. POST /GetStage with {"arn":S} — 404 ResourceNotFoundException, as expected.
5. POST /GetIngestConfiguration with {"arn":I}: the emulator still reports "stageArn": S, an ARN that names no stage; AWS reports the association removed (empty stageArn).
