Issue-Source: trace-sweep-divergence
Issue-Trace: wisdom-assistant-association-error-paths
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wisdom-assistant-association-error-paths wisdom-assistant-association-lifecycle wisdom-session-recommendations-query-workflow wisdom-session-error-paths
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.CASE_SUMMARIZATION.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.EMAIL_GENERATIVE_ANSWER.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.EMAIL_OVERVIEW.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.EMAIL_RESPONSE.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.MANUAL_SEARCH.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.NOTE_TAKING.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.SELF_SERVICE.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.integrationConfiguration.topicIntegrationArn
Issue-Key: wisdom|CreateSession|2xx->2xx|$.session.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId
Issue-Key: wisdom|CreateSession|2xx->2xx|$.session.aiAgentConfiguration.CASE_SUMMARIZATION.aiAgentId
Issue-Key: wisdom|CreateSession|2xx->2xx|$.session.aiAgentConfiguration.EMAIL_GENERATIVE_ANSWER.aiAgentId
Issue-Key: wisdom|CreateSession|2xx->2xx|$.session.aiAgentConfiguration.EMAIL_OVERVIEW.aiAgentId
Issue-Key: wisdom|CreateSession|2xx->2xx|$.session.aiAgentConfiguration.EMAIL_RESPONSE.aiAgentId
Issue-Key: wisdom|CreateSession|2xx->2xx|$.session.aiAgentConfiguration.MANUAL_SEARCH.aiAgentId
Issue-Key: wisdom|CreateSession|2xx->2xx|$.session.aiAgentConfiguration.NOTE_TAKING.aiAgentId
Issue-Key: wisdom|CreateSession|2xx->2xx|$.session.aiAgentConfiguration.SELF_SERVICE.aiAgentId
Issue-Key: wisdom|GetSession|2xx->2xx|$.session.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId
Issue-Key: wisdom|GetSession|2xx->2xx|$.session.aiAgentConfiguration.CASE_SUMMARIZATION.aiAgentId
Issue-Key: wisdom|GetSession|2xx->2xx|$.session.aiAgentConfiguration.EMAIL_GENERATIVE_ANSWER.aiAgentId
Issue-Key: wisdom|GetSession|2xx->2xx|$.session.aiAgentConfiguration.EMAIL_OVERVIEW.aiAgentId
Issue-Key: wisdom|GetSession|2xx->2xx|$.session.aiAgentConfiguration.EMAIL_RESPONSE.aiAgentId
Issue-Key: wisdom|GetSession|2xx->2xx|$.session.aiAgentConfiguration.MANUAL_SEARCH.aiAgentId
Issue-Key: wisdom|GetSession|2xx->2xx|$.session.aiAgentConfiguration.NOTE_TAKING.aiAgentId
Issue-Key: wisdom|GetSession|2xx->2xx|$.session.aiAgentConfiguration.SELF_SERVICE.aiAgentId
Issue-Key: wisdom|GetSession|2xx->4xx|$.session
Issue-Key: wisdom|GetSession|2xx->4xx|status
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# Wisdom assistant and session service-managed defaults missing: random aiAgentIds and no SYSTEM session

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssistant

## Description
The emulator does not model Amazon Q in Connect / Wisdom's service-managed defaults for an assistant: the built-in (system) AI Agent set that every assistant and session is pre-configured with, and the system-managed default session that every assistant owns. 4 causes, all in the wisdom service, all rooted in the same missing default-seeding step.

Cause 1 of 4: CreateAssistant returns a freshly minted random aiAgentId for every AI Agent type.
AWS: CreateAssistant answers 200 with AssistantData.aiAgentConfiguration already populated, one entry per AIAgentType key (ANSWER_RECOMMENDATION, MANUAL_SEARCH, SELF_SERVICE, EMAIL_OVERVIEW, EMAIL_RESPONSE, EMAIL_GENERATIVE_ANSWER, NOTE_TAKING, CASE_SUMMARIZATION), each pointing at the service-owned built-in AI Agent for that type together with its published version qualifier. Smithy: AssistantData.aiAgentConfiguration -> AIAgentConfigurationMap (key AIAgentType, value AIAgentConfigurationData), doc 'The configuration of the AI Agents (mapped by AI Agent Type to AI Agent version) that is set on the Amazon Q in Connect Assistant'; AIAgentConfigurationData.aiAgentId target UuidWithQualifier, @required, @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}(:[A-Z0-9_$]+){0,1}$. The requirement is that these ids are SERVICE constants, not per-resource values: they must be identical for every assistant and session in the account/region and must resolve to a real AI Agent, and each must carry the built-in agent's version qualifier (the corpus records the same eight id:version pairs in four independently recorded fixtures, e.g. ANSWER_RECOMMENDATION -> '3025a3fa-5419-4c9c-a040-877213b93f70:12', MANUAL_SEARCH -> '652f0287-1610-4fee-bbe3-c687c13f5ec3:9', CASE_SUMMARIZATION -> '9114812c-2a1e-4501-8817-daec4223ce57:3').
Emulator: answers 200 but fabricates a brand-new random UUID per assistant per AI Agent type, always with qualifier ':1' (e.g. ANSWER_RECOMMENDATION -> 'c35e88e6-95d8-e562-7ec7-f30d1a1983c9:1', MANUAL_SEARCH -> '8f779f0c-750b-4b55-ecc0-173b9997813c:1'); a second CreateAssistant yields a different set again.
Why: a client that reads aiAgentConfiguration and then calls GetAIAgent / ListAIAgentVersions / UpdateAssistantAIAgent with the returned aiAgentId addresses an agent that does not exist, and no two assistants agree on the default agent set, so any code that pins or compares the default agent for a type breaks.
Fix: assistant state construction in the wisdom/qconnect CreateAssistant handler plus the shared default (system) AI-Agent registry it should seed from — not the serializer, which already emits the map correctly.
Traces: wisdom-assistant-association-error-paths#setup#1, wisdom-assistant-association-lifecycle#task#1 (same cluster also covers wisdom-session-recommendations-query-workflow#task#1).

Cause 2 of 4: CreateSession propagates the same fabricated aiAgentIds.
AWS: CreateSession answers 200 with SessionData.aiAgentConfiguration inherited from the assistant, i.e. the identical service-default id:version pairs described in cause 1. Smithy: SessionData.aiAgentConfiguration -> AIAgentConfigurationMap, doc 'The configuration of the AI Agents (mapped by AI Agent Type to AI Agent version) that should be used by Amazon Q in Connect for this Session'.
Emulator: emits eight random per-session UUIDs with ':1' qualifiers instead of the inherited defaults.
Why: the session's agent configuration does not match its assistant's, so a client cannot correlate session behavior with the assistant configuration it set, and the ids resolve to nothing.
Fix: CreateSession handler / session state — inherit the assistant's aiAgentConfiguration rather than generating one.
Traces: wisdom-session-recommendations-query-workflow#task#2.

Cause 3 of 4: GetSession echoes the fabricated session aiAgentIds.
AWS: GetSession returns the session's stored aiAgentConfiguration, equal to the defaults in cause 1.
Emulator: returns the random ':1' ids stored by cause 2.
Why: read-after-write on a session reports agent ids that address no AI Agent.
Fix: same session state as cause 2; GetSession simply serves what was stored.
Traces: wisdom-session-recommendations-query-workflow#task#3.

Cause 4 of 4: the assistant's system-managed default session does not exist.
AWS: Q in Connect creates a default, service-managed session for each assistant, reachable at sessionId 00000000-0000-0000-0000-000000000000. GetSession on it answers 200 with SessionData whose origin is SYSTEM, name 'QinConnectSystemSession', description 'System-managed session for Amazon Q in Connect.', sessionArn of the form arn:aws:wisdom:<region>:<account>:session/<assistantId>/00000000-0000-0000-0000-000000000000 (the region, account and assistantId are taken from the request/state; only the zero sessionId is fixed text), and the default aiAgentConfiguration of cause 1. Smithy: SessionData.origin -> Origin enum [SYSTEM, CUSTOMER], doc 'SYSTEM for a default Session created by Amazon Q in Connect or CUSTOMER for a Session created by calling CreateSession'.
Emulator: answers 404 with no session body at all for that sessionId, i.e. it treats the system session as a nonexistent resource.
Why: Connect-integrated clients that read or notify on the system session (the path used when no explicit session was created) get ResourceNotFoundException and cannot proceed; the SYSTEM value of Origin is unreachable in the emulator.
Fix: assistant creation should materialize the system session in state, and the GetSession handler must resolve the all-zero sessionId to it.
Traces: wisdom-session-error-paths#task#3.

Note: the earlier issue 'QConnect assistant and session configuration state is not persisted or serialized' (20261023T054358Z, status fixed) covers persistence/serialization of the map; it does not cover which ids belong in it or the system session, which is what this issue reports.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, score 65.9%).

Cause 1 — wisdom-assistant-association-error-paths#setup#1, CreateAssistant, expected 200 / actual 200:
  $.assistant.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId: expected '3025a3fa-5419-4c9c-a040-877213b93f70:12', got 'c35e88e6-95d8-e562-7ec7-f30d1a1983c9:1'
  $.assistant.aiAgentConfiguration.MANUAL_SEARCH.aiAgentId: expected '652f0287-1610-4fee-bbe3-c687c13f5ec3:9', got '8f779f0c-750b-4b55-ecc0-173b9997813c:1'
  $.assistant.aiAgentConfiguration.SELF_SERVICE.aiAgentId: expected '529207fe-80fd-42be-bcf2-facd2141aec7:5', got 'a076c590-ada8-5375-6b1e-4001c6b84b87:1'
  $.assistant.aiAgentConfiguration.EMAIL_OVERVIEW / EMAIL_RESPONSE / EMAIL_GENERATIVE_ANSWER / NOTE_TAKING / CASE_SUMMARIZATION .aiAgentId: same shape of mismatch (expected '91bb967b-bd5e-4e02-a8d5-cb7c5b2bf9c7:5', '8f4e7d2a-9c31-4b5e-a6f8-e2d9c7b54321:7', '4d7e9f2a-1b3c-5d6e-8f7a-9b0c1d2e3f4a:7', '7fc84960-5b46-4e44-9dd6-5956b2acfaa4:3', '9114812c-2a1e-4501-8817-daec4223ce57:3'; got fresh UUIDs with ':1').
  The same eight expected ids recur in wisdom-assistant-association-lifecycle#task#1 and in the session fixtures, while the emulator's differ on every call — the expected set is a service constant, the emulator's is per-call.

Cause 2 — wisdom-session-recommendations-query-workflow#task#2, CreateSession, 200/200: all eight $.session.aiAgentConfiguration.*.aiAgentId mismatched the same way (e.g. ANSWER_RECOMMENDATION expected '3025a3fa-5419-4c9c-a040-877213b93f70:12', got 'd30f3050-b94e-0a1c-5f37-926877516bde:1').

Cause 3 — wisdom-session-recommendations-query-workflow#task#3, GetSession, 200/200: identical eight mismatches, confirming the fabricated ids are what state holds.

Cause 4 — wisdom-session-error-paths#task#3, GetSession, expected 200, actual 404:
  $.session missing; AWS body is {aiAgentConfiguration: {<8 default agents>}, description: 'System-managed session for Amazon Q in Connect.', name: 'QinConnectSystemSession', origin: 'SYSTEM', sessionArn: 'arn:aws:wisdom:<region>:<account>:session/<assistantId>/00000000-0000-0000-0000-000000000000', sessionId: '00000000-0000-0000-0000-000000000000'}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: POST /assistants (CreateAssistant) and read assistant.aiAgentConfiguration — every aiAgentId is a fresh random UUID with qualifier ':1'; repeat the call and observe a different set, instead of the account's stable built-in AI Agent ids with their version qualifiers. Fixture wisdom-assistant-association-error-paths step setup#1; also wisdom-assistant-association-lifecycle#task#1.
3. Cause 2: POST /assistants/{assistantId}/sessions (CreateSession) and compare session.aiAgentConfiguration with the assistant's — the ids differ from both the assistant's and the service defaults. Fixture wisdom-session-recommendations-query-workflow step task#2.
4. Cause 3: GET /assistants/{assistantId}/sessions/{sessionId} (GetSession) on that session — the same fabricated ids come back. Fixture wisdom-session-recommendations-query-workflow step task#3.
5. Cause 4: GET /assistants/{assistantId}/sessions/00000000-0000-0000-0000-000000000000 (GetSession) right after creating an assistant — the emulator answers 404 instead of 200 with the SYSTEM-origin 'QinConnectSystemSession'. Fixture wisdom-session-error-paths step task#3.
