Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# Modeled @httpHeader response members (CORS/security headers) are never emitted on any response

_Filed: 2026-10-24T13:55:13Z_
_Operation: CreateIngestConfiguration | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateIngestConfigurationResponse` binds seven members to response HEADERS via `@httpHeader`: `accessControlAllowOrigin` (Access-Control-Allow-Origin), `accessControlExposeHeaders` (Access-Control-Expose-Headers), `cacheControl` (Cache-Control), `contentSecurityPolicy` (Content-Security-Policy), `strictTransportSecurity` (Strict-Transport-Security), `xContentTypeOptions` (X-Content-Type-Options) and `xFrameOptions` (X-Frame-Options). These members are part of the operation's modeled output surface — AWS binds them because the real endpoint returns those headers on the response.

Expected: a successful `CreateIngestConfiguration` answers 200 with the `ingestConfiguration` body AND the CORS/security headers the model binds on its output shape.

Actual: the handler builds its output as `CreateIngestConfigurationOutput { ingest_configuration: Some(..), ..Default::default() }`, so all seven header members stay `None` and the serializer writes none of the headers. The response carries only the framework's own headers (content-type, x-amzn-RequestId) and the JSON body.

The same seam affects the other operations whose output shape carries these members — `StartParticipantReplication` (implemented; its output is NOTHING BUT these seven header members, so its 200 is an entirely empty response with no modeled output realised at all), and the still-stubbed `StopParticipantReplication` / `UpdateIngestConfiguration`. Fixing it in one shared place (the way `crate::state` centralises this service's other cross-operation rules) would cover all four.

## Evidence

Spec: `smithy_query(query="operation", operation="CreateIngestConfiguration")` / OPERATIONS.md line 37 — output members `accessControlAllowOrigin` (string, @httpHeader(Access-Control-Allow-Origin)), `accessControlExposeHeaders`, `cacheControl`, `contentSecurityPolicy`, `strictTransportSecurity`, `xContentTypeOptions`, `xFrameOptions`, plus `ingestConfiguration`. The same seven members appear on `StartParticipantReplicationResponse` (OPERATIONS.md line 273), `StopParticipantReplicationResponse` (line 287) and `UpdateIngestConfigurationResponse` (line 309), and on every modeled error shape in this model.

Generated output type: src/ops/types/create_ingest_configuration_types.rs lines 29-38 declares all seven as `Option<String>`.

Handler: src/ops/create_ingest_configuration.rs lines 340-343 — `Ok(CreateIngestConfigurationOutput { ingest_configuration: Some(ingest_configuration), ..Default::default() })`; nothing in the crate ever assigns any of the seven (grep for `x_frame_options:` / `strict_transport_security:` across the repo finds only type declarations, never an assignment). src/ops/start_participant_replication.rs lines 352-356 states the same decision explicitly: "Every member of `StartParticipantReplicationResponse` is an `@httpHeader` binding for a CORS or security response header ... so each stays absent" and returns `StartParticipantReplicationOutput::default()`.

Test coverage: none — grep over tests/ for `access-control`, `cache-control`, `content-security`, `x-frame` finds no match; tests/all/ingest_configurations.rs and tests/all/error_envelope.rs inspect only the JSON body and the x-amzn-* envelope headers, so no test pins either the presence or the absence of these headers.

## Steps to Reproduce

1. POST /CreateIngestConfiguration {"ingestProtocol":"RTMPS","insecureIngest":false,"redundantIngest":false} → 200 with the ingestConfiguration body
2. Inspect the response headers: expected Access-Control-Allow-Origin, Access-Control-Expose-Headers, Cache-Control, Content-Security-Policy, Strict-Transport-Security, X-Content-Type-Options and X-Frame-Options (the seven members the output shape binds); actual none of them are present
3. POST /CreateStage {} twice → source ARN A, destination ARN B; POST /CreateParticipantToken {"stageArn":"A"} → participantId P
4. POST /StartParticipantReplication {"sourceStageArn":"A","destinationStageArn":"B","participantId":"P"} → 200 whose entire modeled output is those seven headers; actual response carries none of them, so no modeled output member is realised at all
