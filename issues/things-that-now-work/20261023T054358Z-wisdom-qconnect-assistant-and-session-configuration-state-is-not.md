Issue-Source: trace-sweep-divergence
Issue-Trace: qconnect-ai-guardrail-lifecycle
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: qconnect-ai-guardrail-lifecycle qconnect-assistant-association-lifecycle qconnect-assistant-lifecycle-and-tags qconnect-session-lifecycle qconnect-assistant-ai-agent-configuration
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.ANSWER_RECOMMENDATION.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.CASE_SUMMARIZATION.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.EMAIL_GENERATIVE_ANSWER.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.EMAIL_OVERVIEW.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.EMAIL_RESPONSE.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.MANUAL_SEARCH.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.NOTE_TAKING.aiAgentId
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.aiAgentConfiguration.SELF_SERVICE.aiAgentId
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
Issue-Key: wisdom|ListAssistants|2xx->2xx|$.assistantSummaries[name="sim2real-probe-qconnect-assistant-26ab1a1f"].tags.owner
Issue-Key: wisdom|CreateSession|2xx->2xx|$.session.aiAgentConfiguration
Issue-Key: wisdom|CreateSession|2xx->2xx|$.session.integrationConfiguration
Issue-Key: wisdom|CreateSession|2xx->2xx|$.session.orchestratorConfigurationList
Issue-Key: wisdom|GetSession|2xx->2xx|$.session.aiAgentConfiguration
Issue-Key: wisdom|GetSession|2xx->2xx|$.session.integrationConfiguration
Issue-Key: wisdom|GetSession|2xx->2xx|$.session.orchestratorConfigurationList
Issue-Commit: 1d15d4851343e3a31be69cd8cea36918aab666fe

# QConnect assistant and session configuration state is not persisted or serialized

> _Note: this issue was identified against the code base at commit `1d15d4851343e3a31be69cd8cea36918aab666fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssistant

## Description
This bundle carries 5 causes, all on service `wisdom` (the Q Connect / Wisdom endpoint, sigv4Name `wisdom`), all net-new: the replay report shows stub_path empty and owned_by empty for every one of them. They share one fault: the emulator does not hold the assistant's and session's CONFIGURATION state. The AI-agent configuration map, the resource tags and the session's integration/orchestrator members are either regenerated at serialization time from fresh random values or dropped from the response altogether, instead of being derived from the account's default AI agents and from what the create/update request stored.

Shared AWS grounding (cited once, applies to all 5 causes). Model: services/service-com-amazonaws-qconnect-wisdomservice/smithy-model/com-amazonaws-qconnect-wisdomservice.json (smithy_query query=operation on CreateAssistant/GetAssistant/ListAssistants/CreateSession/GetSession and query=shape on AssistantData/AssistantSummary/SessionData/AIAgentConfigurationMap/AIAgentConfigurationData/UuidWithQualifier). `AIAgentConfigurationMap` is a map of AIAgentType -> `AIAgentConfigurationData`, whose member `aiAgentId` is REQUIRED and targets `UuidWithQualifier` with pattern `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}(:[A-Z0-9_$]+){0,1}$` — a uuid optionally followed by a `:<VERSION>` qualifier (`:9`, `:$LATEST`). `aiAgentConfiguration` is a modeled member of `AssistantData`, `AssistantSummary` and `SessionData`; `tags` is a modeled member of all three; `SessionData` additionally models `integrationConfiguration` (SessionIntegrationConfiguration) and `orchestratorConfigurationList`. The model does not state WHICH ids AWS populates, so for the concrete values this is recorded behavior only: the recordings show AWS returning, per AI-agent type, the account's default system AI agent as uuid PLUS an explicit version qualifier, and returning the SAME value on every later read of the same resource; where the fixture itself configured an agent, AWS echoes exactly the configured id. The requirement asserted here is therefore (a) that `aiAgentId` carries a version qualifier as the `UuidWithQualifier` shape allows, (b) that the value is STABLE across CreateAssistant -> GetAssistant -> ListAssistants for one assistant and equals what was configured, and (c) that the modeled members are present. Individual recorded uuids, ARNs and tag values are examples, not requirements.

Shared Emulator behavior: every response that embeds an AI-agent configuration emits a newly minted bare uuid with no `:<qualifier>` for each AI-agent type, differing in each response for the same assistant; the session shapes omit the configuration members entirely.

Shared Fix: the assistant/session state model plus the AssistantData/AssistantSummary/SessionData serializers — the AI-agent configuration must be seeded once per account/assistant (with version qualifiers) and then stored and read back, and CreateSession/GetSession must project the stored assistant configuration, the session integration configuration and the orchestrator list into their response shapes. It is not a per-handler value fix: four handlers read the same missing state.

Relation to open issues: 20261022T141946Z-wisdom-model-operations-refused-500-ambiguousservice-delete-list.md covers the router's AmbiguousService refusal on the Wisdom-model fixture family and names none of the steps below in its replay pointer; routing now succeeds for these steps (they return 200/400), so none of these 5 causes is covered by it.

Cause 1 of 5 — CreateAssistant (POST /assistants -> 200, output `assistant`: AssistantData). AWS: returns `assistant.aiAgentConfiguration` with one entry per configured AI-agent type whose `aiAgentId` is uuid+`:<version>` (recording: MANUAL_SEARCH '652f0287-1610-4fee-bbe3-c687c13f5ec3:9', SELF_SERVICE '529207fe-80fd-42be-bcf2-facd2141aec7:5', EMAIL_OVERVIEW '...:5', EMAIL_RESPONSE '...:7', EMAIL_GENERATIVE_ANSWER '...:7', NOTE_TAKING '...:3', CASE_SUMMARIZATION '...:3' — examples; ANSWER_RECOMMENDATION is shape-asserted only). Emulator: 200 with the right map keys but a random bare uuid per type and no version qualifier (e.g. MANUAL_SEARCH 'b33cae15-1579-272b-dea3-3648804c3a2e'), and the shape-only assertion on ANSWER_RECOMMENDATION.aiAgentId also fails. Why: a client that reads the configured agent version off CreateAssistant and then invokes or updates that agent passes an id that exists nowhere, and cannot pin a version at all. Fix: CreateAssistant handler + assistant state seeding. Traces: qconnect-ai-guardrail-lifecycle#setup#1 (cluster also covers qconnect-assistant-association-lifecycle#setup#1, qconnect-assistant-lifecycle-and-tags#task#1, qconnect-session-lifecycle#setup#1).

Cause 2 of 5 — GetAssistant (GET /assistants/{assistantId} -> 200). AWS: echoes the stored configuration, so a read after create/update returns the SAME ids, including an id the fixture itself created and configured (the recording asserts MANUAL_SEARCH equals the fixture's own agent id variable, and ANSWER_RECOMMENDATION '3025a3fa-5419-4c9c-a040-877213b93f70:$LATEST'). Emulator: 200 with yet another set of random bare uuids, unrelated both to what CreateAssistant returned and to the agent the fixture configured. Why: configuration writes appear to succeed but are invisible to reads, so any configure-then-verify flow (the normal way to manage AI agents) can never converge. Fix: GetAssistant handler / assistant state read path. Traces: qconnect-assistant-ai-agent-configuration#task#4 (cluster also covers qconnect-assistant-ai-agent-configuration#task#6, qconnect-assistant-lifecycle-and-tags#task#2).

Cause 3 of 5 — ListAssistants (GET /assistants -> 200, required `assistantSummaries`). AWS: each `AssistantSummary` repeats the assistant's `aiAgentConfiguration` (same qualified ids as the detail read) and its `tags` as supplied at create time (recording: tag key 'owner' = 'qconnect-test' — value is an example; the requirement is that a tag set on create appears on the summary). Emulator: 200, but the summary's aiAgentConfiguration again carries fresh random bare uuids, and `tags.owner` is missing entirely from the summary. Why: listing is the discovery path — clients cannot filter/select assistants by tag, and see a third disagreeing set of agent ids. Fix: ListAssistants handler / AssistantSummary projection (must read tags and configuration from stored assistant state). Traces: qconnect-assistant-lifecycle-and-tags#task#7.

Cause 4 of 5 — CreateSession (POST /assistants/{assistantId}/sessions -> 200, output `session`: SessionData). AWS: the created session carries `aiAgentConfiguration` (inherited from the assistant, including an `enabled: false` entry for ANSWER_RECOMMENDATION in the recording), `integrationConfiguration` with `topicIntegrationArn` (an SNS topic ARN — the recorded ARN is account/region specific and is an example; the requirement is the member's presence), and `orchestratorConfigurationList` present as an empty list. Emulator: 200 but all three members are absent from `$.session`. Why: a client reading the session's AI-agent configuration or subscribing to the session's integration topic gets nothing to work with, and SDK code treating the empty-but-present orchestrator list as the no-op case cannot distinguish it from an unsupported field. Fix: CreateSession handler / SessionData serializer and session state. Traces: qconnect-session-lifecycle#task#2.

Cause 5 of 5 — GetSession (GET /assistants/{assistantId}/sessions/{sessionId} -> 200). AWS: the read returns the same fully populated SessionData as the create (aiAgentConfiguration, integrationConfiguration.topicIntegrationArn, orchestratorConfigurationList). Emulator: 200 with the same three members missing. Why: the session read, which is what long-running contact flows poll, never exposes the session's configuration at all. Fix: GetSession handler / SessionData serializer. Traces: qconnect-session-lifecycle#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline: emulator_up=true ('/__admin/health answered 200 and the :33037 data listener accepts'), bootstrap_signal=false, state_reset_failures=0, fixtures_replayed=12, fixtures_replay_error=0; steps_total=114, passed=26, mismatched=42, blocked=46, score_pct=22.8, divergence_count=6 — all 6 on service `wisdom`, verdict=mismatch, stub_path empty, owned_by empty. The 5 clusters bundled here:

1) qconnect-ai-guardrail-lifecycle#setup#1 CreateAssistant, expected 200 -> got 200, divergence_key service=wisdom|CreateAssistant|status=200->200|keys=$.assistant.aiAgentConfiguration.*.aiAgentId (8 keys), affected_count=4. Examples: $.assistant.aiAgentConfiguration.MANUAL_SEARCH.aiAgentId expected '652f0287-1610-4fee-bbe3-c687c13f5ec3:9', got '4c8215b6-4855-9041-8b51-6c795d8194cc'; SELF_SERVICE expected '529207fe-80fd-42be-bcf2-facd2141aec7:5', got '82f580c3-8901-8f60-879a-bbff5155d358'; EMAIL_RESPONSE expected '8f4e7d2a-9c31-4b5e-a6f8-e2d9c7b54321:7', got 'd8201497-4d49-c5d4-5fe0-a4d5ad684a86'; CASE_SUMMARIZATION expected '9114812c-2a1e-4501-8817-daec4223ce57:3', got 'c8b5f613-aa54-6604-4a0e-605355c0b6f2'; ANSWER_RECOMMENDATION expected '<shaped-not-asserted>', got 'b33cae15-1579-272b-dea3-3648804c3a2e'. Every observed value is a bare uuid with no ':<version>' qualifier.

2) qconnect-assistant-ai-agent-configuration#task#4 GetAssistant, expected 200 -> got 200, divergence_key service=wisdom|GetAssistant|status=200->200|keys=$.assistant.aiAgentConfiguration.*.aiAgentId (8 keys), affected_count=3. ANSWER_RECOMMENDATION expected '3025a3fa-5419-4c9c-a040-877213b93f70:$LATEST', got 'bc55ac1e-d5db-8a1b-1115-b373a477fac3'; MANUAL_SEARCH expected the fixture's own configured agent id ('{{agent_id}}'), got '7d73b1a5-cafe-166a-054a-a106c923a0aa'. The observed ids also differ from those returned by CreateAssistant for the same assistant, i.e. they are regenerated per response rather than stored.

3) qconnect-assistant-lifecycle-and-tags#task#7 ListAssistants, expected 200 -> got 200, divergence_key service=wisdom|ListAssistants|status=200->200|keys=$.assistantSummaries[name="sim2real-probe-qconnect-assistant-26ab1a1f"].aiAgentConfiguration.*.aiAgentId (8 keys) + .tags.owner, affected_count=1. All eight aiAgentId values are fresh bare uuids (e.g. ANSWER_RECOMMENDATION expected '3025a3fa-5419-4c9c-a040-877213b93f70:$LATEST', got 'dc8d29e9-1fc6-c3f5-3caf-128ebe80a640'), and `tags.owner` is reported missing (expected 'qconnect-test', got None).

4) qconnect-session-lifecycle#task#2 CreateSession, expected 200 -> got 200, divergence_key service=wisdom|CreateSession|status=200->200|keys=$.session.aiAgentConfiguration,$.session.integrationConfiguration,$.session.orchestratorConfigurationList, affected_count=1. All three reported missing (got None): aiAgentConfiguration expected an 8-entry map (ANSWER_RECOMMENDATION {'aiAgentId': '3025a3fa-...:$LATEST', 'enabled': False}, MANUAL_SEARCH '652f0287-...:9', ...), integrationConfiguration expected {'topicIntegrationArn': 'arn:aws:sns:us-west-2:000000000000:qconnect-assistant-954f1103-18da-3693-cdad-5cbf71993214'} (ARN is an example; presence is the requirement), orchestratorConfigurationList expected [].

5) qconnect-session-lifecycle#task#3 GetSession, expected 200 -> got 200, divergence_key service=wisdom|GetSession|status=200->200|keys=$.session.aiAgentConfiguration,$.session.integrationConfiguration,$.session.orchestratorConfigurationList, affected_count=1 — identical three missing members on the read path.

The service is stood up in this checkout (resolved bootstrap issues for service-com-amazonaws-wisdom-wisdomservice and service-com-amazonaws-qconnect-wisdomservice), so these are fidelity defects on a bootstrapped service.

## Steps to Reproduce
1. Clone mainline and build the emulator (both wisdom/qconnect service crates are registered; protocol restJson1, signing name `wisdom`).
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Representative step: fixture qconnect-ai-guardrail-lifecycle, step setup#1 — POST /assistants (CreateAssistant). Expect `assistant.aiAgentConfiguration.<TYPE>.aiAgentId` to be a stable uuid carrying a ':<VERSION>' qualifier for every type; observe freshly generated bare uuids.
4. Re-replay each other bundled cause and check it independently: qconnect-assistant-ai-agent-configuration#task#4 (GetAssistant must echo the configured/stored agent ids, including the agent the fixture created), qconnect-assistant-lifecycle-and-tags#task#7 (ListAssistants summaries must carry the same configuration and the `owner` tag set at create time), qconnect-session-lifecycle#task#2 (CreateSession response must contain session.aiAgentConfiguration, session.integrationConfiguration.topicIntegrationArn and session.orchestratorConfigurationList), qconnect-session-lifecycle#task#3 (GetSession must return the same three members).
5. Equivalently with boto3 `qconnect`: create_assistant(...), note assistant['aiAgentConfiguration'], then get_assistant(assistantId=...) and list_assistants() — the aiAgentId values differ in all three responses and never carry a version qualifier; create_session(...)/get_session(...) return a `session` with no aiAgentConfiguration, integrationConfiguration or orchestratorConfigurationList.
