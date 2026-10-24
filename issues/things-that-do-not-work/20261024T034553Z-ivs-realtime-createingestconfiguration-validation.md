Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ba63228bbc1032132c1fbdbb0b54a8c844ce19f7

# CreateIngestConfiguration accepts a stageArn that names no existing stage

_Filed: 2026-10-24T03:45:53Z_
_Operation: CreateIngestConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `ba63228bbc1032132c1fbdbb0b54a8c844ce19f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a `CreateIngestConfiguration` request whose `stageArn` is well formed but names a stage the account does not hold is refused — the operation models `ValidationException` (HTTP 400) and no other error that fits a bad reference (it declares no `ResourceNotFoundException`), and IVS RealTime does not let a caller create an IngestConfiguration whose association points at a stage that does not exist: the association is what a publisher's RTMP(S) ingest is routed into, so a configuration bound to a non-existent stage is unusable.

Actual: the handler never consults `store.stages`. Any string the modelled `IngestConfigurationStageArn` pattern admits is persisted verbatim into `IngestConfigurationRecord.stage_arn`, the call answers HTTP 200, and `GetIngestConfiguration` then reports an association to a stage that `GetStage` answers 404 for. Because `UpdateIngestConfiguration` is still a scaffold, the dangling association can never be corrected by a client either.

Note this is distinct from the already-filed DeleteStage issue (`20261024T023807Z-ivs-realtime-deletestage-wrong-behavior`), which is about an association that goes stale *after* creation; this one is about an association that was never valid at creation time.

## Evidence

Spec: `smithy_query(query="operation", operation="CreateIngestConfiguration")` declares errors AccessDeniedException(403), PendingVerification(403), ServiceQuotaExceededException(402), ValidationException(400) — ValidationException is the only modelled vehicle for a bad input reference. `smithy_query(query="shape", shape="IngestConfiguration")` shows `stageArn` is `@required` with pattern `^^$|^arn:aws:ivs:[a-z0-9-]+:[0-9]+:stage/[a-zA-Z0-9-]+$$`, i.e. the empty string is the modelled "no stage" value and anything else is supposed to denote a real stage; the doc on DeleteStage ("also removes the stageArn from the associated IngestConfiguration") shows AWS keeps the association consistent with the stage's existence.

Handler: services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/src/ops/create_ingest_configuration.rs, `handle()` — after the attributes-size and insecureIngest/RTMP checks it takes the store lock, checks only the store bound, mints ids and then stores `stage_arn: input.stage_arn.unwrap_or_else(|| crate::state::NO_ASSOCIATED_STAGE_ARN.to_string())`. The inline comment states the choice explicitly: "this operation models no `ResourceNotFoundException`, so a stage ARN this store does not hold is persisted rather than refused" — i.e. no existence check is performed at all, not even with the ValidationException the operation does model. Contrast `CreateParticipantToken` (src/ops/create_participant_token.rs), which does check `store.stages.contains_key(&input.stage_arn)`.

Tests: tests/all/ingest_configurations.rs `create_cases()` covers only an absent stageArn, an explicitly empty stageArn, and an ARN of a stage that was really created (`a_created_ingest_configuration_is_read_back_exactly`, which creates the stage first on purpose). No test sends a well-formed-but-absent stage ARN, so neither the current acceptance nor a refusal is asserted anywhere.

## Steps to Reproduce

1. POST /CreateStage with {} and delete it, or simply construct a well-formed stage ARN that was never created, e.g. arn:aws:ivs:us-east-1:<account>:stage/neverCreated1 (signing the request for us-east-1 with the ivs signing name).
2. POST /CreateIngestConfiguration with {"ingestProtocol":"RTMPS","insecureIngest":false,"redundantIngest":false,"stageArn":"arn:aws:ivs:us-east-1:<account>:stage/neverCreated1"}.
3. Observe HTTP 200 and an ingestConfiguration whose stageArn is the non-existent stage; AWS refuses the call with ValidationException (400) and creates nothing.
4. POST /GetIngestConfiguration with the returned arn — the emulator keeps reporting the dangling stageArn, while POST /GetStage with that same stage ARN answers 404 ResourceNotFoundException.
