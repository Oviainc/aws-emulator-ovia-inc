Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-kbdeps-in-place-embedding-change-refusals
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-kbdeps-in-place-embedding-change-refusals
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|UpdateKnowledgeBase|4xx->4xx|$.message
Issue-Commit: d5d5092b5af2686a6a1d42f09a764e6dd1daf2a1

# bedrock-agent UpdateKnowledgeBase enum-value-set ValidationException lists the whole KnowledgeBaseType enum instead of the operation's accepted [VECTOR]

> _Note: this issue was identified against the code base at commit `d5d5092b5af2686a6a1d42f09a764e6dd1daf2a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateKnowledgeBase

## Description
Cause 1 of 1: UpdateKnowledgeBase's enum-constraint ValidationException renders its accepted-value list from the public KnowledgeBaseType enum shape rather than from the set of values the operation accepts, so the refusal advertises three types UpdateKnowledgeBase does not accept.

AWS: Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json): UpdateKnowledgeBase http {method: PUT, uri: /knowledgebases/{knowledgeBaseId}, code: 202}; knowledgeBaseConfiguration is required and KnowledgeBaseConfiguration.type is a required enum member targeting KnowledgeBaseType, whose declared values are VECTOR, KENDRA, SQL, MANAGED; the operation models ValidationException (httpError 400). The model does not say which subset this operation accepts, so the recording settles it. Recorded behavior: a request whose knowledgeBaseConfiguration.type is not a member of the accepted set is refused 400 ValidationException with AWS's standard counted-constraint template "1 validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy enum value set: [<accepted values>]". Fixed text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy enum value set: [" and "]". Variable parts taken from the request: <value> is the rejected value and <memberPath> the input member path (knowledgeBaseConfiguration.type). The accepted-value list is NOT a per-run value: for this member on UpdateKnowledgeBase the recorded list is exactly [VECTOR].
Emulator: the status (400) and the error code (ValidationException) match, but the message's accepted-value list is the full modeled enum: "1 validation error detected: Value 'managed' at 'knowledgeBaseConfiguration.type' failed to satisfy constraint: Member must satisfy enum value set: [VECTOR, KENDRA, SQL, MANAGED]" where AWS writes "... Member must satisfy enum value set: [VECTOR]". Only the bracketed set differs; the rejected value and member path are correct.
Why: within a generic ValidationException the message is the only machine-usable discriminator, so a client or deployment tool that matches AWS's sentence does not recognise the refusal. Worse, the emulator's list tells an operator that KENDRA, SQL or MANAGED would be accepted by UpdateKnowledgeBase, so they retry with a value the real service refuses the same way, and a tool that parses the bracketed set to offer valid choices offers three invalid ones.
Fix: the UpdateKnowledgeBase handler's input validation and the shared enum-value-set constraint-message builder in the bedrock-agent crate (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/update_knowledge_base.rs and the enum constraint-message helper it calls) — compose the accepted-value list from the values this operation accepts for knowledgeBaseConfiguration.type, not from the enum shape's declaration. Not the serializer: the error shape and status are already right.
Related open work that does NOT cover this (it does not carry this fixture step's replay key): 20261030T065936Z-bedrock-agent-validateflowdefinition-renders-the-flownodetype-enum-value.md records the ORDER of the FlowNodeType enum-value-set list on ValidateFlowDefinition; this cause is a different operation and a different defect (set MEMBERSHIP on UpdateKnowledgeBase).
Traces: bedrock-agent-kbdeps-in-place-embedding-change-refusals#task#6.

## Evidence
Fixture bedrock-agent-kbdeps-in-place-embedding-change-refusals step task#6, operation UpdateKnowledgeBase, HTTP 400 both sides, error code ValidationException both sides; $.message diverges:
Expected (AWS): "1 validation error detected: Value 'managed' at 'knowledgeBaseConfiguration.type' failed to satisfy constraint: Member must satisfy enum value set: [VECTOR]"
Actual (emulator): "1 validation error detected: Value 'managed' at 'knowledgeBaseConfiguration.type' failed to satisfy constraint: Member must satisfy enum value set: [VECTOR, KENDRA, SQL, MANAGED]"
Message anatomy: fixed text "1 validation error detected: Value '" / "' at '" / "' failed to satisfy constraint: Member must satisfy enum value set: [" / "]"; 'managed' is the request's rejected value and knowledgeBaseConfiguration.type the member path (both template slots, correct here); the bracketed accepted-value list is the requirement that diverges — [VECTOR] for this member on UpdateKnowledgeBase, not the four declared KnowledgeBaseType values.
Smithy cited: KnowledgeBaseType enum values [VECTOR, KENDRA, SQL, MANAGED]; KnowledgeBaseConfiguration.type required; UpdateKnowledgeBase errors include ValidationException httpError 400.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus and read the bedrock-agent cluster for fixture bedrock-agent-kbdeps-in-place-embedding-change-refusals, step task#6.
2. Create a VECTOR knowledge base (iam CreateRole trusting bedrock.amazonaws.com, bedrock-agent CreateKnowledgeBase with a vector configuration and storageConfiguration) and poll GetKnowledgeBase to ACTIVE.
3. Call UpdateKnowledgeBase(knowledgeBaseId=<kbId>, name=<same name>, roleArn=<same role>, knowledgeBaseConfiguration={'type':'managed', ...}) — any value outside the accepted set, e.g. the lowercase 'managed'.
4. Observe HTTP 400 ValidationException whose message ends "Member must satisfy enum value set: [VECTOR, KENDRA, SQL, MANAGED]" where AWS ends it "Member must satisfy enum value set: [VECTOR]".
