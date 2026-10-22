Issue-Source: trace-sweep-divergence
Issue-Trace: qconnect-ai-agent-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: qconnect-ai-agent-lifecycle qconnect-ai-prompt-lifecycle qconnect-assistant-ai-agent-configuration qconnect-error-paths qconnect-ai-guardrail-lifecycle qconnect-assistant-association-lifecycle qconnect-assistant-lifecycle-and-tags qconnect-session-lifecycle qconnect-kb-error-paths qconnect-message-template-attachment-render qconnect-message-template-lifecycle qconnect-message-template-versions
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: wisdom|CreateAssistant|2xx->5xx|status
Issue-Key: wisdom|CreateAssistant|2xx->5xx|$.assistant
Issue-Key: wisdom|CreateAssistant|2xx->5xx|$headers.content-type
Issue-Key: wisdom|CreateKnowledgeBase|2xx->5xx|$.knowledgeBase
Issue-Key: wisdom|CreateKnowledgeBase|2xx->5xx|$headers.content-type
Issue-Key: wisdom|CreateKnowledgeBase|2xx->5xx|status
Issue-Key: wisdom|ListAssistants|2xx->5xx|$.assistantSummaries
Issue-Key: wisdom|ListAssistants|2xx->5xx|$headers.content-type
Issue-Key: wisdom|ListAssistants|2xx->5xx|status
Issue-Key: wisdom|GetAssistant|4xx->5xx|$.message
Issue-Key: wisdom|GetAssistant|4xx->5xx|$headers.content-type
Issue-Key: wisdom|GetAssistant|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: wisdom|GetAssistant|4xx->5xx|status
Issue-Key: wisdom|CreateAIAgent|4xx->5xx|$.message
Issue-Key: wisdom|CreateAIAgent|4xx->5xx|$headers.content-type
Issue-Key: wisdom|CreateAIAgent|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: wisdom|CreateAIAgent|4xx->5xx|status
Issue-Key: wisdom|CreateSession|4xx->5xx|$.message
Issue-Key: wisdom|CreateSession|4xx->5xx|$headers.content-type
Issue-Key: wisdom|CreateSession|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: wisdom|CreateSession|4xx->5xx|status
Issue-Key: wisdom|GetKnowledgeBase|4xx->5xx|$.message
Issue-Key: wisdom|GetKnowledgeBase|4xx->5xx|$headers.content-type
Issue-Key: wisdom|GetKnowledgeBase|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: wisdom|GetKnowledgeBase|4xx->5xx|status
Issue-Key: wisdom|CreateKnowledgeBase|4xx->5xx|$.message
Issue-Key: wisdom|CreateKnowledgeBase|4xx->5xx|$headers.content-type
Issue-Key: wisdom|CreateKnowledgeBase|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: wisdom|CreateKnowledgeBase|4xx->5xx|status
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# Wisdom/QConnect wire fixtures all refused 500 AmbiguousService: router keys on the shared 'wisdom' signing name instead of the restJson1 route

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssistant

## Description
This bundle carries 9 causes, all on service `wisdom`. Every one of them is a wire step that never reached a Wisdom/QConnect handler at all: the emulator's request router refuses the request with an emulator-internal 500 `AmbiguousService`, serialized in the wrong protocol envelope. 114 corpus steps ran, 3 passed, 19 mismatched and 92 were blocked downstream, so the service is effectively 100% down on the wire while reporting as bootstrapped.

Shared AWS grounding (cited once, applies to all 9 causes). Both models in this repo bind service shape `WisdomService` version 2020-10-19 with protocol `restJson1`: services/service-com-amazonaws-wisdom-wisdomservice (smithy_query service_info: sdkId `Wisdom`, sigv4Name `wisdom`, arnNamespace `wisdom`, 41 ops) and services/service-com-amazonaws-qconnect-wisdomservice (sdkId `QConnect`, sigv4Name `wisdom`, arnNamespace `wisdom`, 94 ops, endpoint templates https://wisdom.{Region}.{dnsSuffix}). AWS deliberately serves Q Connect on Wisdom's endpoint prefix and signing name; because restJson1 carries no X-Amz-Target, a request is dispatched by HTTP method + request URI, so a shared signing name is never a reason to refuse service. restJson1 responses — success and error alike — are `content-type: application/json`, and modeled errors carry `x-amzn-errortype: <ShapeName>`. `AmbiguousService` is not an error shape in either model; it is emulator-internal and appears in no AWS Wisdom/QConnect response.

Shared Fix (applies to all 9): the service-registry / request-router dispatch layer, not any Wisdom or QConnect operation handler. The registry must admit two services that share endpointPrefix/sigv4Name/arnNamespace and resolve them deterministically — by registration identity (sdkId `Wisdom` vs `QConnect`) and/or by matching the concrete method+URI against each registered service's restJson1 routes — instead of counting claimants and failing closed. Secondarily, the router's own error path must serialize in the target service's protocol: a restJson1 service must answer `content-type: application/json`, never `application/x-amz-json-1.1`. Until routing resolves, none of the 9 expected wire outcomes below has been exercised even once, so each must be re-replayed individually after the fix.

Related open issue 20261022T031220Z-wisdom-all-wisdom-requests-rejected-with-ambiguousservice-two-emulated.md reports the same router collision from the BEHAVIORAL corpus (simparity-behavioral-aws-v1, 47 check ids). It does NOT cover these causes: it carries no wire fixture/step replay keys, so replaying it exercises none of the 9 wire steps below, and the replay report confirms none of these clusters is owned by it.

Cause 1 of 9 — CreateAssistant, 200 success envelope. AWS: Smithy CreateAssistant POST /assistants -> 200 with output member `assistant` (AssistantData); errors AccessDeniedException 403, ConflictException 409, ServiceQuotaExceededException 402, UnauthorizedException 401, ValidationException 400. Emulator: 500 with errortype `AmbiguousService`. Why: no assistant can be created, so every downstream step of the fixture is blocked. Traces: qconnect-ai-agent-lifecycle#setup#1 (cluster also covers qconnect-ai-prompt-lifecycle#setup#1, qconnect-assistant-ai-agent-configuration#setup#1, qconnect-error-paths#setup#1).

Cause 2 of 9 — CreateAssistant, 200 envelope plus wrong content-type. AWS: as cause 1; restJson1 success body is `application/json` and carries `assistant` with assistantId/assistantArn, `status` ACTIVE, `type` AGENT, `capabilityConfiguration`, `aiAgentConfiguration`, `integrationConfiguration`, `orchestratorConfigurationList`, `tags`. Emulator: 500, `$.assistant` absent entirely, and `content-type: application/x-amz-json-1.1` where restJson1 requires `application/json`. Why: a client parsing a restJson1 response gets an awsJson1.1-typed body with no modeled member. Traces: qconnect-ai-guardrail-lifecycle#setup#1 (cluster also covers qconnect-assistant-association-lifecycle#setup#1, qconnect-assistant-lifecycle-and-tags#task#1, qconnect-session-lifecycle#setup#1).

Cause 3 of 9 — CreateKnowledgeBase, 200 success envelope. AWS: Smithy CreateKnowledgeBase POST /knowledgeBases -> 200 with `knowledgeBase` (KnowledgeBaseData: knowledgeBaseId/knowledgeBaseArn, knowledgeBaseType, status ACTIVE, name, tags); errors AccessDenied 403, Conflict 409, ServiceQuotaExceeded 402, Unauthorized 401, Validation 400. Emulator: 500, `$.knowledgeBase` missing, content-type `application/x-amz-json-1.1`. Why: no knowledge base exists, blocking every content/message-template fixture built on one. Traces: qconnect-assistant-association-lifecycle#setup#2 (cluster also covers qconnect-kb-error-paths#setup#1, qconnect-message-template-attachment-render#setup#2, qconnect-message-template-lifecycle#task#2, qconnect-message-template-versions#setup#2).

Cause 4 of 9 — ListAssistants, 200 paginated envelope. AWS: Smithy ListAssistants GET /assistants -> 200 with REQUIRED output member `assistantSummaries` (list) and optional `nextToken`; paginated(items=assistantSummaries). Emulator: 500, `$.assistantSummaries` missing, content-type `application/x-amz-json-1.1`. Why: a required response member is absent, so SDK paginators fail rather than returning an empty or populated page. Traces: qconnect-assistant-lifecycle-and-tags#task#7.

Cause 5 of 9 — GetAssistant, 404 ResourceNotFoundException for an unknown assistant id. AWS: Smithy GetAssistant GET /assistants/{assistantId} models ResourceNotFoundException httpError 404; the recorded response is 404, `x-amzn-errortype: ResourceNotFoundException`, `content-type: application/json`, message 'Assistant does not exist.' (fixed service text, example). Emulator: 500, errortype `AmbiguousService`, content-type `application/x-amz-json-1.1`, message '2 emulated services all claim requests for \'wisdom\': wisdom, wisdom; the request is rejected rather than served by an arbitrary one'. Why: client not-found branches keyed on ResourceNotFoundException never fire, and a 500 is retried as transient. Traces: qconnect-error-paths#task#2.

Cause 6 of 9 — CreateAIAgent, 404 ResourceNotFoundException for an unknown parent assistant. AWS: Smithy (qconnect model) CreateAIAgent POST /assistants/{assistantId}/aiagents models ResourceNotFoundException 404 (alongside Conflict 409, Unauthorized 401, Validation 400, ServiceQuotaExceeded 402, Throttling 400); recorded 404 with errortype ResourceNotFoundException, content-type application/json, message 'Assistant does not exist.' as example. Emulator: 500, errortype `AmbiguousService`, content-type `application/x-amz-json-1.1`. Why: a child-resource create against a missing parent looks like a server fault instead of a modeled client error. Traces: qconnect-error-paths#task#3.

Cause 7 of 9 — CreateSession, 401 UnauthorizedException. AWS: Smithy (qconnect model) CreateSession POST /assistants/{assistantId}/sessions models UnauthorizedException httpError 401; recorded 401 with `x-amzn-errortype: UnauthorizedException`, content-type application/json, message 'Unauthorized' as example. Emulator: 500, errortype `AmbiguousService`, content-type `application/x-amz-json-1.1`. Why: the authorization-failure status and code are lost, so credential/permission handling cannot be distinguished from a server error. Traces: qconnect-error-paths#task#4.

Cause 8 of 9 — GetKnowledgeBase, 404 ResourceNotFoundException for an unknown knowledge-base id. AWS: Smithy GetKnowledgeBase GET /knowledgeBases/{knowledgeBaseId} models ResourceNotFoundException 404; recorded 404, errortype ResourceNotFoundException, content-type application/json, message 'Knowledge Base does not exist.' as example. Emulator: 500, errortype `AmbiguousService`, content-type `application/x-amz-json-1.1`. Why: as cause 5, on the knowledge-base read path. Traces: qconnect-kb-error-paths#task#2.

Cause 9 of 9 — CreateKnowledgeBase, 409 ConflictException on a duplicate name. AWS: Smithy CreateKnowledgeBase models ConflictException httpError 409; recorded 409, `x-amzn-errortype: ConflictException`, content-type application/json, message 'Name is already in use' as example. Emulator: 500, errortype `AmbiguousService`, content-type `application/x-amz-json-1.1`. Why: duplicate-name conflict is indistinguishable from a server fault, so idempotency/conflict handling and retry decisions are both wrong. Traces: qconnect-kb-error-paths#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, /__admin/health 200, data listener accepting; bootstrap_signal=false; state_reset_failures=0): steps_total=114, steps_passed=3, steps_mismatched=19, steps_blocked=92, score_pct=2.6, divergence_count=9 — all 9 on service `wisdom`, all verdict=mismatch, all actual_status=500, none stub-blocked, none owned by an open issue.

Success-envelope clusters (expected 200, got 500):
  qconnect-ai-agent-lifecycle#setup#1          CreateAssistant      key service=wisdom|CreateAssistant|status=200->500|keys=status (+3 affected)
  qconnect-ai-guardrail-lifecycle#setup#1      CreateAssistant      $.assistant missing; $headers.content-type expected 'application/json', got 'application/x-amz-json-1.1' (+3 affected)
  qconnect-assistant-association-lifecycle#setup#2  CreateKnowledgeBase  $.knowledgeBase missing; content-type 'application/json' -> 'application/x-amz-json-1.1' (+4 affected)
  qconnect-assistant-lifecycle-and-tags#task#7 ListAssistants       $.assistantSummaries missing (required member); content-type 'application/json' -> 'application/x-amz-json-1.1'

Error-path clusters (expected modeled status/code, got 500 AmbiguousService):
  qconnect-error-paths#task#2     GetAssistant         expected 404 x-amzn-errortype ResourceNotFoundException -> got 500 AmbiguousService
  qconnect-error-paths#task#3     CreateAIAgent        expected 404 ResourceNotFoundException              -> got 500 AmbiguousService
  qconnect-error-paths#task#4     CreateSession        expected 401 UnauthorizedException                  -> got 500 AmbiguousService
  qconnect-kb-error-paths#task#2  GetKnowledgeBase     expected 404 ResourceNotFoundException              -> got 500 AmbiguousService
  qconnect-kb-error-paths#task#6  CreateKnowledgeBase  expected 409 ConflictException                      -> got 500 AmbiguousService

Every error-path cluster also reports content-type expected 'application/json', got 'application/x-amz-json-1.1', and the identical body message '2 emulated services all claim requests for \'wisdom\': wisdom, wisdom; the request is rejected rather than served by an arbitrary one'. Both claimants print as the same token `wisdom`, consistent with the router keying on sigv4Name/endpointPrefix/arnNamespace (all `wisdom` in both models) rather than on sdkId or the restJson1 method+URI.

Recorded message texts above are examples of the service's fixed not-found/conflict/unauthorized wording; the requirements asserted here are the HTTP statuses (200/401/404/409), the `x-amzn-errortype` codes (ResourceNotFoundException, UnauthorizedException, ConflictException), the `application/json` content-type required by restJson1, and the presence of the modeled output members (`assistant`, `knowledgeBase`, `assistantSummaries`).

## Steps to Reproduce
1. Clone mainline and build the emulator; confirm both service crates service-com-amazonaws-wisdom-wisdomservice and service-com-amazonaws-qconnect-wisdomservice are registered.
2. Replay the simparity-wire-aws-v1 wire corpus against it.
3. Representative step: fixture qconnect-ai-agent-lifecycle, step setup#1 — POST /assistants with a restJson1 CreateAssistant body signed for signing name `wisdom`. Expected HTTP 200, content-type application/json, body containing `assistant`. Observed HTTP 500, content-type application/x-amz-json-1.1, x-amzn-errortype AmbiguousService.
4. Re-replay each of the other 8 bundled steps to confirm each expected wire outcome independently: qconnect-ai-guardrail-lifecycle#setup#1 (CreateAssistant 200 + content-type), qconnect-assistant-association-lifecycle#setup#2 (CreateKnowledgeBase 200), qconnect-assistant-lifecycle-and-tags#task#7 (ListAssistants 200 with assistantSummaries), qconnect-error-paths#task#2 (GetAssistant 404 ResourceNotFoundException), qconnect-error-paths#task#3 (CreateAIAgent 404 ResourceNotFoundException), qconnect-error-paths#task#4 (CreateSession 401 UnauthorizedException), qconnect-kb-error-paths#task#2 (GetKnowledgeBase 404 ResourceNotFoundException), qconnect-kb-error-paths#task#6 (CreateKnowledgeBase 409 ConflictException).
5. Equivalently, point a boto3 `wisdom` or `qconnect` client at the emulator and call any operation (e.g. create_assistant(name='probe', type='AGENT')): the call is retried to exhaustion and raises ClientError code AmbiguousService instead of returning the modeled response.
