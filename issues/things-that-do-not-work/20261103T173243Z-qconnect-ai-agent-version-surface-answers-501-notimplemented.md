Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: qconnect.ai-agent-version-monotonic-numbering qconnect.ai-agent-version-immutable-after-update qconnect.delete-referenced-ai-agent-version-succeeds qconnect.content-update-stale-revision-rejected
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: qconnect.ai-agent-version-monotonic-numbering|_poll_until#8|TimeoutError|raise TimeoutError(f"poll did not satisfy predicate within {timeout_s}s, last value={value!r}")
Issue-Key: qconnect.ai-agent-version-immutable-after-update|run#9|TypeError|version = _poll_until(
Issue-Key: qconnect.delete-referenced-ai-agent-version-succeeds|run#9|TypeError|version = _poll_until(
Issue-Key: qconnect.content-update-stale-revision-rejected|run#18|AssertionError|assert message == "", f"AWS refuses a stale revisionId with an empty message, got {message!r}"
Issue-Commit: 17991a37a16e8138323e28f2fbf79fb37d0f661c

# qconnect AI Agent version surface answers 501 NotImplemented, and UpdateContent's stale-revisionId refusal carries a message AWS does not send (5 causes)

> _Note: this issue was identified against the code base at commit `17991a37a16e8138323e28f2fbf79fb37d0f661c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAIAgentVersion

## Description
Five net-new root causes in the qconnect (com.amazonaws.qconnect / WisdomService) emulator, grounded in four failing behavioral checks. Causes 1-4 are the whole AI Agent *versioning* surface still answering 501; cause 5 is a wording divergence on an implemented operation. Neither is covered by an existing open issue: the auto-filed stub issues for this service name CreateMessageTemplate, CreateAIPrompt, DeleteQuickResponse, SearchContent and UpdateContent only, and the replay report did not auto-file causes 1-4 because the checks call CreateAIAgentVersion inside a try/except poll helper that swallows the ClientError, so the harness never saw the NotImplemented code.

--- Cause 1 of 5: CreateAIAgentVersion is an unimplemented scaffold stub ---
AWS: Smithy: CreateAIAgentVersion, POST /assistants/{assistantId}/aiagents/{aiAgentId}/versions, success 200, output {aiAgent: AIAgentData, versionNumber: Version @range min 1}; modeled errors AccessDeniedException(403), ConflictException(409), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(400), UnauthorizedException(401), ValidationException(400). AWS publishes a numbered, immutable snapshot of the AI Agent's current $LATEST state and returns a strictly increasing versionNumber starting at 1 for the first version of an agent. 501/NotImplemented is not in that error set.
Emulator: every CreateAIAgentVersion call answers HTTP 501 with error code `NotImplemented` and message `operation CreateAIAgentVersion is scaffolded but not implemented in this emulator` — the handler is still the generated scaffold (`Err(crate::scaffolded::Scaffolded::not_implemented("CreateAIAgentVersion"))`). The checks poll it for 30s and never get a success, so they end with `TypeError: 'NoneType' object is not subscriptable` / `TimeoutError: poll did not satisfy predicate within 30.0s, last value=None`.
Why: a client can never publish an AI Agent version. No numbered agent id (`<aiAgentId>:<n>`) can ever exist, so version pinning, rollback and any assistant binding to a specific agent version are impossible, and 501 is an unmodeled status botocore surfaces as a retry-less NotImplemented rather than one of the modeled client errors.
Fix: the operation's handler, services/service-com-amazonaws-qconnect-wisdomservice/src/ops/create_ai_agent_version.rs, plus the version records it must add to crate::state (first version = 1, each subsequent one +1, snapshotting the agent's current configuration/description/visibilityStatus).
Traces: qconnect.ai-agent-version-monotonic-numbering, qconnect.ai-agent-version-immutable-after-update, qconnect.delete-referenced-ai-agent-version-succeeds.

--- Cause 2 of 5: ListAIAgentVersions is an unimplemented scaffold stub ---
AWS: Smithy: ListAIAgentVersions, GET /assistants/{assistantId}/aiagents/{aiAgentId}/versions, success 200, output {aiAgentVersionSummaries: AIAgentVersionSummariesList @required, nextToken}, paginated (inputToken/outputToken nextToken, pageSize maxResults, items aiAgentVersionSummaries); modeled errors AccessDenied(403), ResourceNotFound(404), Throttling(400), Unauthorized(401), Validation(400). AWS lists every published version of the agent, each summary carrying its versionNumber.
Emulator: answers HTTP 501 `NotImplemented` — `services/.../src/ops/list_ai_agent_versions.rs` still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and the scaffold `Err(...)`.
Why: even once cause 1 is fixed, a client cannot enumerate the versions it created, and cannot observe that a version was removed; the checks assert on the set of listed versionNumbers and would still fail.
Fix: the operation's handler, services/service-com-amazonaws-qconnect-wisdomservice/src/ops/list_ai_agent_versions.rs, reading the version records added for cause 1.
Traces: qconnect.ai-agent-version-monotonic-numbering, qconnect.delete-referenced-ai-agent-version-succeeds.

--- Cause 3 of 5: DeleteAIAgentVersion is an unimplemented scaffold stub ---
AWS: Smithy: DeleteAIAgentVersion, DELETE /assistants/{assistantId}/aiagents/{aiAgentId}/versions/{versionNumber}, success 204 with an empty output; modeled errors AccessDenied(403), Conflict(409), ResourceNotFound(404), Throttling(400), Unauthorized(401), Validation(400). Recorded behavior: the delete succeeds (204) even while that exact version is still bound to the assistant's AI Agent configuration — no referential-integrity refusal — and the version then disappears from ListAIAgentVersions.
Emulator: answers HTTP 501 `NotImplemented` — services/.../src/ops/delete_ai_agent_version.rs is still the scaffold.
Why: versions cannot be cleaned up; a client relying on a 204 delete gets an unmodeled 501, and test/teardown paths that delete a version leave state behind.
Fix: the operation's handler, services/service-com-amazonaws-qconnect-wisdomservice/src/ops/delete_ai_agent_version.rs — remove the version record, answer 204 with an empty body, and do NOT block on an existing assistant binding.
Traces: qconnect.delete-referenced-ai-agent-version-succeeds.

--- Cause 4 of 5: UpdateAssistantAIAgent is an unimplemented scaffold stub ---
AWS: Smithy: UpdateAssistantAIAgent, POST /assistants/{assistantId}/aiagentConfiguration, success 200, input {assistantId @required, aiAgentType @required, configuration: AIAgentConfigurationData @required, orchestratorUseCase}, output {assistant: AssistantData}; modeled errors AccessDenied(403), ResourceNotFound(404), Throttling(400), Validation(400). It binds an (optionally version-qualified) aiAgentId to the assistant for the given aiAgentType, and the assistant stays ACTIVE afterwards.
Emulator: answers HTTP 501 `NotImplemented` — services/.../src/ops/update_assistant_ai_agent.rs is still the scaffold.
Why: an assistant can never be pointed at an AI Agent, so the whole assistant-to-agent binding plane (and the paired RemoveAssistantAIAgent teardown) is unusable; the dangling-reference behavior the check exercises cannot be reached at all.
Fix: the operation's handler, services/service-com-amazonaws-qconnect-wisdomservice/src/ops/update_assistant_ai_agent.rs, storing the binding on the assistant record in crate::state and returning the updated AssistantData.
Traces: qconnect.delete-referenced-ai-agent-version-succeeds.

--- Cause 5 of 5: UpdateContent's stale-revisionId refusal carries a message AWS does not send ---
AWS: recorded behavior only for the message. Smithy: UpdateContent error PreconditionFailedException, errorKind client, httpError 412, member `message` (String, OPTIONAL — not @required), doc "The provided revisionId does not match, indicating the content has been modified since it was last read." The recording shows the refusal carrying NO message text at all: botocore reports Error.Code = `PreconditionFailedException` and Error.Message = `''` (botocore's JSON error parser defaults Message to the empty string when the body omits the member), i.e. AWS serializes the 412 without a populated `message`. There is no fixed text and no variable part to reproduce here — the requirement is the absence of the member.
Emulator: the status (412) and code (`PreconditionFailedException`) match, but the body carries `message` = `Request precondition failed` — a hard-coded constant, `MESSAGE_STALE_REVISION`, that the handler attaches to every stale-revision refusal. Check assertion: `AssertionError: AWS refuses a stale revisionId with an empty message, got 'Request precondition failed'`.
Why: a client that branches on, logs, or asserts the error message for an optimistic-lock failure sees a sentence real AWS never emits; parity harnesses and golden-output comparisons diverge on an operation that is otherwise correct (the surrounding assertions — the title not being applied, the revisionId advancing, a fresh-revisionId update succeeding — all passed).
Fix: the operation's error construction in services/service-com-amazonaws-qconnect-wisdomservice/src/ops/update_content.rs — stop populating PreconditionFailedException.message (leave it None) for the stale-revision path; the `MESSAGE_STALE_REVISION` constant at line 163 and its use are what has to go. The sibling legacy view at services/service-com-amazonaws-wisdom-wisdomservice/src/ops/update_content.rs carries its own copy of the same constant and must stay consistent. NOTE: the open issue 20261029T203651Z-qconnect-updatecontent-stub.md ("Implement qconnect UpdateContent: unimplemented scaffold stub") does NOT cover this — UpdateContent is implemented and answering; only the message text diverges.
Traces: qconnect.content-update-stale-revision-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, qconnect 14/25 passed (56%), 11 failed. 7 of the 11 are already filed (6 blocked by the CreateMessageTemplate stub, 1 by the CreateAIPrompt stub) and are excluded here. The 4 remaining are this issue's grounding.

Causes 1-4 (AI Agent versioning surface, all three checks):
  [error] qconnect.ai-agent-version-monotonic-numbering
    TimeoutError: poll did not satisfy predicate within 30.0s, last value=None
    (the poll body is `_try_create_version` -> qc.create_ai_agent_version(...), whose ClientError is swallowed; the underlying answer is
     HTTP 501 Error.Code=NotImplemented, Error.Message="operation CreateAIAgentVersion is scaffolded but not implemented in this emulator")
    Expected: v1 == 1, v2 == 2, ListAIAgentVersions -> {1, 2}.
  [error] qconnect.ai-agent-version-immutable-after-update
    TypeError: 'NoneType' object is not subscriptable at `version = _poll_until(_try_create_version, ...)["versionNumber"]`
    Expected: CreateAIAgentVersion returns a versionNumber; $LATEST shows description "updated-description" while version N stays "original-description".
  [error] qconnect.delete-referenced-ai-agent-version-succeeds
    TypeError: 'NoneType' object is not subscriptable at the same CreateAIAgentVersion poll.
    Expected: CreateAIAgentVersion 200 -> ListAIAgentVersions contains it -> UpdateAssistantAIAgent 200 binds `<agentId>:<version>` ->
    DeleteAIAgentVersion 204 succeeds despite the binding -> ListAIAgentVersions no longer contains it -> assistant still ACTIVE.
  Emulator source confirming the cause (each file's header still carries `SCAFFOLDED, NOT IMPLEMENTED`):
    services/service-com-amazonaws-qconnect-wisdomservice/src/ops/create_ai_agent_version.rs:185  Err(Scaffolded::not_implemented("CreateAIAgentVersion"))
    services/service-com-amazonaws-qconnect-wisdomservice/src/ops/list_ai_agent_versions.rs:4
    services/service-com-amazonaws-qconnect-wisdomservice/src/ops/delete_ai_agent_version.rs:4
    services/service-com-amazonaws-qconnect-wisdomservice/src/ops/update_assistant_ai_agent.rs:4
  Note the preceding steps of these checks PASS: CreateAIAgent, UpdateAIAgent(visibilityStatus=PUBLISHED) and GetAIAgent all now work and the agent does reach PUBLISHED, so the failure is isolated to the version/binding operations.

Cause 5 (UpdateContent stale revision):
  [mismatch] qconnect.content-update-stale-revision-rejected
    Expected: UpdateContent(knowledgeBaseId, contentId, title=..., revisionId=<stale>) -> HTTP 412, Error.Code="PreconditionFailedException", Error.Message="" (no message member serialized).
    Actual:   HTTP 412, Error.Code="PreconditionFailedException", Error.Message="Request precondition failed".
    AssertionError: AWS refuses a stale revisionId with an empty message, got 'Request precondition failed'
  Emulator source: services/service-com-amazonaws-qconnect-wisdomservice/src/ops/update_content.rs:163
    const MESSAGE_STALE_REVISION: &str = "Request precondition failed";
  Smithy: PreconditionFailedException { message: String (optional) }, httpError 412 — the member is optional, so omitting it is model-legal.

## Steps to Reproduce
Causes 1-4 (AI Agent versioning surface):
1. qconnect CreateAssistant(name=<unique>, type="AGENT"); poll GetAssistant until status == ACTIVE.
2. CreateAIAgent(assistantId, name=<unique>, type="MANUAL_SEARCH", configuration={"manualSearchAIAgentConfiguration": {}}, visibilityStatus="SAVED", description="original-description").
3. UpdateAIAgent(assistantId, aiAgentId, visibilityStatus="PUBLISHED"); poll GetAIAgent until visibilityStatus == PUBLISHED (this part succeeds today).
4. CreateAIAgentVersion(assistantId, aiAgentId) -> observe HTTP 501 Error.Code=NotImplemented instead of 200 with versionNumber=1. Repeat -> expect versionNumber=2.
5. ListAIAgentVersions(assistantId, aiAgentId) -> observe HTTP 501 instead of 200 with aiAgentVersionSummaries containing versionNumbers {1,2}.
6. UpdateAssistantAIAgent(assistantId, aiAgentType="MANUAL_SEARCH", configuration={"aiAgentId": "<aiAgentId>:1"}) -> observe HTTP 501 instead of 200 with the assistant.
7. DeleteAIAgentVersion(assistantId, aiAgentId, versionNumber=1) -> observe HTTP 501 instead of 204; after a correct delete, ListAIAgentVersions must no longer contain 1 and GetAssistant must still report ACTIVE.
8. Also: UpdateAIAgent(..., description="updated-description") must change only $LATEST; GetAIAgent(aiAgentId="<aiAgentId>:1") must still report description "original-description".

Cause 5 (UpdateContent stale revisionId message):
1. CreateKnowledgeBase(name=<unique>, knowledgeBaseType="CUSTOM"); poll GetKnowledgeBase until ACTIVE.
2. StartContentUpload(knowledgeBaseId, contentType="text/plain"); PUT the returned url with the returned headersToInclude and a small body.
3. CreateContent(knowledgeBaseId, name=<unique>, uploadId); poll GetContent until status == ACTIVE; record revisionId as <stale>.
4. UpdateContent(knowledgeBaseId, contentId, title="first-title-change") with NO revisionId -> 200; GetContent shows a NEW revisionId.
5. UpdateContent(knowledgeBaseId, contentId, title="should-not-apply", revisionId=<stale>) -> the refusal is correct in status (412) and code (PreconditionFailedException) but carries Error.Message="Request precondition failed"; AWS serializes no message, so botocore reports Error.Message == "".
