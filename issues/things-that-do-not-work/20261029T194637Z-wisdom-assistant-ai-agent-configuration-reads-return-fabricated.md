Issue-Source: trace-sweep-divergence
Issue-Trace: qconnect-assistant-ai-agent-configuration
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: qconnect-assistant-ai-agent-configuration qconnect-assistant-lifecycle-and-tags
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: wisdom|GetAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId
Issue-Key: wisdom|GetAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.CASE_SUMMARIZATION.aiAgentId
Issue-Key: wisdom|GetAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.EMAIL_GENERATIVE_ANSWER.aiAgentId
Issue-Key: wisdom|GetAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.EMAIL_OVERVIEW.aiAgentId
Issue-Key: wisdom|GetAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.EMAIL_RESPONSE.aiAgentId
Issue-Key: wisdom|GetAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.MANUAL_SEARCH.aiAgentId
Issue-Key: wisdom|GetAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.NOTE_TAKING.aiAgentId
Issue-Key: wisdom|GetAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.SELF_SERVICE.aiAgentId
Issue-Key: wisdom|ListAssistants|2xx->2xx|$.assistantSummaries[name="sim2real-probe-qconnect-assistant-26ab1a1f"].aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId
Issue-Key: wisdom|ListAssistants|2xx->2xx|$.assistantSummaries[name="sim2real-probe-qconnect-assistant-26ab1a1f"].aiAgentConfiguration.CASE_SUMMARIZATION.aiAgentId
Issue-Key: wisdom|ListAssistants|2xx->2xx|$.assistantSummaries[name="sim2real-probe-qconnect-assistant-26ab1a1f"].aiAgentConfiguration.EMAIL_GENERATIVE_ANSWER.aiAgentId
Issue-Key: wisdom|ListAssistants|2xx->2xx|$.assistantSummaries[name="sim2real-probe-qconnect-assistant-26ab1a1f"].aiAgentConfiguration.EMAIL_OVERVIEW.aiAgentId
Issue-Key: wisdom|ListAssistants|2xx->2xx|$.assistantSummaries[name="sim2real-probe-qconnect-assistant-26ab1a1f"].aiAgentConfiguration.EMAIL_RESPONSE.aiAgentId
Issue-Key: wisdom|ListAssistants|2xx->2xx|$.assistantSummaries[name="sim2real-probe-qconnect-assistant-26ab1a1f"].aiAgentConfiguration.MANUAL_SEARCH.aiAgentId
Issue-Key: wisdom|ListAssistants|2xx->2xx|$.assistantSummaries[name="sim2real-probe-qconnect-assistant-26ab1a1f"].aiAgentConfiguration.NOTE_TAKING.aiAgentId
Issue-Key: wisdom|ListAssistants|2xx->2xx|$.assistantSummaries[name="sim2real-probe-qconnect-assistant-26ab1a1f"].aiAgentConfiguration.SELF_SERVICE.aiAgentId
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# Wisdom assistant AI Agent configuration reads return fabricated aiAgentIds

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAssistant

## Description
The assistant READ paths of the wisdom (Amazon Q in Connect) service do not serve the assistant's real AI Agent configuration: every read regenerates a random aiAgentId per AI Agent type instead of returning the account's built-in service-default agents or the agent the client configured with UpdateAssistantAIAgent. 2 causes, both in the wisdom service, both rooted in the same missing default-AI-Agent registry / unpersisted AI-Agent override behind the read handlers.

Cause 1 of 2: GetAssistant returns random per-call aiAgentIds instead of the configured/default agents.
AWS: GetAssistant answers 200 with AssistantData.aiAgentConfiguration populated with one entry per AIAgentType key (Smithy: AIAgentType values MANUAL_SEARCH, ANSWER_RECOMMENDATION, SELF_SERVICE, EMAIL_RESPONSE, EMAIL_OVERVIEW, EMAIL_GENERATIVE_ANSWER, ORCHESTRATION, NOTE_TAKING, CASE_SUMMARIZATION). Each value is an AIAgentConfigurationData whose aiAgentId names the agent CURRENTLY configured for that type: the customer agent previously assigned with UpdateAssistantAIAgent for the overridden type (the recording holds the fixture's own created-agent variable there), and the account's built-in service-default agent id together with its version qualifier for every type not overridden. Smithy: AIAgentConfigurationData.aiAgentId -> UuidWithQualifier, @required, @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}(:[A-Z0-9_$]+){0,1}$, doc 'The ID of the AI Agent to be configured'; AssistantData.aiAgentConfiguration -> AIAgentConfigurationMap, doc 'The configuration of the AI Agents (mapped by AI Agent Type to AI Agent version) that is set on the Amazon Q in Connect Assistant'. The requirement is that the default ids are SERVICE/ACCOUNT constants — the same id:version pair recurs for the same AI Agent type across independently recorded fixtures — and that an overridden type reports back exactly the agent id the client set, with the version qualifier it was pinned at (the corpus shows both forms: a published numeric qualifier and the $LATEST qualifier).
Emulator: answers 200 but fabricates a brand-new random UUID for every AI Agent type on every read, always with the qualifier ':1' — e.g. ANSWER_RECOMMENDATION 'd7852e87-b5ba-4a44-e36e-a041cd21a5c0:1', MANUAL_SEARCH '6defbe83-3d04-c30e-951d-fe7c75c32c86:1' — so the agent the fixture assigned to MANUAL_SEARCH with UpdateAssistantAIAgent is absent and no id matches the account defaults. Two reads of the same assistant disagree with each other.
Why: a client that reads aiAgentConfiguration and then calls GetAIAgent / ListAIAgentVersions / UpdateAssistantAIAgent with the returned aiAgentId addresses an agent that does not exist; read-after-write on UpdateAssistantAIAgent never confirms the assignment, so configuration workflows cannot verify or compare the agent configured for a type.
Fix: the GetAssistant handler and the assistant state it reads in the wisdom/qconnect service — the shared built-in (service-default) AI-Agent registry the assistant should be seeded from, plus persistence of the UpdateAssistantAIAgent override into that state. The serializer is fine: the map is emitted with the right keys and shape.
Traces: qconnect-assistant-ai-agent-configuration#task#4 (same cluster also covers qconnect-assistant-ai-agent-configuration#task#6 and qconnect-assistant-lifecycle-and-tags#task#2).

Cause 2 of 2: ListAssistants reports the same fabricated aiAgentIds in each summary.
AWS: ListAssistants answers 200 with assistantSummaries, each AssistantSummary carrying aiAgentConfiguration equal to what GetAssistant reports for that assistant — i.e. the account's built-in default agent id:version per AI Agent type, plus any configured override. Smithy: AssistantSummary.aiAgentConfiguration -> AIAgentConfigurationMap, doc 'The configuration of the AI Agents (mapped by AI Agent Type to AI Agent version) that is set on the Amazon Q in Connect Assistant'; ListAssistants is @readonly and @paginated over assistantSummaries, so a listed summary must agree with the single-resource read.
Emulator: answers 200 but the listed assistant's aiAgentConfiguration carries yet another set of freshly generated random UUIDs with ':1' qualifiers (e.g. ANSWER_RECOMMENDATION 'b589ecba-19f3-01dd-5d60-32176dfa04be:1', CASE_SUMMARIZATION '86358a65-3a77-de15-3978-db22d8292c63:1'), differing both from the account defaults and from what GetAssistant returned for the same assistant moments earlier.
Why: a client enumerating assistants cannot tell which assistants share the default agent set or which have been reconfigured, and every id it carries forward resolves to no AI Agent; list and get are mutually inconsistent for the same resource.
Fix: same assistant state as cause 1 — ListAssistants should project the stored aiAgentConfiguration rather than generating one per summary.
Traces: qconnect-assistant-lifecycle-and-tags#task#7.

Relation to open issues: '20261029T192454Z-wisdom-assistant-and-session-service-managed-defaults-missing.md' reports the same missing-default root cause on the CreateAssistant, CreateSession and GetSession clusters and owns those replay keys; it does not cover the GetAssistant or ListAssistants read paths, nor the unpersisted UpdateAssistantAIAgent override reported here, which the replay reports as unowned divergences.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, score 35.1%).

Cause 1 — qconnect-assistant-ai-agent-configuration#task#4, GetAssistant, expected 200 / actual 200, verdict mismatch (cluster also affects qconnect-assistant-ai-agent-configuration#task#6 and qconnect-assistant-lifecycle-and-tags#task#2):
  $.assistant.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId: expected '3025a3fa-5419-4c9c-a040-877213b93f70:$LATEST', got 'd7852e87-b5ba-4a44-e36e-a041cd21a5c0:1'
  $.assistant.aiAgentConfiguration.MANUAL_SEARCH.aiAgentId: expected the fixture's configured agent id (recorded as the variable '{{agent_id}}'), got '6defbe83-3d04-c30e-951d-fe7c75c32c86:1'
  $.assistant.aiAgentConfiguration.SELF_SERVICE / EMAIL_OVERVIEW / EMAIL_RESPONSE / EMAIL_GENERATIVE_ANSWER / NOTE_TAKING / CASE_SUMMARIZATION .aiAgentId: same shape of mismatch — a stable default id with a version qualifier expected, a fresh random UUID with ':1' returned.

Cause 2 — qconnect-assistant-lifecycle-and-tags#task#7, ListAssistants, expected 200 / actual 200, verdict mismatch:
  $.assistantSummaries[name="<probe assistant>"].aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId: expected '3025a3fa-5419-4c9c-a040-877213b93f70:$LATEST', got 'b589ecba-19f3-01dd-5d60-32176dfa04be:1'
  the remaining seven AI Agent types mismatch identically (MANUAL_SEARCH, SELF_SERVICE, EMAIL_OVERVIEW, EMAIL_RESPONSE, EMAIL_GENERATIVE_ANSWER, NOTE_TAKING, CASE_SUMMARIZATION).

The expected id for a given AI Agent type is byte-identical across unrelated fixtures while the emulator's differs on every single response, which is what identifies the defaults as service constants and the emulator's values as per-call fabrications. Only the id VALUES are quoted above as examples; the requirement is stability of the default set and fidelity to the configured override, not any one recorded UUID.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: create an assistant, assign an AI Agent to MANUAL_SEARCH via UpdateAssistantAIAgent, then GET /assistants/{assistantId} (GetAssistant) and read assistant.aiAgentConfiguration — MANUAL_SEARCH does not report the agent just configured, and every other AI Agent type reports a fresh random UUID with the qualifier ':1'. Repeat the GET and observe a different set again. Fixture qconnect-assistant-ai-agent-configuration step task#4 (also task#6; qconnect-assistant-lifecycle-and-tags step task#2).
3. Cause 2: GET /assistants (ListAssistants) and compare the summary's aiAgentConfiguration for that same assistant with what GetAssistant returned — the ids differ from the get, from each other call, and from the account's built-in default agent set. Fixture qconnect-assistant-lifecycle-and-tags step task#7.
