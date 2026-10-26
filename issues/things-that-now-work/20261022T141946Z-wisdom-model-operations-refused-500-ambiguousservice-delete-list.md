Issue-Source: trace-sweep-divergence
Issue-Trace: wisdom-assistant-creation-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wisdom-assistant-creation-errors wisdom-knowledge-base-lifecycle wisdom-knowledge-base-validation-errors wisdom-quick-response-error-paths wisdom-session-error-paths
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom|CreateAssistant|4xx->5xx|$.message
Issue-Key: wisdom|CreateAssistant|4xx->5xx|$headers.content-type
Issue-Key: wisdom|CreateAssistant|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: wisdom|CreateAssistant|4xx->5xx|status
Issue-Key: wisdom|DeleteAssistant|4xx->5xx|$.message
Issue-Key: wisdom|DeleteAssistant|4xx->5xx|$headers.content-type
Issue-Key: wisdom|DeleteAssistant|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: wisdom|DeleteAssistant|4xx->5xx|status
Issue-Key: wisdom|ListKnowledgeBases|2xx->5xx|$.knowledgeBaseSummaries
Issue-Key: wisdom|ListKnowledgeBases|2xx->5xx|$headers.content-type
Issue-Key: wisdom|ListKnowledgeBases|2xx->5xx|status
Issue-Key: wisdom|DeleteKnowledgeBase|4xx->5xx|$.message
Issue-Key: wisdom|DeleteKnowledgeBase|4xx->5xx|$headers.content-type
Issue-Key: wisdom|DeleteKnowledgeBase|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: wisdom|DeleteKnowledgeBase|4xx->5xx|status
Issue-Key: wisdom|SearchQuickResponses|4xx->5xx|$.message
Issue-Key: wisdom|SearchQuickResponses|4xx->5xx|$headers.content-type
Issue-Key: wisdom|SearchQuickResponses|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: wisdom|SearchQuickResponses|4xx->5xx|status
Issue-Key: wisdom|QueryAssistant|4xx->5xx|$.message
Issue-Key: wisdom|QueryAssistant|4xx->5xx|$headers.content-type
Issue-Key: wisdom|QueryAssistant|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: wisdom|QueryAssistant|4xx->5xx|status
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# Wisdom-model operations refused 500 AmbiguousService: delete, list, search and query paths never dispatched

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssistant

## Description
This bundle carries 6 causes, all on service `wisdom`, all net-new (none stub-blocked, none owned by an open issue in the replay report). Every one is a wire step of the Wisdom-model (sdkId `Wisdom`) fixture family that never reached a handler: the emulator's request router refuses it with an emulator-internal HTTP 500 `AmbiguousService`, serialized in the wrong protocol envelope.

Shared AWS grounding (cited once, applies to all 6 causes). smithy_query service_info / operation on services/service-com-amazonaws-wisdom-wisdomservice/smithy-model/com-amazonaws-wisdom-wisdomservice.json: service shape `WisdomService`, protocol `restJson1`, sigv4Name `wisdom`, arnNamespace `wisdom`. A second crate in the same checkout, services/service-com-amazonaws-qconnect-wisdomservice, binds the same `WisdomService` shape with sdkId `QConnect` and the identical sigv4Name/endpointPrefix/arnNamespace `wisdom` — this is exactly how AWS ships it (Q Connect is served on Wisdom's endpoint prefix and signing name), so a shared signing name is never grounds to refuse service. restJson1 carries no X-Amz-Target, so dispatch is by HTTP method + request URI. Under restJson1 every response, success or error, is `content-type: application/json`, and a modeled error carries `x-amzn-errortype: <ShapeName>`. `AmbiguousService` is not an error shape in either model; it is emulator-internal and appears in no AWS Wisdom/QConnect response.

Shared Emulator behavior (all 6): HTTP 500, `x-amzn-errortype: AmbiguousService`, `content-type: application/x-amz-json-1.1`, and body message "2 emulated services all claim requests for 'wisdom': wisdom, wisdom; the request is rejected rather than served by an arbitrary one" (recorded as an example of the internal text; the requirements asserted here are the statuses, the error codes and the content-type). Both claimants print as the same token `wisdom`, consistent with the router keying on sigv4Name/endpointPrefix/arnNamespace rather than on sdkId or the restJson1 method+URI.

Shared Fix (all 6): the service-registry / request-router dispatch layer, not any Wisdom operation handler. The registry must admit two services sharing endpointPrefix/sigv4Name/arnNamespace and resolve them deterministically — by registration identity (sdkId `Wisdom` vs `QConnect`) and/or by matching the concrete method+URI against each registered service's restJson1 routes — instead of counting claimants and failing closed. Secondarily the router's own error path must serialize in the target service's protocol: a restJson1 service must answer `content-type: application/json`, never `application/x-amz-json-1.1`.

Relation to already-open issues. 20261022T092108Z-wisdom-qconnect-wire-fixtures-all-refused-500-ambiguousservice.md reports the same router collision and owns 9 wire clusters, but it covers only the `qconnect-*` fixtures and the operation/status keys CreateAssistant 2xx, CreateKnowledgeBase 2xx/4xx, ListAssistants 2xx, GetAssistant 404, CreateAIAgent 404, CreateSession 401, GetKnowledgeBase 404. None of the 6 causes below is in its replay pointer — the replay report confirms `owned_by` is empty for each — so replaying that issue exercises none of them. 20261022T031220Z-wisdom-all-wisdom-requests-rejected-with-ambiguousservice-two-emulated.md reports the collision from the BEHAVIORAL corpus and carries no wire fixture/step keys at all. All 6 causes must be re-replayed individually after the fix.

Cause 1 of 6 — CreateAssistant, 400 malformed-request rejection. AWS: Smithy CreateAssistant POST /assistants -> 200, modeled errors AccessDeniedException 403, ConflictException 409, ServiceQuotaExceededException 402, ValidationException 400; `BadRequestException` is not bound to this operation, so this is the restJson1 framework's malformed-body rejection — recorded behavior only: the recording shows HTTP 400 with `x-amzn-errortype: BadRequestException`, `content-type: application/json`, message 'Invalid request body' (example text). Emulator: 500 / AmbiguousService / application/x-amz-json-1.1. Why: a client that sent an unparseable body is told the server failed, so it retries a request that can never succeed instead of surfacing a 4xx to the caller. Traces: wisdom-assistant-creation-errors#task#1.

Cause 2 of 6 — DeleteAssistant, 404 ResourceNotFoundException for an unknown assistant id. AWS: Smithy DeleteAssistant DELETE /assistants/{assistantId} -> 204, models ResourceNotFoundException httpError 404 (with AccessDeniedException 403, ValidationException 400); recorded 404, `x-amzn-errortype: ResourceNotFoundException`, `content-type: application/json`, message 'Assistant does not exist.' (example). Emulator: 500 / AmbiguousService / application/x-amz-json-1.1. Why: delete-if-exists and cleanup paths keyed on ResourceNotFoundException never fire, and a 500 is retried as transient. Traces: wisdom-assistant-creation-errors#task#3.

Cause 3 of 6 — ListKnowledgeBases, 200 paginated envelope. AWS: Smithy ListKnowledgeBases GET /knowledgeBases -> 200 with REQUIRED output member `knowledgeBaseSummaries` (KnowledgeBaseList) plus optional `nextToken`; paginated(items=knowledgeBaseSummaries). Emulator: 500, `$.knowledgeBaseSummaries` missing entirely, content-type application/x-amz-json-1.1. Why: a required response member is absent, so SDK paginators fault instead of returning a populated or empty page, and no client can enumerate knowledge bases. Traces: wisdom-knowledge-base-lifecycle#task#3.

Cause 4 of 6 — DeleteKnowledgeBase, 404 ResourceNotFoundException for an unknown knowledge-base id. AWS: Smithy DeleteKnowledgeBase DELETE /knowledgeBases/{knowledgeBaseId} -> 204, models ResourceNotFoundException 404 (with ConflictException 409, AccessDeniedException 403, ValidationException 400); recorded 404, errortype ResourceNotFoundException, content-type application/json, message 'Knowledge Base does not exist.' (example). Emulator: 500 / AmbiguousService / application/x-amz-json-1.1. Why: as cause 2, on the knowledge-base delete path — not-found is indistinguishable from a server fault. Traces: wisdom-knowledge-base-validation-errors#task#4.

Cause 5 of 6 — SearchQuickResponses, 404 ResourceNotFoundException for an unknown parent knowledge base. AWS: Smithy SearchQuickResponses POST /knowledgeBases/{knowledgeBaseId}/search/quickResponses -> 200 with required `results`; models ResourceNotFoundException 404 (with AccessDeniedException 403, RequestTimeoutException 408, ValidationException 400); recorded 404, errortype ResourceNotFoundException, content-type application/json, message 'Knowledge Base does not exist.' (example). Emulator: 500 / AmbiguousService / application/x-amz-json-1.1. Why: a search against a missing parent resource looks like a server fault, so the client retries rather than reporting the bad id. Traces: wisdom-quick-response-error-paths#task#7.

Cause 6 of 6 — QueryAssistant, 404 ResourceNotFoundException for an unknown assistant id. AWS: Smithy QueryAssistant POST /assistants/{assistantId}/query -> 200 with required `results` (deprecated operation, still modeled and bound); models ResourceNotFoundException 404 (with AccessDeniedException 403, RequestTimeoutException 408, ValidationException 400); recorded 404, errortype ResourceNotFoundException, content-type application/json, message 'Assistant does not exist.' (example). Emulator: 500 / AmbiguousService / application/x-amz-json-1.1. Why: the query path's not-found branch never fires and the 500 is retried to exhaustion. Traces: wisdom-session-error-paths#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, '/__admin/health answered 200 and the :38309 data listener accepts'; bootstrap_signal=false; state_reset_failures=0; fixtures_replayed=14, fixtures_replay_error=0): steps_total=91, steps_passed=2, steps_mismatched=25, steps_blocked=64, score_pct=2.2, divergence_count=15 — all 15 on service `wisdom`, all verdict=mismatch, all actual_status=500, none stub-blocked. 9 of the 15 are owned by 20261022T092108Z-wisdom-qconnect-wire-fixtures-all-refused-500-ambiguousservice.md and are excluded here; the 6 below report owned_by=[] and are net-new.

The 6 bundled clusters, each affected_count=1:
  wisdom-assistant-creation-errors#task#1        CreateAssistant      expected 400 -> got 500; $.message expected 'Invalid request body' got the AmbiguousService text; $headers.x-amzn-errortype expected 'BadRequestException' got 'AmbiguousService'; $headers.content-type expected 'application/json' got 'application/x-amz-json-1.1'. divergence_key service=wisdom|CreateAssistant|status=400->500|keys=$.message,$headers.content-type,$headers.x-amzn-errortype,status
  wisdom-assistant-creation-errors#task#3        DeleteAssistant      expected 404 -> got 500; errortype expected 'ResourceNotFoundException' got 'AmbiguousService'; content-type 'application/json' -> 'application/x-amz-json-1.1'; message example 'Assistant does not exist.'. divergence_key service=wisdom|DeleteAssistant|status=404->500|keys=$.message,$headers.content-type,$headers.x-amzn-errortype,status
  wisdom-knowledge-base-lifecycle#task#3         ListKnowledgeBases   expected 200 -> got 500; $.knowledgeBaseSummaries missing (required member; expected a one-element list of KnowledgeBaseSummary with knowledgeBaseId/knowledgeBaseArn, knowledgeBaseType CUSTOM, status ACTIVE, name, tags); content-type 'application/json' -> 'application/x-amz-json-1.1'. divergence_key service=wisdom|ListKnowledgeBases|status=200->500|keys=$.knowledgeBaseSummaries,$headers.content-type,status
  wisdom-knowledge-base-validation-errors#task#4 DeleteKnowledgeBase  expected 404 -> got 500; errortype expected 'ResourceNotFoundException' got 'AmbiguousService'; content-type 'application/json' -> 'application/x-amz-json-1.1'; message example 'Knowledge Base does not exist.'. divergence_key service=wisdom|DeleteKnowledgeBase|status=404->500|keys=$.message,$headers.content-type,$headers.x-amzn-errortype,status
  wisdom-quick-response-error-paths#task#7       SearchQuickResponses expected 404 -> got 500; errortype expected 'ResourceNotFoundException' got 'AmbiguousService'; content-type 'application/json' -> 'application/x-amz-json-1.1'; message example 'Knowledge Base does not exist.'. divergence_key service=wisdom|SearchQuickResponses|status=404->500|keys=$.message,$headers.content-type,$headers.x-amzn-errortype,status
  wisdom-session-error-paths#task#5              QueryAssistant       expected 404 -> got 500; errortype expected 'ResourceNotFoundException' got 'AmbiguousService'; content-type 'application/json' -> 'application/x-amz-json-1.1'; message example 'Assistant does not exist.'. divergence_key service=wisdom|QueryAssistant|status=404->500|keys=$.message,$headers.content-type,$headers.x-amzn-errortype,status

All six carry the identical body message "2 emulated services all claim requests for 'wisdom': wisdom, wisdom; the request is rejected rather than served by an arbitrary one", which is emulator-internal text and is quoted only as an example. The requirements asserted are the HTTP statuses (200/400/404), the error codes (`BadRequestException`, `ResourceNotFoundException`), the `application/json` content-type required by restJson1, and the presence of the required output member `knowledgeBaseSummaries`.

Both claimants are bootstrapped in this checkout (resolved bootstrap issues exist for service-com-amazonaws-wisdom-wisdomservice and service-com-amazonaws-qconnect-wisdomservice), so this is a fidelity defect on a stood-up service, not a bootstrap gap.

## Steps to Reproduce
1. Clone mainline and build the emulator; confirm both service crates services/service-com-amazonaws-wisdom-wisdomservice and services/service-com-amazonaws-qconnect-wisdomservice are registered (both bind service shape WisdomService with sigv4Name/endpointPrefix/arnNamespace `wisdom`, protocol restJson1).
2. Replay the simparity-wire-aws-v1 wire corpus against it.
3. Representative step: fixture wisdom-assistant-creation-errors, step task#1 — POST /assistants with a malformed restJson1 CreateAssistant body signed for signing name `wisdom`. Expected HTTP 400, content-type application/json, x-amzn-errortype BadRequestException. Observed HTTP 500, content-type application/x-amz-json-1.1, x-amzn-errortype AmbiguousService.
4. Re-replay each of the other 5 bundled steps to confirm each expected wire outcome independently: wisdom-assistant-creation-errors#task#3 (DeleteAssistant 404 ResourceNotFoundException), wisdom-knowledge-base-lifecycle#task#3 (ListKnowledgeBases 200 with required knowledgeBaseSummaries), wisdom-knowledge-base-validation-errors#task#4 (DeleteKnowledgeBase 404 ResourceNotFoundException), wisdom-quick-response-error-paths#task#7 (SearchQuickResponses 404 ResourceNotFoundException), wisdom-session-error-paths#task#5 (QueryAssistant 404 ResourceNotFoundException).
5. Equivalently, point a boto3 `wisdom` client at the emulator and call delete_assistant(assistantId='00000000-0000-0000-0000-000000000000') or list_knowledge_bases(): the call is retried to exhaustion and raises a ClientError with code AmbiguousService and HTTP 500 instead of the modeled 404 ResourceNotFoundException / 200 page.
