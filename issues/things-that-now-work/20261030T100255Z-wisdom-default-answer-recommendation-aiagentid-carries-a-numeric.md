Issue-Source: trace-sweep-divergence
Issue-Trace: qconnect-assistant-association-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: qconnect-assistant-association-lifecycle qconnect-assistant-lifecycle-and-tags qconnect-session-lifecycle
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId
Issue-Key: wisdom|CreateSession|2xx->2xx|$.session.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId
Issue-Key: wisdom|GetSession|2xx->2xx|$.session.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId
Issue-Commit: 7d4a82fc68d671fcbf7a8009b036d72ed0c4ec91

# Wisdom default ANSWER_RECOMMENDATION aiAgentId carries a numeric version qualifier instead of $LATEST on assistant and session writes/reads

> _Note: this issue was identified against the code base at commit `7d4a82fc68d671fcbf7a8009b036d72ed0c4ec91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssistant

## Description
Every wisdom (Amazon Q in Connect) response that projects an aiAgentConfiguration map qualifies the account's built-in service-default ANSWER_RECOMMENDATION agent with a NUMERIC version qualifier instead of the $LATEST qualifier AWS uses for the service-default agent. The default agent UUID itself is now correct (a previous defect that fabricated random UUIDs has been fixed and its issue resolved), so the remaining divergence is purely the qualifier appended to the default agent id. 3 causes, all in the wisdom service, all rooted in the same place: the AI-Agent-configuration defaulting that seeds assistant and session state picks a concrete numeric version for the built-in agent rather than its $LATEST alias, and every write and read path projects that same stored value.

Cause 1 of 3: CreateAssistant returns the default ANSWER_RECOMMENDATION agent pinned to a numeric version.
AWS: CreateAssistant answers 200 with AssistantData.aiAgentConfiguration pre-populated with the account's built-in service-default agent for each AIAgentType. For ANSWER_RECOMMENDATION the default agent is reported as the default agent's UUID suffixed with the qualifier ':$LATEST' — the floating alias, not a pinned numeric version. Smithy: CreateAssistant POST /assistants -> CreateAssistantResponse.assistant -> AssistantData.aiAgentConfiguration -> AIAgentConfigurationMap (key AIAgentType, value AIAgentConfigurationData); AIAgentConfigurationData.aiAgentId -> UuidWithQualifier, @required, @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}(:[A-Z0-9_$]+){0,1}$, doc 'The ID of the AI Agent to be configured'. The pattern admits both a numeric and a '$LATEST' qualifier, so the model does not by itself decide which; the recordings do: the SAME default UUID appears with the ':$LATEST' qualifier across independently recorded fixtures (recorded behavior), which identifies $LATEST as the qualifier the service attaches to its built-in default agent. The requirement is the qualifier form, not any one UUID: the default agent id is an account/service constant whose variable part is the UUID and whose fixed part is the ':$LATEST' suffix.
Emulator: answers 200 with the right default UUID but the wrong qualifier — ANSWER_RECOMMENDATION.aiAgentId ends in a numeric version (':12' in this replay) where the recording has ':$LATEST'. Every other member of the response matches.
Why: a client that reads the default aiAgentConfiguration and then addresses that agent (GetAIAgent, ListAIAgentVersions, UpdateAssistantAIAgent, or a session override) sends a pinned version that does not exist, so the call fails or resolves to the wrong agent version; and a client that expects the floating alias silently stops tracking new default-agent versions.
Fix: the wisdom/qconnect AI-Agent-configuration defaulting in the assistant state seeded by the CreateAssistant handler — the built-in default agent must be recorded with its $LATEST qualifier rather than a materialized version number. The serializer is correct: the map keys and shape match.
Traces: qconnect-assistant-association-lifecycle#setup#1 (same cluster also covers qconnect-assistant-lifecycle-and-tags#task#1 and qconnect-session-lifecycle#setup#1).

Cause 2 of 3: CreateSession propagates the same mis-qualified default into SessionData.
AWS: CreateSession answers 200 with SessionData.aiAgentConfiguration carrying the AI Agent configuration that applies to the session — inherited from the assistant when the request does not override a type. Smithy: CreateSession POST /assistants/{assistantId}/sessions -> CreateSessionResponse.session -> SessionData.aiAgentConfiguration -> AIAgentConfigurationMap, doc 'The configuration of the AI Agents (mapped by AI Agent Type to AI Agent version) that should be used by Amazon Q in Connect for this Session'. The inherited default for ANSWER_RECOMMENDATION therefore carries the same ':$LATEST' qualifier the assistant reports (recorded behavior: byte-identical id across the assistant and session fixtures).
Emulator: answers 200 with session.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId ending in a numeric version (':12') instead of ':$LATEST'.
Why: the session's effective agent configuration disagrees with what AWS would report, so a client reconciling session configuration against the assistant's, or carrying the session's agent id into a subsequent call, addresses a pinned version rather than the floating default.
Fix: same defaulting/inheritance path as cause 1, as consumed by the CreateSession handler and the session state it writes.
Traces: qconnect-session-lifecycle#task#2.

Cause 3 of 3: GetSession reads back the same mis-qualified default.
AWS: GetSession (@readonly, GET /assistants/{assistantId}/sessions/{sessionId}) answers 200 with SessionData.aiAgentConfiguration equal to what the session was created with, so the ANSWER_RECOMMENDATION default is reported with the ':$LATEST' qualifier (recorded behavior).
Emulator: answers 200 with session.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId ending in a numeric version (':12') instead of ':$LATEST' — consistent with the create response, so this is the stored value being wrong, not a read-path-only fault.
Why: read-after-write on a session never yields the default agent id in the form AWS serves, so no client can round-trip the default configuration.
Fix: same stored session/assistant AI-Agent configuration as causes 1 and 2; the GetSession handler projects it unchanged and needs no separate change once the stored qualifier is correct.
Traces: qconnect-session-lifecycle#task#3.

Relation to open issues: '20261029T194637Z-wisdom-assistant-ai-agent-configuration-reads-return-fabricated.md' reports the GetAssistant/ListAssistants read paths and the unpersisted UpdateAssistantAIAgent override and owns those replay keys; it does not cover the CreateAssistant, CreateSession or GetSession clusters, which the replay reports as unowned. The earlier issue that did own these three clusters is no longer open (resolved), and the replay confirms owned_by is empty for all three.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, steps_total=114, steps_passed=48, score 42.1%). All three clusters are verdict=mismatch with expected_status 200 and actual_status 200 — only the body member diverges.

Cause 1 — qconnect-assistant-association-lifecycle#setup#1, CreateAssistant, expected 200 / actual 200, affected_count=3 (cluster also affects qconnect-assistant-lifecycle-and-tags#task#1 and qconnect-session-lifecycle#setup#1):
  $.assistant.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId: value_mismatch (expected '3025a3fa-5419-4c9c-a040-877213b93f70:$LATEST', got '3025a3fa-5419-4c9c-a040-877213b93f70:12')

Cause 2 — qconnect-session-lifecycle#task#2, CreateSession, expected 200 / actual 200:
  $.session.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId: value_mismatch (expected '3025a3fa-5419-4c9c-a040-877213b93f70:$LATEST', got '3025a3fa-5419-4c9c-a040-877213b93f70:12')

Cause 3 — qconnect-session-lifecycle#task#3, GetSession, expected 200 / actual 200:
  $.session.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId: value_mismatch (expected '3025a3fa-5419-4c9c-a040-877213b93f70:$LATEST', got '3025a3fa-5419-4c9c-a040-877213b93f70:12')

The UUID part agrees in all three clusters and across unrelated fixtures, and only the qualifier after the colon differs, which is what isolates the defect to the qualifier the default-agent seeding attaches. The UUID '3025a3fa-5419-4c9c-a040-877213b93f70' and the number '12' are quoted only as the recorded example values; the requirement is that the built-in default ANSWER_RECOMMENDATION agent is reported with the fixed ':$LATEST' suffix (the UUID being whatever default agent the environment holds), not with any materialized version number.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: POST /assistants (CreateAssistant) with a name and type, and read assistant.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId in the 200 response — it ends in a numeric version qualifier (':12') where the recording ends in ':$LATEST'. Fixture qconnect-assistant-association-lifecycle step setup#1 (cluster also covers qconnect-assistant-lifecycle-and-tags#task#1 and qconnect-session-lifecycle#setup#1).
3. Cause 2: POST /assistants/{assistantId}/sessions (CreateSession) against that assistant without overriding ANSWER_RECOMMENDATION, and read session.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId — same numeric qualifier instead of ':$LATEST'. Fixture qconnect-session-lifecycle step task#2.
4. Cause 3: GET /assistants/{assistantId}/sessions/{sessionId} (GetSession) for that session and read the same member — the stored value is returned unchanged, still numerically qualified. Fixture qconnect-session-lifecycle step task#3.
5. In each case compare the returned aiAgentId's suffix against the recorded one: the UUID matches, the qualifier does not.
