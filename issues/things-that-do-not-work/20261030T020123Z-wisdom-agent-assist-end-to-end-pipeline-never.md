Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wisdom.assistant-knowledge-base-content-session-end-to-end-workflow
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom.assistant-knowledge-base-content-session-end-to-end-workflow|_poll_until#32|Timeout|time.sleep(interval_s)
Issue-Commit: 6007c454e542516ce92f9fc20505b3e72db0d81f

# wisdom agent-assist end-to-end pipeline never reaches a verdict: the chained assistant→KB→content→session→query flow is killed at the 60s budget, and QueryAssistant answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `6007c454e542516ce92f9fc20505b3e72db0d81f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
QueryAssistant

## Description
Two net-new root causes in the wisdom (Amazon Connect Wisdom) data plane, both exercised by the one end-to-end behavioral check that chains the whole agent-assist pipeline: CreateAssistant → CreateKnowledgeBase → CreateAssistantAssociation → GetAssistantAssociation → ListAssistantAssociations → StartContentUpload + presigned PUT → CreateContent → GetContent (poll to ACTIVE) → CreateSession → GetSession → GetRecommendations → QueryAssistant.

Cause 1 of 2: the chained pipeline never returns a verdict — the check is killed at the harness budget instead of completing or failing a hop.
AWS: every hop of this chain answers synchronously and promptly. Smithy (services/service-com-amazonaws-wisdom-wisdomservice/smithy-model/com-amazonaws-wisdom-wisdomservice.json): CreateContent POST /knowledgeBases/{knowledgeBaseId}/contents → 200 ContentData; GetContent GET /knowledgeBases/{knowledgeBaseId}/contents/{contentId} → 200 ContentData (ContentStatus moves CREATE_IN_PROGRESS → ACTIVE); ListAssistantAssociations GET /assistants/{assistantId}/associations → 200 with required assistantAssociationSummaries; GetRecommendations GET /assistants/{assistantId}/sessions/{sessionId}/recommendations with waitTimeSeconds @default 0 @range 0..20 → 200 with required recommendations list; QueryAssistant POST /assistants/{assistantId}/query → 200 with required results list. A client driving this pipeline gets a response for every call within seconds.
Emulator: the replay reports `[mismatch] wisdom.assistant-knowledge-base-content-session-end-to-end-workflow — Error: Check timed out after 60.0s`. Nothing in the chain produced a verdict inside the 60s budget: no hop's response, no refusal and no assertion failure were observed, so the pipeline as a whole is unusable even though its individual calls pass component-level checks. The content state machine is NOT the stall: crate::state::CONTENT_ACTIVATION_DELAY is 500ms, so the CREATE_IN_PROGRESS → ACTIVE poll should settle on the first or second probe. Hypothesis (neither model nor recording states it): the stall is a hop that never answers at all, which for a client is indistinguishable from a hung endpoint — the SDK sits on its read timeout. Two hops on this chain are known-unserved stubs in this crate and are already filed separately — ListAssistantAssociations (20261030T000858Z-wisdom-listassistantassociations-stub.md) and SearchContent, the search surface this flow's query depends on (20261030T000858Z-wisdom-searchcontent-stub.md) — but neither of those issues covers the end-to-end hang itself, and a 501 refusal would have produced an immediate error verdict rather than a 60s timeout, so a distinct defect remains after both are closed.
Why: a client that composes Wisdom the way the service is meant to be composed (associate a knowledge base, ingest content, open a session, ask for recommendations/answers) never gets an answer and cannot distinguish a wedged endpoint from a slow one. Component-level fidelity of each call is already present; the composed flow is the thing that does not work.
Fix: the wisdom service's data-plane wiring — the handlers on this chain plus the state that threads knowledge-base association → ingested content → session recommendation/query feed, and whichever hop fails to answer. This must be re-driven as a whole chain, not per call: each call already passes in isolation.
Traces: wisdom.assistant-knowledge-base-content-session-end-to-end-workflow.

Cause 2 of 2: QueryAssistant is an unimplemented scaffold stub, so the terminal hop of the pipeline can never succeed.
AWS: Smithy QueryAssistant — POST /assistants/{assistantId}/query, input assistantId (@httpLabel UuidOrArn, required), queryText (QueryText, required, @sensitive), optional nextToken/maxResults (@range 1..100); output 200 with REQUIRED results (QueryResultsList) and optional nextToken; modeled errors AccessDeniedException (403), RequestTimeoutException (408), ResourceNotFoundException (404), ValidationException (400). A query against an assistant whose associated knowledge base holds the ingested content answers 200 and the results carry that content's contentReference.contentId with its excerpt/title text; a query naming an assistant the account does not hold answers 404 ResourceNotFoundException. `NotImplemented` is not a code this operation can return.
Emulator: services/service-com-amazonaws-wisdom-wisdomservice/src/ops/query_assistant.rs is still the generated stub — handle() returns crate::scaffolded::Scaffolded::not_implemented("QueryAssistant"), which renders HTTP 501 with wire error code `NotImplemented` and the message `operation QueryAssistant is scaffolded but not implemented in this emulator` (fixed emulator text; the operation name is its only variable part). No status, no modeled error and no result shape of QueryAssistant is therefore asserted by this emulator, and the one behavioral check that reaches this hop can never pass.
Why: a client branching on QueryAssistant's 200/results or on its modeled 404/400/408 refusals gets a 501 with a code outside the operation's model — not retryable, not matchable, and it hides whether the content it ingested is queryable at all.
Fix: implement the QueryAssistant handler in that file against the content records CreateContent persisted for the assistant's associated knowledge base (the same records the SearchContent surface must serve), returning the modeled results list and the modeled ResourceNotFoundException for an assistant/knowledge base the account does not hold; remove the SCAFFOLDED marker.
Traces: wisdom.assistant-knowledge-base-content-session-end-to-end-workflow.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), wisdom 33/47 (70%), 14 failed.

Failing check, not marked ALREADY FILED and not OWNED by any open issue:
  [mismatch] wisdom.assistant-knowledge-base-content-session-end-to-end-workflow
  Error: Check timed out after 60.0s

Expected (AWS): every hop of the chain answers within seconds — CreateContent 200 CREATE_IN_PROGRESS, GetContent 200 reaching ACTIVE, ListAssistantAssociations 200 with the association just created, CreateSession/GetSession 200, GetRecommendations 200 with a recommendations list (waitTimeSeconds default 0 = short poll), QueryAssistant 200 with a results list whose contentReference.contentId is the content this run uploaded and whose excerpt or title carries the uploaded marker.

Actual (emulator): no verdict at all within the 60s check budget — the pipeline neither completes nor refuses. The content state machine cannot account for it (CONTENT_ACTIVATION_DELAY = 500ms in services/service-com-amazonaws-wisdom-wisdomservice/src/state.rs:327-328).

Actual for the terminal hop: services/service-com-amazonaws-wisdom-wisdomservice/src/ops/query_assistant.rs:1-12 carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and its handle body (same file, handler tail) returns Scaffolded::not_implemented("QueryAssistant") → HTTP 501, error code `NotImplemented`, message `operation QueryAssistant is scaffolded but not implemented in this emulator`, versus AWS's modeled 200 {results:[...]} / 404 ResourceNotFoundException / 400 ValidationException / 408 RequestTimeoutException / 403 AccessDeniedException.

Already-filed neighbours on the same chain (named, not re-filed): 20261030T000858Z-wisdom-listassistantassociations-stub.md (ListAssistantAssociations 501) and 20261030T000858Z-wisdom-searchcontent-stub.md (SearchContent 501).

## Steps to Reproduce
1. Boot the emulator and point a botocore `wisdom` client at it.
2. create_assistant(name=<unique>, type='AGENT', tags={...}) → keep assistantId.
3. create_knowledge_base(name=<unique>, knowledgeBaseType='CUSTOM') → keep knowledgeBaseId.
4. create_assistant_association(assistantId=.., associationType='KNOWLEDGE_BASE', association={'knowledgeBaseId': ..}) → keep assistantAssociationId; then get_assistant_association(..) and list_assistant_associations(assistantId=..) and check the association is reflected by both.
5. start_content_upload(knowledgeBaseId=.., contentType='text/plain'), PUT a body containing a run-generated marker to the returned url with headersToInclude, then create_content(knowledgeBaseId=.., name=<unique>, uploadId=..).
6. Poll get_content(knowledgeBaseId=.., contentId=..) until content.status is ACTIVE or CREATE_FAILED.
7. create_session(assistantId=.., name=<unique>), get_session(..), then get_recommendations(assistantId=.., sessionId=..).
8. query_assistant(assistantId=.., queryText='marker') and look for a result whose document.contentReference.contentId equals the created contentId and whose excerpt/title text carries the marker.
   Observed: the whole sequence produces no verdict within 60s (replay check wisdom.assistant-knowledge-base-content-session-end-to-end-workflow times out at 60.0s); step 8 in isolation answers HTTP 501 `NotImplemented` (`operation QueryAssistant is scaffolded but not implemented in this emulator`).
   Expected: every call answers promptly, and step 8 answers 200 with a results list containing the uploaded content.
