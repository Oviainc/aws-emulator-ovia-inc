Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: qconnect.ai-agent-visibility-status-transition
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: qconnect.ai-agent-visibility-status-transition|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Commit: 15282e7a5cd21fb97893a5ceda853b3e1ef07f09

# qconnect UpdateAIAgent answers 501 NotImplemented, so an AI Agent's visibilityStatus never transitions SAVED -> PUBLISHED (1 cause)

> _Note: this issue was identified against the code base at commit `15282e7a5cd21fb97893a5ceda853b3e1ef07f09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAIAgent

## Description
The Amazon Q in Connect emulator (wire service `wisdom`, model com.amazonaws.qconnect) cannot publish an AI Agent: the only operation that writes an agent's visibilityStatus answers 501, so an agent created SAVED stays SAVED forever and a client that polls for the transition never converges. ONE cause.

Cause 1 of 1: UpdateAIAgent is a 501 scaffold, so the SAVED -> PUBLISHED visibility transition is unreachable and never becomes durably observable (failing check qconnect.ai-agent-visibility-status-transition).

AWS: Smithy (services/service-com-amazonaws-qconnect-wisdomservice/smithy-model/com-amazonaws-qconnect-wisdomservice.json) models UpdateAIAgent as POST /assistants/{assistantId}/aiagents/{aiAgentId}, @idempotent, success 200, input `assistantId` (UuidOrArn, @httpLabel, required), `aiAgentId` (UuidOrArnOrEitherWithQualifier, @httpLabel, required), `visibilityStatus` (VisibilityStatus, one of PUBLISHED | SAVED, required), optional `configuration` (AIAgentConfiguration), optional `description` (Description, @length 1..255, pattern ^[a-zA-Z0-9\s_.,-]+) and optional `clientToken` (@idempotencyToken, @length 1..4096); output `aiAgent` -> AIAgentData — the same shape CreateAIAgent and GetAIAgent (@readonly, GET on the same URI, output aiAgent + versionNumber) return, and AIAgentData declares `visibilityStatus` required. Its only modeled refusals are AccessDeniedException (403), ConflictException (409), ResourceNotFoundException (404), ThrottlingException (400), UnauthorizedException (401) and ValidationException (400); NotImplemented is not among them and 501 is not a modeled status for this operation. So on AWS, UpdateAIAgent on an existing agent of an existing assistant with visibilityStatus=PUBLISHED answers 200 with `aiAgent.visibilityStatus` = 'PUBLISHED' (the value from the request, not a literal of any one run), and the following GetAIAgent on that same assistantId/aiAgentId reports that same value. Only a request that names an absent assistant or agent is the modeled 404 ResourceNotFoundException.

Emulator: UpdateAIAgent never reaches any agent state. The handler is still the generated scaffold and its last statement is `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateAIAgent"))`, so every call — including one naming a just-created, existing agent under a just-created ACTIVE assistant — is refused with HTTP 501, error code `NotImplemented` and message "operation UpdateAIAgent is scaffolded but not implemented in this emulator" (that exact text is observable on the sibling UpdateAIAgent call in this same suite). Because the refusal is permanent rather than transient, the check's retry loop over UpdateAIAgent never produces a response and the check is cut off by the harness after 60.0s with verdict `mismatch` / "Check timed out after 60.0s"; the agent is left in its create-time visibilityStatus 'SAVED'.

Why: a client that creates an agent SAVED and then publishes it gets neither the published agent nor any modeled error it can interpret. 501/NotImplemented is not in the operation's modeled error set, so an SDK caller's error handling for AccessDenied/Conflict/ResourceNotFound/Validation does not match it; retry-until-success publish loops (the normal shape for a control-plane transition) spin until their own deadline instead of failing fast, and any workflow that wires an assistant to a PUBLISHED agent, or gates traffic on an agent being published, cannot proceed at all. Nothing in the emulator can move an AI Agent out of SAVED.

Fix: the UpdateAIAgent handler and the AI-Agent state it must write in the qconnect service crate — resolve assistantId/aiAgentId against the stored agent map (modeled 404 ResourceNotFoundException when either is absent), write the request's `visibilityStatus` (and `description`/`configuration` when supplied) onto the stored agent record, and answer 200 with the updated AIAgentData; drop the scaffold 501. The read half must then serve the stored values — that read-side projection is a separate, still-open defect: 20261031T001959Z-wisdom-qconnect-getaiagent-read-path-ignores-ai-agent-lifecycle.md (GetAIAgent serves the create-time snapshot), which does NOT cover this write-side 501, and the auto-filed UpdateAIAgent scaffold-stub work item names only the stub, not this SAVED -> PUBLISHED transition requirement or the fact that the refusal is unmodeled for this operation. Both halves must be closed before the transition is durably observable.

Traces: qconnect.ai-agent-visibility-status-transition (the publish poll never converges; harness timeout at 60.0s). Code: services/service-com-amazonaws-qconnect-wisdomservice/src/ops/update_ai_agent.rs:180 (Scaffolded::not_implemented) and the SCAFFOLDED marker at line 4.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator (bootstrap_signal false; qconnect answered 11/25 checks, so the service is serving).

Failing check qconnect.ai-agent-visibility-status-transition — verdict [mismatch], detail: "Check timed out after 60.0s".

What the check does and what it observed:
  1. CreateAssistant(type=AGENT) -> polled to ACTIVE: succeeds.
  2. CreateAIAgent(assistantId, type=MANUAL_SEARCH, configuration={manualSearchAIAgentConfiguration:{}}, visibilityStatus='SAVED') -> 200, aiAgent.visibilityStatus == 'SAVED' (assertion passes).
  3. UpdateAIAgent(assistantId, aiAgentId, visibilityStatus='PUBLISHED') in a retry loop (interval 8s, the check's own budget 220s): every attempt is refused, so the loop's probe returns None on every attempt and the harness cuts the check off at 60.0s. Expected: HTTP 200 with aiAgent.visibilityStatus == 'PUBLISHED'.
  4. Never reached: GetAIAgent must durably report visibilityStatus == 'PUBLISHED'.

The refusal UpdateAIAgent serves is visible verbatim on the sibling UpdateAIAgent call in this same run (check qconnect.ai-agent-version-immutable-after-update):
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateAIAgent operation: operation UpdateAIAgent is scaffolded but not implemented in this emulator
HTTP status 501, error code 'NotImplemented' — a status/code pair UpdateAIAgent's model does not declare (its modeled errors are 403/409/404/400/401/400 only).

Model checked: services/service-com-amazonaws-qconnect-wisdomservice/smithy-model/com-amazonaws-qconnect-wisdomservice.json — UpdateAIAgent (POST /assistants/{assistantId}/aiagents/{aiAgentId}, 200, required visibilityStatus of enum VisibilityStatus {PUBLISHED, SAVED}, output aiAgent -> AIAgentData), GetAIAgent (@readonly, GET same URI, 200), AIAgentData (visibilityStatus required).
Code checked: services/service-com-amazonaws-qconnect-wisdomservice/src/ops/update_ai_agent.rs — handle() binds its inputs and returns Scaffolded::not_implemented("UpdateAIAgent") at line 180; the file still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker at line 4.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral check qconnect.ai-agent-visibility-status-transition (suite simparity-behavioral-aws-v1), or reproduce by hand against the emulator endpoint with an AWS SDK:
2. qconnect.create_assistant(name=<unique>, type='AGENT'); poll get_assistant until assistant.status == 'ACTIVE'.
3. qconnect.create_ai_agent(assistantId=<id>, name=<unique>, type='MANUAL_SEARCH', configuration={'manualSearchAIAgentConfiguration': {}}, visibilityStatus='SAVED') and note aiAgent.aiAgentId; the response reports visibilityStatus 'SAVED' as expected.
4. qconnect.update_ai_agent(assistantId=<id>, aiAgentId=<agent id>, visibilityStatus='PUBLISHED'). Observe HTTP 501 with error code 'NotImplemented' and message 'operation UpdateAIAgent is scaffolded but not implemented in this emulator', instead of HTTP 200 with aiAgent.visibilityStatus == 'PUBLISHED'. Repeating the call never succeeds, so any poll-until-published loop hangs to its deadline (the harness cuts the check at 60.0s).
5. qconnect.get_ai_agent(assistantId=<id>, aiAgentId=<agent id>) still reports visibilityStatus 'SAVED': the transition is unreachable.
