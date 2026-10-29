Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime.create-invocation-duplicate-id-rejected bedrock-agent-runtime.duplicate-invocation-id-rejected bedrock-agent-runtime.create-session-invalid-encryption-key-rejected bedrock-agent-runtime.end-already-ended-session-rejected bedrock-agent-runtime.end-session-already-ended-rejected bedrock-agent-runtime.end-session-twice-rejected bedrock-agent-runtime.get-invocation-step-wrong-invocation-scope-rejected bedrock-agent-runtime.get-invocation-step-wrong-session-scope-rejected bedrock-agent-runtime.put-invocation-step-conflicting-content-rejected bedrock-agent-runtime.put-invocation-step-duplicate-id-conflicting-payload-rejected bedrock-agent-runtime.put-invocation-step-idempotent-same-content bedrock-agent-runtime.list-sessions-invalid-next-token-rejected bedrock-agent-runtime.tag-resource-unknown-arn-not-found bedrock-agent-runtime.rerank-scoring-consistency-across-calls bedrock-agent-runtime.deploy-agent-alias-invoke-behaves
Issue-Shape-Id: com.amazonaws.bedrockagentruntime#AmazonBedrockAgentRunTimeService
Issue-Key: bedrock-agent-runtime.create-invocation-duplicate-id-rejected|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: bedrock-agent-runtime.duplicate-invocation-id-rejected|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: bedrock-agent-runtime|CreateSession|ValidationException
Issue-Key: bedrock-agent-runtime.end-already-ended-session-rejected|run#10|AssertionError|assert message == "Cannot end a terminated session", (
Issue-Key: bedrock-agent-runtime.end-session-already-ended-rejected|run#8|AssertionError|assert message == "Cannot end a terminated session", (
Issue-Key: bedrock-agent-runtime.end-session-twice-rejected|run#9|AssertionError|assert message == "Cannot end a terminated session", (
Issue-Key: bedrock-agent-runtime.get-invocation-step-wrong-invocation-scope-rejected|run#11|AssertionError|assert message == expected_message, (
Issue-Key: bedrock-agent-runtime.get-invocation-step-wrong-session-scope-rejected|run#11|AssertionError|assert message == expected_message, (
Issue-Key: bedrock-agent-runtime.put-invocation-step-conflicting-content-rejected|run#16|AssertionError|assert message == expected_message, (
Issue-Key: bedrock-agent-runtime.put-invocation-step-duplicate-id-conflicting-payload-rejected|run#15|AssertionError|assert message == expected_message, (
Issue-Key: bedrock-agent-runtime.put-invocation-step-idempotent-same-content|_error#11|AssertionError|raise AssertionError(
Issue-Key: bedrock-agent-runtime.list-sessions-invalid-next-token-rejected|run#8|AssertionError|assert message == "Next token for pagination is invalid", (
Issue-Key: bedrock-agent-runtime.tag-resource-unknown-arn-not-found|run#13|AssertionError|assert message == expected_message, (
Issue-Key: bedrock-agent-runtime.rerank-scoring-consistency-across-calls|run#11|AssertionError|assert first_order[0] in (0, 2), f"top-ranked result was not relevant: {first}"
Issue-Key: bedrock-agent-runtime|CreateStack|ValidationError
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# bedrock-agent-runtime: ten net-new causes — session/invocation refusal messages are all re-worded, PutInvocationStep accepts an identical-content replay, Rerank is not relevance-driven, and CloudFormation does not know AWS::Bedrock::Flow

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutInvocationStep

## Description
TEN distinct net-new root causes, all reached through bedrock-agent-runtime behavioral checks. Smithy model consulted: services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json. None of these are covered by the open issue 20261023T004328Z-bedrock-agent-runtime-retrieve-optimizeprompt-getagentmemory-deleteagentmemory-and.md, which is about operations that answer NotImplemented — every operation below DOES answer, and answers wrongly.

=== Cause 1 of 10: CreateInvocation's duplicate-invocationId ConflictException is re-worded ===
AWS: Smithy: CreateInvocation is PUT /sessions/{sessionIdentifier}/invocations/ (success 201) and models ConflictException @httpError 409. Recorded behavior: re-creating an invocationId that already exists in the session is refused 409 ConflictException with Message `Invocation with ID <invocationId> already exists`. Template: the fixed text is `Invocation with ID ` ... ` already exists`; the only variable part is <invocationId> taken from the request. The message carries NO session identifier and NO trailing period.
Emulator: 409 ConflictException with `Invocation <invocationId> already exists in session <sessionId>.` — different fixed text, an extra state-derived session id, and a trailing period.
Why: a client that matches on the refusal text (or logs/alerts on it) to tell a duplicate-invocation replay apart from other 409s never matches, and the message leaks the session id AWS deliberately keeps out of it.
Fix: the CreateInvocation handler's conflict branch in the bedrock-agent-runtime crate (src/ops/create_invocation.rs) — word the ConflictException as AWS does.
Traces: bedrock-agent-runtime.create-invocation-duplicate-id-rejected, bedrock-agent-runtime.duplicate-invocation-id-rejected.

=== Cause 2 of 10: CreateSession's encryptionKeyArn ValidationException quotes a KMS ARN pattern missing the three iso partitions ===
AWS: Recorded behavior: CreateSession with a malformed encryptionKeyArn is refused 400 ValidationException with Message `1 validation error detected: Value '<value>' at 'encryptionKeyArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(|-cn|-us-gov|-iso|-iso-b|-iso-f):kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}`. Fixed text: the `1 validation error detected: Value '` / `' at '` / `' failed to satisfy constraint: Member must satisfy regular expression pattern: ` frame plus the quoted pattern; variable parts: the rejected value and the member name, both from the request. Smithy on disk disagrees with the live service here: CreateSession.encryptionKeyArn -> KmsKeyArn carries @pattern `^arn:aws(|-cn|-us-gov):kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$` and @length(1,2048) — the narrower, three-partition-short spelling. The pattern the live service QUOTES in the refusal is the requirement.
Emulator: same frame but quotes the on-disk model's narrower alternation: `arn:aws(|-cn|-us-gov):kms:...`, i.e. `-iso`, `-iso-b` and `-iso-f` are missing.
Why: the refusal tells a client a wrong rule — an iso-partition CMK ARN is reported as not matching a pattern the real service accepts, and any client or test asserting on the stated constraint diverges.
Fix: the CreateSession validation for encryptionKeyArn (src/ops/create_session.rs / the crate's KmsKeyArn constraint), including the partition alternation rendered in the message.
Traces: bedrock-agent-runtime.create-session-invalid-encryption-key-rejected.

=== Cause 3 of 10: EndSession's already-ended ConflictException is re-worded and leaks session state ===
AWS: Smithy: EndSession is PATCH /sessions/{sessionIdentifier} and models ConflictException @httpError 409. Recorded behavior: ending a session that is already terminated is refused 409 ConflictException with Message `Cannot end a terminated session` — the message is ENTIRELY fixed text, with no variable part at all: no session id, no status name, no trailing period.
Emulator: `Session <sessionId> has status ENDED and cannot be ended.` — wholly different wording, and it interpolates both the session id and the stored status.
Why: the refusal for a double-EndSession is unrecognisable to a client matching AWS's text, and the emulator discloses stored status in a message AWS keeps opaque.
Fix: the EndSession handler's terminal-state conflict branch (src/ops/end_session.rs).
Traces: bedrock-agent-runtime.end-already-ended-session-rejected, bedrock-agent-runtime.end-session-already-ended-rejected, bedrock-agent-runtime.end-session-twice-rejected.

=== Cause 4 of 10: GetInvocationStep's not-found message is re-worded AND splits one AWS refusal into two different messages ===
AWS: Smithy: GetInvocationStep is POST /sessions/{sessionIdentifier}/invocationSteps/{invocationStepId} with required invocationIdentifier, and models ResourceNotFoundException @httpError 404. Recorded behavior: BOTH miss shapes — a step id that exists but under a different invocation, and an invocation that exists but under a different session — are refused with the SAME 404 Message: `Combination of Invocation with ID <invocationIdentifier> and Invcoation Step with ID <invocationStepId> not found`. Fixed text: `Combination of Invocation with ID ` / ` and Invcoation Step with ID ` / ` not found` — note AWS's own misspelling `Invcoation`, which is part of the fixed text and must be reproduced verbatim. Variable parts: the two ids, both from the request. AWS deliberately does not reveal WHICH of the two scopes missed.
Emulator: two distinct messages, neither AWS's: `Invocation step <stepId> was not found in invocation <invocationId>.` for the wrong-invocation scope, and `Invocation <invocationId> was not found in session <sessionId>.` for the wrong-session scope.
Why: beyond not matching AWS's text, the emulator turns one opaque refusal into a scope oracle — a caller can enumerate which ids exist under which parent, which the real service does not permit.
Fix: the GetInvocationStep handler's lookup/not-found branch (src/ops/get_invocation_step.rs) — collapse both miss shapes onto the single AWS-worded refusal.
Traces: bedrock-agent-runtime.get-invocation-step-wrong-invocation-scope-rejected, bedrock-agent-runtime.get-invocation-step-wrong-session-scope-rejected.

=== Cause 5 of 10: PutInvocationStep's duplicate-step ConflictException is re-worded ===
AWS: Smithy: PutInvocationStep is PUT /sessions/{sessionIdentifier}/invocationSteps/ (success 201) and models ConflictException @httpError 409. Recorded behavior: a second PutInvocationStep reusing an invocationStepId already stored under that invocation is refused 409 ConflictException with Message `Combination of Invocation with ID <invocationIdentifier> and Invcoation Step with ID <invocationStepId> already exists` — same fixed frame (and same `Invcoation` misspelling) as Cause 4, ending in ` already exists`; variable parts are the two request ids.
Emulator: `Invocation step <stepId> already exists in invocation <invocationId> with different content.` — different fixed text, and it additionally asserts a content comparison AWS's message never mentions.
Fix: the PutInvocationStep handler's conflict branch (src/ops/put_invocation_step.rs).
Traces: bedrock-agent-runtime.put-invocation-step-conflicting-content-rejected, bedrock-agent-runtime.put-invocation-step-duplicate-id-conflicting-payload-rejected.

=== Cause 6 of 10: PutInvocationStep ACCEPTS a replayed invocationStepId when the payload is byte-identical, instead of refusing it ===
AWS: Smithy: PutInvocationStep models ConflictException @httpError 409; invocationStepId is an optional client-supplied Uuid. Recorded behavior: invocationStepId is a WRITE-ONCE key per invocation — a retry carrying the same invocationStepId is refused 409 ConflictException EVEN WHEN the payload is byte-for-byte identical to the stored step (the service does not special-case identical-content retries as idempotent no-ops), and exactly one step remains stored. A retry with a fresh invocationStepId succeeds and yields two stored steps.
Emulator: the identical-content retry SUCCEEDS — HTTP 201, body `{"invocationStepId": "<same id>"}` — so the write-once key is enforced only when the content differs (see Cause 5).
Why: this is an overly-permissive accept on a conflict path. A client that relies on the 409 to detect a double-submit gets a success, and the emulator's conflict rule (content-sensitive) is not the service's rule (id-sensitive), so emulator-passing code breaks on AWS the moment a retry is genuinely identical.
Fix: the PutInvocationStep handler's duplicate-key guard (src/ops/put_invocation_step.rs) — key the conflict on (invocationIdentifier, invocationStepId) alone, with no payload comparison.
Traces: bedrock-agent-runtime.put-invocation-step-idempotent-same-content.

=== Cause 7 of 10: ListSessions' invalid-nextToken ValidationException is re-worded ===
AWS: Smithy: ListSessions is POST /sessions/, @paginated(inputToken: nextToken), nextToken @length(1,2048) @pattern ^\\S*$, and models ValidationException @httpError 400. Recorded behavior: a well-formed but never-issued nextToken is refused 400 ValidationException with Message `Next token for pagination is invalid` — entirely fixed text, no variable part, and the offending token is NOT echoed.
Emulator: `nextToken is not a pagination token this service issued for this account's sessions` — wholly emulator-authored wording.
Fix: the ListSessions handler's token-decode failure branch (src/ops/list_sessions.rs).
Traces: bedrock-agent-runtime.list-sessions-invalid-next-token-rejected.

=== Cause 8 of 10: TagResource's unknown-session ResourceNotFoundException is re-worded and echoes the ARN ===
AWS: Smithy: TagResource is POST /tags/{resourceArn}; resourceArn -> TaggableResourcesArn @length(20,1011) with the session-only pattern `(^arn:aws(-[^:]+)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}:(session)/<uuid>$)`; it models ResourceNotFoundException @httpError 404. Recorded behavior: tagging a well-formed session ARN that names no existing session is refused 404 ResourceNotFoundException with Message `Session resource not found.` — entirely fixed text (with the trailing period), and the ARN is NOT echoed.
Emulator: `Resource <resourceArn> was not found. Check the Amazon Resource Name (ARN) and try your request again.` — generic cross-service wording that names the resource kind nowhere and echoes the whole request ARN.
Fix: the TagResource handler's resource-resolution branch (src/ops/tag_resource.rs) — raise the session-specific, fixed-text refusal.
Traces: bedrock-agent-runtime.tag-resource-unknown-arn-not-found.

=== Cause 9 of 10: Rerank's relevanceScore is not relevance-driven, so the top-ranked source is the wrong document ===
AWS: Smithy: Rerank is POST /rerank with required queries (@length 1..1), sources (@length 1..1000) and rerankingConfiguration, returning a required `results` list of (index, relevanceScore). Recorded behavior: reranking the query `What is the capital of France?` over five inline text sources with amazon.rerank-v1:0 puts a France/Paris document first — i.e. the ordering is a semantic relevance ranking over query and document text, deterministic across identical calls.
Emulator: the ranking is deterministic (that part passes) but is NOT relevance-driven. For that query it returns, in order: index 1 (`Berlin is the capital of Germany.`) 0.6666666865348816, index 4 (`Madrid is the capital of Spain.`) 0.6666666865348816, index 0 (`Paris is the capital and most populous city of France.`) 0.64549720287323, index 2 (`The Eiffel Tower is located in Paris, France.`) 0.4330126941204071, index 3 (`Bananas are a good source of potassium.`) 0.15430335700511932 — the two documents about OTHER countries' capitals outrank both France documents, and they tie exactly. Hypothesis: the score is a lexical/token-overlap similarity (the sources that share the most surface tokens with `capital of` win) rather than a semantic one, and exact ties are left unbroken.
Why: Rerank's entire purpose is the ordering. A client that reranks retrieved passages and takes the top result gets the wrong passage, and any RAG pipeline built against the emulator grounds its answer in an irrelevant document.
Fix: the Rerank handler's scoring function in the bedrock-agent-runtime crate (src/ops/rerank.rs) — the relevanceScore computation must rank the query-relevant source first and break ties deterministically rather than emitting equal scores.
Traces: bedrock-agent-runtime.rerank-scoring-consistency-across-calls.

=== Cause 10 of 10: CloudFormation refuses the resource type AWS::Bedrock::Flow, so no Bedrock flow can be provisioned through a stack ===
AWS: recorded behavior only (this is CloudFormation's template-type registry, not a bedrock-agent-runtime model fact): a stack template declaring `AWS::Bedrock::Flow` (Name, ExecutionRoleArn, Definition with Input/Output nodes) alongside an AWS::IAM::Role is ACCEPTED by CreateStack and provisions a flow that is then routable through the bedrock-agent-runtime InvokeFlow data plane.
Emulator: CreateStack is refused 400 ValidationError with `Template format error: Unrecognized resource types: [AWS::Bedrock::Flow]` — the type is not in the emulator's recognised-resource-type set at all, so the template is rejected before any resource is created.
Why: the deploy-through-CloudFormation-then-invoke-through-the-runtime contract cannot be exercised for Bedrock at all; a client's stack fails at validation rather than provisioning.
Fix: register AWS::Bedrock::Flow in the CloudFormation resource-type registry and give it a provisioner that calls the bedrock-agent control plane (the emulator's cloudformation crate resource-type table).
Traces: bedrock-agent-runtime.deploy-agent-alias-invoke-behaves.

## Evidence
Cause 1 — expected Message 'Invocation with ID 844047b8-b967-444d-b306-702f5e66e3a7 already exists'; actual 'Invocation 844047b8-b967-444d-b306-702f5e66e3a7 already exists in session 49dd7b9c-90dc-4546-9a45-8d4f8f26c073.' (code ConflictException matched). Second occurrence with id ca4404ec-5811-454c-a109-98fdfe18ffbe / session cb2b257c-3e79-4e53-ae15-c85a68441039. The ids are per-run values filling the template; the divergence is the fixed text and the extra session clause.
Cause 2 — expected "1 validation error detected: Value 'not-a-valid-kms-arn' at 'encryptionKeyArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(|-cn|-us-gov|-iso|-iso-b|-iso-f):kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}"; actual the same string with the alternation 'arn:aws(|-cn|-us-gov)' — '-iso', '-iso-b', '-iso-f' absent. On-disk Smithy KmsKeyArn @pattern is the narrower spelling the emulator echoes.
Cause 3 — expected Message 'Cannot end a terminated session'; actual 'Session c2d04788-322a-403c-9b9d-b2e53c7cc7f7 has status ENDED and cannot be ended.' (also seen for sessions 83fd40d0-5475-4e85-bfee-5fe85006c537 and 41ab189c-eec7-4197-a415-17754b3992ba). AWS's message has no variable part at all.
Cause 4 — expected Message 'Combination of Invocation with ID <invId> and Invcoation Step with ID <stepId> not found' for BOTH scopes; actual 'Invocation step e4d4f544-5b10-4c19-999f-2be986f09f58 was not found in invocation 96c463a6-b472-4ab9-850a-8d1a0a327b12.' (wrong-invocation scope) and 'Invocation 675e9604-743e-43be-8cd6-7bde545d879e was not found in session cdfffe6d-5938-4cac-8cb7-4eb458892319.' (wrong-session scope).
Cause 5 — expected Message 'Combination of Invocation with ID <invId> and Invcoation Step with ID <stepId> already exists'; actual 'Invocation step ca2f876e-c52d-4441-8953-816168a15a5b already exists in invocation ae74be16-6f83-4581-b6f0-87883c62889b with different content.' (also 6feade16-19ec-4323-9678-fda3a35d5a88 / 26342aad-ce05-46f7-9b94-97b098e23639).
Cause 6 — expected the identical-content retry to be refused 409 ConflictException; actual HTTP 201 with body {'invocationStepId': 'ab88aea1-84fd-4d69-b6a5-b14a42e715fa'} (the same id as the first write).
Cause 7 — expected Message 'Next token for pagination is invalid'; actual "nextToken is not a pagination token this service issued for this account's sessions".
Cause 8 — expected Message 'Session resource not found.'; actual 'Resource arn:aws:bedrock:us-west-2:000000000000:session/46d34593-1802-4b45-9372-28e1be004337 was not found. Check the Amazon Resource Name (ARN) and try your request again.'
Cause 9 — expected results[0].index in (0, 2) (a France/Paris source); actual results = [{'index': 1, 'relevanceScore': 0.6666666865348816}, {'index': 4, 'relevanceScore': 0.6666666865348816}, {'index': 0, 'relevanceScore': 0.64549720287323}, {'index': 2, 'relevanceScore': 0.4330126941204071}, {'index': 3, 'relevanceScore': 0.15430335700511932}] for query 'What is the capital of France?'.
Cause 10 — expected CreateStack to accept the template; actual 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::Bedrock::Flow]'.

## Steps to Reproduce
Cause 1: create_session(); create_invocation(sessionIdentifier=<sid>, invocationId=<uuid>); repeat the create_invocation with the same invocationId -> inspect the 409 ConflictException Message.
Cause 2: create_session(encryptionKeyArn='not-a-valid-kms-arn') -> inspect the 400 ValidationException Message's quoted pattern.
Cause 3: create_session(); end_session(sessionIdentifier=<sid>); end_session again -> inspect the 409 ConflictException Message.
Cause 4: create one session with invocation A holding step S, and a second session with invocation B; then get_invocation_step(sessionIdentifier=<sid>, invocationIdentifier=<B>, invocationStepId=<S>) and get_invocation_step with an invocation that belongs to the other session -> inspect both 404 Messages.
Cause 5: put_invocation_step(sessionIdentifier=<sid>, invocationIdentifier=<inv>, invocationStepId=<uuid>, payload=P1); repeat with the same invocationStepId and a DIFFERENT payload -> inspect the 409 Message.
Cause 6: same as Cause 5 but repeat with a byte-identical payload -> emulator answers 201 instead of 409.
Cause 7: list_sessions(nextToken='not-a-token-this-service-issued') -> inspect the 400 ValidationException Message.
Cause 8: tag_resource(resourceArn='arn:aws:bedrock:<region>:<account>:session/<random uuid>', tags={'k':'v'}) -> inspect the 404 ResourceNotFoundException Message.
Cause 9: rerank(queries=[{'type':'TEXT','textQuery':{'text':'What is the capital of France?'}}], sources=[INLINE TEXT for 'Paris is the capital and most populous city of France.', 'Berlin is the capital of Germany.', 'The Eiffel Tower is located in Paris, France.', 'Bananas are a good source of potassium.', 'Madrid is the capital of Spain.'], rerankingConfiguration={'type':'BEDROCK_RERANKING_MODEL','bedrockRerankingConfiguration':{'modelConfiguration':{'modelArn':'arn:aws:bedrock:<region>::foundation-model/amazon.rerank-v1:0'}}}) -> results[0].index is 1, not 0 or 2.
Cause 10: cloudformation.create_stack with a template whose Resources contain an AWS::IAM::Role and a `FlowResource: Type: AWS::Bedrock::Flow` (Name, ExecutionRoleArn, Definition with an Input node and an Output node) -> 400 ValidationError 'Unrecognized resource types: [AWS::Bedrock::Flow]'.
