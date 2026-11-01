Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-runtime-generate-query-text-to-sql
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-runtime-generate-query-text-to-sql bedrock-agent-runtime-invoke-inline-agent-invalid-model-id bedrock-agent-runtime-malformed-execution-identifier-validation-errors bedrock-agent-runtime-tag-operations-invalid-resource-arn
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime|GenerateQuery|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|InvokeInlineAgent|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|GetExecutionFlowSnapshot|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|ListFlowExecutionEvents|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|StopFlowExecution|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|TagResource|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|UntagResource|4xx->4xx|$.message
Issue-Key: bedrock-agent-runtime|ListTagsForResource|4xx->4xx|$.message
Issue-Commit: 7240e32f2500d20910730c71d4ad0714ff1f3217

# bedrock-agent-runtime ValidationException constraint messages quote the wrong pattern or the wrong member across eight operations

> _Note: this issue was identified against the code base at commit `7240e32f2500d20910730c71d4ad0714ff1f3217`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GenerateQuery

## Description
EIGHT net-new root causes, all reached through replayed bedrock-agent-runtime wire fixtures. All eight are the same fault class: the emulator DOES refuse the request with 400 ValidationException and the right Coral message frame, but the pattern it renders into that message — or, in one case, the member it validates at all — is not the service's. Smithy model consulted: services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json. The shared Coral frame is fixed text — `1 validation error detected: Value '` / `' at '` / `' failed to satisfy constraint: Member must satisfy regular expression pattern: ` — whose variable parts are only (a) the rejected value, (b) the dotted member path, both taken from the request, and (c) the member's pattern as the SERVICE renders it, with the outer `^`/`$` anchors stripped and no added grouping parentheses. Not covered by the open issue 20261029T171944Z-bedrock-agent-runtime-ten-net-new-causes-session.md: that behavioral issue reports the same rendering fault only for CreateSession.encryptionKeyArn and carries none of the replay keys below.

=== Cause 1 of 8: GenerateQuery's knowledgeBaseArn pattern omits the three iso partitions ===
AWS: Smithy: GenerateQuery is POST /generateQuery with required transformationConfiguration and models ValidationException @httpError 400; the offending member is transformationConfiguration.textToSqlConfiguration.knowledgeBaseConfiguration.knowledgeBaseArn -> KnowledgeBaseArn. Recorded behavior: a malformed value is refused 400 ValidationException whose quoted pattern is `arn:aws(|-cn|-us-gov|-iso|-iso-b|-iso-f):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:knowledge-base/[0-9a-zA-Z]+` — a six-way partition alternation. The on-disk model disagrees with the live service here: KnowledgeBaseArn @pattern is the narrower `^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:knowledge-base/[0-9a-zA-Z]+$`. The pattern the live service QUOTES is the requirement.
Emulator: identical frame, identical member path, but it echoes the on-disk model's narrower alternation `arn:aws(|-cn|-us-gov):bedrock:...` — `-iso`, `-iso-b` and `-iso-f` are missing.
Why: the refusal states a rule that is not the service's, so an iso-partition knowledge-base ARN is reported as violating a pattern AWS accepts, and any client or test asserting on the stated constraint diverges.
Fix: the GenerateQuery validation for knowledgeBaseArn in the bedrock-agent-runtime crate (the KnowledgeBaseArn constraint and the alternation rendered into the message).
Traces: bedrock-agent-runtime-generate-query-text-to-sql#task#2.

=== Cause 2 of 8: InvokeInlineAgent validates a member the operation does not even have, instead of foundationModel ===
AWS: Smithy: InvokeInlineAgent is POST /agents/{sessionId}; its required members include `foundationModel` -> ModelIdentifier @length(1,2048) with the model-identifier @pattern, and it has NO `agentId` member at all. It models ValidationException @httpError 400. Recorded behavior: a `foundationModel` that is not a model id, custom-model ARN, foundation-model ARN or inference-profile ARN is refused 400 ValidationException naming `foundationModel` as the failing member and quoting the ModelIdentifier pattern as the service renders it (the full alternation ending `...inference-profile/[a-zA-Z0-9-:.]+`). Variable parts: the rejected foundationModel value and the member name, both from the request.
Emulator: 400 ValidationException, but it names a DIFFERENT member — `agentId` — echoes the request's SESSION identifier as the rejected value, and quotes `^[0-9a-zA-Z]{10}$` (the InvokeAgent AgentId pattern, complete with both anchors AWS strips). So it is validating the wrong member of the wrong operation and never inspects foundationModel.
Why: the refusal points the caller at a member that does not exist on the request, with a value the caller did not supply for it, so the malformed field is never identified; a client fixing what the message names cannot make the call succeed, and a correct foundationModel-free request would be mis-diagnosed too.
Fix: the InvokeInlineAgent handler's input validation in the bedrock-agent-runtime crate — validate the operation's own members (foundationModel, instruction, sessionId) and render ModelIdentifier's pattern with outer anchors stripped.
Traces: bedrock-agent-runtime-invoke-inline-agent-invalid-model-id#task#1.

=== Cause 3 of 8: GetExecutionFlowSnapshot wraps FlowExecutionIdentifier's alternation in parentheses instead of stripping its outer anchors ===
AWS: Smithy: GetExecutionFlowSnapshot is GET /flows/{flowIdentifier}/aliases/{flowAliasIdentifier}/executions/{executionIdentifier}/flowsnapshot; executionIdentifier -> FlowExecutionIdentifier @length(max 2048) @pattern `^[a-zA-Z0-9-]{1,36}$|^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10}/alias/[0-9a-zA-Z]{10}/execution/[a-zA-Z0-9-]{1,36})$`; it models ValidationException @httpError 400. Recorded behavior: a malformed executionIdentifier is refused 400 ValidationException quoting the pattern with ONLY the outermost leading `^` and trailing `$` removed and nothing added: `[a-zA-Z0-9-]{1,36}$|^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10}/alias/[0-9a-zA-Z]{10}/execution/[a-zA-Z0-9-]{1,36})` — the interior `$` and `^` of the alternation stay.
Emulator: same frame and same member, but it wraps the whole alternation in a grouping pair the service never emits: `([a-zA-Z0-9-]{1,36}$|^(arn:...{1,36}))`.
Why: the quoted constraint is the only machine-readable statement of the rule in the refusal; a client (or a contract test) comparing it against the modeled pattern, or re-compiling it to pre-validate locally, gets a different regex than AWS states.
Fix: the shared ValidationException pattern-rendering helper used by the flow-execution operations in the bedrock-agent-runtime crate — strip the outer anchors only, never re-group.
Traces: bedrock-agent-runtime-malformed-execution-identifier-validation-errors#task#1.

=== Cause 4 of 8: ListFlowExecutionEvents renders the same executionIdentifier pattern wrongly ===
AWS: Smithy: ListFlowExecutionEvents is GET /flows/{flowIdentifier}/aliases/{flowAliasIdentifier}/executions/{executionIdentifier}/events with required eventType; executionIdentifier is the same FlowExecutionIdentifier shape, and the operation models ValidationException @httpError 400. Recorded behavior: the refusal quotes the pattern with the outer anchors stripped and no added grouping, exactly as in Cause 3.
Emulator: the parenthesised rendering `([a-zA-Z0-9-]{1,36}$|^(arn:...))`.
Why: as Cause 3 — on a second operation, so a fixer must confirm the rendering on each flow-execution path rather than only on the snapshot one.
Fix: same rendering helper, exercised from the ListFlowExecutionEvents handler's label validation.
Traces: bedrock-agent-runtime-malformed-execution-identifier-validation-errors#task#2.

=== Cause 5 of 8: StopFlowExecution renders the same executionIdentifier pattern wrongly ===
AWS: Smithy: StopFlowExecution is POST /flows/{flowIdentifier}/aliases/{flowAliasIdentifier}/executions/{executionIdentifier}/stop, executionIdentifier -> FlowExecutionIdentifier, models ValidationException @httpError 400. Recorded behavior: as Causes 3 and 4 — outer anchors stripped, nothing added.
Emulator: the parenthesised rendering.
Why: as above, on the mutating flow-execution path.
Fix: same rendering helper, exercised from the StopFlowExecution handler's label validation.
Traces: bedrock-agent-runtime-malformed-execution-identifier-validation-errors#task#3.

=== Cause 6 of 8: TagResource quotes the bedrock-agent CONTROL-PLANE ARN pattern instead of the runtime's taggable-resource pattern ===
AWS: Smithy: TagResource is POST /tags/{resourceArn}; resourceArn -> TaggableResourcesArn @length(20,1011), documented 'ARN of Taggable resources: [session]', and the operation models ValidationException @httpError 400. Recorded behavior: an ARN that is not a taggable runtime resource is refused 400 ValidationException quoting `arn:aws(-[^:]+)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}:(session|advanced-optimize-prompt-job)/[a-zA-Z0-9-]+` — a single, anchor-stripped alternative admitting exactly the two runtime resource kinds. The on-disk model is narrower still (session only, with a uuid body); the pattern the live service QUOTES is the requirement.
Emulator: 400 ValidationException with the right frame and member, but it quotes a four-branch pattern belonging to the bedrock-agent CONTROL plane — `(^arn:aws:bedrock:[a-zA-Z0-9-]+:/d{12}:(agent|agent-alias|knowledge-base|flow|prompt)/[A-Z0-9]{10}...|...retrieval-instruction/[a-z0-9]{10}$)` — complete with both anchors AWS strips and a broken account-id class `/d{12}` (a literal slash plus `d`, not `\d{12}`).
Why: the refusal advertises the wrong resource universe: it tells a caller that agent, knowledge-base, flow and prompt ARNs are taggable here (they are not) and that session ARNs are not (they are), so a client correcting its ARN against the stated rule still fails. The malformed `/d{12}` also means no account id can ever match the quoted regex.
Fix: the TagResource handler's resourceArn validation in the bedrock-agent-runtime crate — validate against the runtime's TaggableResourcesArn and render the service's own anchor-stripped pattern.
Traces: bedrock-agent-runtime-tag-operations-invalid-resource-arn#task#2.

=== Cause 7 of 8: UntagResource quotes the same wrong control-plane ARN pattern ===
AWS: Smithy: UntagResource is DELETE /tags/{resourceArn} with required tagKeys; resourceArn -> TaggableResourcesArn; models ValidationException @httpError 400. Recorded behavior: the same anchor-stripped `(session|advanced-optimize-prompt-job)` pattern as Cause 6.
Emulator: the same control-plane four-branch pattern with `/d{12}` and retained anchors.
Why: as Cause 6, on the untag path.
Fix: the UntagResource handler's resourceArn validation.
Traces: bedrock-agent-runtime-tag-operations-invalid-resource-arn#task#3.

=== Cause 8 of 8: ListTagsForResource quotes the same wrong control-plane ARN pattern ===
AWS: Smithy: ListTagsForResource is GET /tags/{resourceArn}, @readonly; resourceArn -> TaggableResourcesArn; models ValidationException @httpError 400. Recorded behavior: the same anchor-stripped `(session|advanced-optimize-prompt-job)` pattern as Cause 6.
Emulator: the same control-plane four-branch pattern.
Why: as Cause 6, on the read path.
Fix: the ListTagsForResource handler's resourceArn validation.
Traces: bedrock-agent-runtime-tag-operations-invalid-resource-arn#task#4.

## Evidence
All eight steps returned the expected 400 status; the divergence is the message body. Rejected values quoted below are the per-run values that FILL the template — the requirement is the frame plus the pattern.
Cause 1 — bedrock-agent-runtime-generate-query-text-to-sql#task#2: $.message expected "...Value 'not-a-valid-arn' at 'transformationConfiguration.textToSqlConfiguration.knowledgeBaseConfiguration.knowledgeBaseArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(|-cn|-us-gov|-iso|-iso-b|-iso-f):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:knowledge-base/[0-9a-zA-Z]+"; actual the same string with `arn:aws(|-cn|-us-gov):bedrock:...` — the three iso partitions absent.
Cause 2 — bedrock-agent-runtime-invoke-inline-agent-invalid-model-id#task#1: expected "...Value '???not-a-real-model-id???' at 'foundationModel' failed to satisfy constraint: Member must satisfy regular expression pattern: .*(^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:(([0-9]{12}:custom-model/...)|(:foundation-model/...))|(...)|(([0-9a-zA-Z][_-]?)+))$|(^arn:aws(|-us-gov|-cn|-iso|-iso-b):bedrock:(|[0-9a-z-]{1,20}):(|[0-9]{12}):inference-profile/[a-zA-Z0-9-:.]+)"; actual "...Value 'sim2real-probe-bedrock-agent-runtime-session-0a1f2a1d' at 'agentId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-zA-Z]{10}$" — a different member name, a different echoed value (the session id), and a different pattern with both anchors retained.
Cause 3 — bedrock-agent-runtime-malformed-execution-identifier-validation-errors#task#1 (GetExecutionFlowSnapshot): expected pattern text `[a-zA-Z0-9-]{1,36}$|^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10}/alias/[0-9a-zA-Z]{10}/execution/[a-zA-Z0-9-]{1,36})`; actual the same alternation wrapped as `([a-zA-Z0-9-]{1,36}$|^(arn:...{1,36}))`. Rejected value 'bad!exec!id', member 'executionIdentifier'.
Cause 4 — bedrock-agent-runtime-malformed-execution-identifier-validation-errors#task#2 (ListFlowExecutionEvents): identical expected/actual pair.
Cause 5 — bedrock-agent-runtime-malformed-execution-identifier-validation-errors#task#3 (StopFlowExecution): identical expected/actual pair.
Cause 6 — bedrock-agent-runtime-tag-operations-invalid-resource-arn#task#2 (TagResource): expected pattern `arn:aws(-[^:]+)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}:(session|advanced-optimize-prompt-job)/[a-zA-Z0-9-]+`; actual `(^arn:aws:bedrock:[a-zA-Z0-9-]+:/d{12}:(agent|agent-alias|knowledge-base|flow|prompt)/[A-Z0-9]{10}(?:/[A-Z0-9]{10})?$|^arn:aws:bedrock:[a-zA-Z0-9-]+:/d{12}:flow/([A-Z0-9]{10})/alias/([A-Z0-9]{10})$|^arn:aws:bedrock:[a-zA-Z0-9-]+:/d{12}:prompt/([A-Z0-9]{10})?(?::/d+)?$|^arn:aws(-cn|-us-gov|-eusc|-iso(-[b-f])?)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}:retrieval-instruction/[a-z0-9]{10}$)`. Member 'resourceArn'; rejected value an agent ARN.
Cause 7 — bedrock-agent-runtime-tag-operations-invalid-resource-arn#task#3 (UntagResource): identical expected/actual pair.
Cause 8 — bedrock-agent-runtime-tag-operations-invalid-resource-arn#task#4 (ListTagsForResource): identical expected/actual pair.

## Steps to Reproduce
Replay the wire corpus simparity-wire-aws-v1; each cause is one step below. In every case the emulator answers 400 ValidationException and the check is on the Message.
Cause 1: POST /generateQuery with transformationConfiguration.textToSqlConfiguration.knowledgeBaseConfiguration.knowledgeBaseArn set to a non-ARN string -> inspect the quoted partition alternation. (bedrock-agent-runtime-generate-query-text-to-sql#task#2)
Cause 2: POST /agents/{sessionId} (InvokeInlineAgent) with a valid sessionId, a >=40-character instruction and foundationModel set to a non-model string -> the message must name 'foundationModel'; the emulator names 'agentId'. (bedrock-agent-runtime-invoke-inline-agent-invalid-model-id#task#1)
Cause 3: GET /flows/<flowId>/aliases/<aliasId>/executions/bad!exec!id/flowsnapshot -> inspect the rendered executionIdentifier pattern. (bedrock-agent-runtime-malformed-execution-identifier-validation-errors#task#1)
Cause 4: GET /flows/<flowId>/aliases/<aliasId>/executions/bad!exec!id/events?eventType=<type> -> same check. (bedrock-agent-runtime-malformed-execution-identifier-validation-errors#task#2)
Cause 5: POST /flows/<flowId>/aliases/<aliasId>/executions/bad!exec!id/stop -> same check. (bedrock-agent-runtime-malformed-execution-identifier-validation-errors#task#3)
Cause 6: POST /tags/<url-encoded arn:aws:bedrock:<region>:<account>:agent/NOTASESSION> with tags -> inspect the quoted resourceArn pattern. (bedrock-agent-runtime-tag-operations-invalid-resource-arn#task#2)
Cause 7: DELETE /tags/<same ARN>?tagKeys=k -> same check. (bedrock-agent-runtime-tag-operations-invalid-resource-arn#task#3)
Cause 8: GET /tags/<same ARN> -> same check. (bedrock-agent-runtime-tag-operations-invalid-resource-arn#task#4)
