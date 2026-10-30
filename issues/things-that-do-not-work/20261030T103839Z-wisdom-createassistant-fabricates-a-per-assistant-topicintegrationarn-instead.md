Issue-Source: trace-sweep-divergence
Issue-Trace: wisdom-assistant-association-error-paths
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wisdom-assistant-association-error-paths
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom|CreateAssistant|2xx->2xx|$.assistant.integrationConfiguration.topicIntegrationArn
Issue-Commit: bd90440bd38277082eb102c17511325b543b3bd6

# Wisdom CreateAssistant fabricates a per-assistant topicIntegrationArn instead of the account's integrated SNS topic

> _Note: this issue was identified against the code base at commit `bd90440bd38277082eb102c17511325b543b3bd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssistant

## Description
CreateAssistant's response carries an assistant integration configuration the emulator invents per call instead of the Amazon SNS topic the account/assistant is actually integrated with, so the ARN it hands back addresses no topic that exists. 1 cause, in the wisdom service.

Cause 1 of 1: assistant.integrationConfiguration.topicIntegrationArn is a freshly minted, per-assistant SNS ARN.
AWS: CreateAssistant answers 200 with AssistantData.integrationConfiguration populated, whose topicIntegrationArn names the Amazon SNS topic used for streaming chat messages for that assistant (Smithy: com.amazonaws.wisdom#AssistantData.integrationConfiguration -> AssistantIntegrationConfiguration, doc 'The configuration information for the Wisdom assistant integration'; AssistantIntegrationConfiguration.topicIntegrationArn -> GenericArn, @length 1..2048, @pattern ^arn:[a-z-]+?:[a-z-]+?:[a-z0-9-]*?:([0-9]{12})?:[a-zA-Z0-9-:/]+$, doc 'The Amazon Resource Name (ARN) of the integrated Amazon SNS topic used for streaming chat messages'). CreateAssistantRequest has NO member for it (Smithy: input members are clientToken, name, type, description, tags, serverSideEncryptionConfiguration), so the value is service/account-supplied rather than client-supplied. Recorded behavior only for which topic that is: the corpus holds the expected value as the fixture-bound variable '{{topic_arn}}' — a topic ARN the fixture already knows independently of this response — so the requirement the recording states is that the returned ARN is the account's existing integrated chat-streaming topic (a stable, resolvable SNS topic), not a value minted by this call. The concrete recorded ARN is account-specific and is NOT itself the requirement.
Emulator: answers 200 but returns a brand-new ARN derived from the new assistant's own id, of the form 'arn:aws:sns:<region>:<account>:qconnect-assistant-<uuid>' (recorded example: arn:aws:sns:us-west-2:000000000000:qconnect-assistant-02e7e4da-b5c2-14aa-3435-6c6889388a47), i.e. a different topic name for every assistant created and one that the client never configured.
Why: topicIntegrationArn is the only handle a client gets for the assistant's chat-message stream. A client that takes the returned ARN and calls SNS Subscribe/Publish/GetTopicAttributes, or that compares it against the topic it integrated with Amazon Q in Connect, addresses a topic that does not exist in the emulator's SNS state; the integration can never be verified, and two assistants in the same account disagree about the streaming topic.
Fix: the CreateAssistant handler and the assistant state it builds in the wisdom service crate (services/service-com-amazonaws-wisdom-wisdomservice) — the integration-configuration seeding step, which should serve the account's integrated SNS topic (one account-level value, resolvable in the emulator's SNS state) rather than synthesizing a name from the assistant id. The serializer is fine: the member is emitted in the right place and matches the GenericArn pattern.
Traces: wisdom-assistant-association-error-paths#setup#1.

Relation to existing issues: the resolved issue '20261029T192454Z-wisdom-assistant-and-session-service-managed-defaults-missing.md' carried the replay key wisdom|CreateAssistant|2xx->2xx|$.assistant.integrationConfiguration.topicIntegrationArn in its front matter but described only the aiAgentConfiguration and SYSTEM-session causes; those were fixed (the aiAgentId keys no longer diverge) and this one was not, and because that issue is resolved it no longer owns this key. The open issue '20261029T194637Z-wisdom-assistant-ai-agent-configuration-reads-return-fabricated.md' covers fabricated aiAgentIds on the assistant READ paths only, not integrationConfiguration.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, score 72.5%, 10 divergence clusters).

Cause 1 — wisdom-assistant-association-error-paths#setup#1, CreateAssistant, expected 200 / actual 200, verdict mismatch (cluster size 1):
  $.assistant.integrationConfiguration.topicIntegrationArn: value_mismatch — expected the fixture-bound topic ARN '{{topic_arn}}' (the account's integrated chat-streaming topic, known to the fixture before this response), got 'arn:aws:sns:us-west-2:000000000000:qconnect-assistant-02e7e4da-b5c2-14aa-3435-6c6889388a47' (a name built from the newly created assistant's id).
  Status, x-amzn-errortype and every other member of $.assistant matched, including aiAgentConfiguration — so the only diverging member is the integration configuration.
The recorded ARN's region/account/topic-name are environment-specific; what the divergence establishes is the SHAPE of the defect: the emulator derives the topic from the per-call assistant id instead of serving an existing account-level SNS topic.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. POST /assistants (CreateAssistant) with a name and type AGENT, then read assistant.integrationConfiguration.topicIntegrationArn in the 200 body: it is 'arn:aws:sns:<region>:<account>:qconnect-assistant-<assistantId>', i.e. minted from the assistant just created. Fixture wisdom-assistant-association-error-paths step setup#1.
3. Repeat the CreateAssistant call: the returned topicIntegrationArn names a different SNS topic again, instead of the one account-level integrated chat-streaming topic.
4. Take the returned ARN to SNS (GetTopicAttributes/Subscribe) — no such topic exists, so the streaming integration the field advertises cannot be used or verified.
