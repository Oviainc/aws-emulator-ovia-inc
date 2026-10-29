Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: qconnect.assistant-create-active-delete-lifecycle qconnect.duplicate-assistant-association-rejected qconnect.quick-response-search-finds-created
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: qconnect.assistant-create-active-delete-lifecycle|run#15|AssertionError|assert tag_code == "NotFoundException", (
Issue-Key: qconnect.duplicate-assistant-association-rejected|run#9|AssertionError|assert message == "Association already exists for these two resources", f"unexpected message {message!r}"
Issue-Key: qconnect.quick-response-search-finds-created|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: 6074294cbfccbc2257fa0c432afede02752270ab

# qconnect: three fidelity divergences — TagResource not-found error code, duplicate-association ConflictException wording, and SearchQuickResponses always empty

> _Note: this issue was identified against the code base at commit `6074294cbfccbc2257fa0c432afede02752270ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource, CreateAssistantAssociation, SearchQuickResponses

## Description
Three distinct root causes in the Amazon Q in Connect (wisdom) emulator crate, all surfaced by the behavioral suite. They are independent: each must be closed before this issue is submitted.

Cause 1 of 3: TagResource against a resource ARN that no longer exists is refused with the WRONG error code.
AWS: recorded behavior only. The Smithy model (services/service-com-amazonaws-qconnect-wisdomservice/smithy-model/com-amazonaws-qconnect-wisdomservice.json) models TagResource with errors ResourceNotFoundException (404) and TooManyTagsException (400) and no NotFoundException shape exists in the model at all; the recording of the real service nevertheless refuses TagResource on a deleted assistant ARN with the error code `NotFoundException`. The check asserts the CODE only, so the code is the requirement here and no message text is asserted. Hypothesis: Q in Connect's tagging path reports resource absence with its own `NotFoundException` code rather than the modeled ResourceNotFoundException.
Emulator: returns code `ResourceNotFoundException` (HTTP 404, message from crate::state::MESSAGE_RESOURCE_NOT_FOUND, resourceName = the request's resourceArn) for the same call.
Why: a client that catches `NotFoundException` around TagResource — the code real AWS emits, and the one botocore/SDK error handlers are written against for this operation — sees an unhandled error class instead, so post-delete tagging cleanup paths and 'tag if the resource still exists' idempotency guards break against the emulator even though the HTTP status matches.
Fix: the TagResource handler's not-found refusal (its error channel / the error code it selects for a resourceArn that resolves to nothing). The same code selection should be checked for the sibling tagging operations (UntagResource, ListTagsForResource) that read the same ARN resolution.
Traces: qconnect.assistant-create-active-delete-lifecycle — "expected TagResource on the deleted assistant to fail with NotFoundException, got ResourceNotFoundException"; refusal built at services/service-com-amazonaws-qconnect-wisdomservice/src/ops/tag_resource.rs:178-186.

Cause 2 of 3: the duplicate-association ConflictException carries the wrong message.
AWS: Smithy: CreateAssistantAssociation errors include ConflictException (HTTP 409, members: message); the model's ConflictException doc is generic ("The request could not be processed because of conflict in the current state of the resource"), so the exact wording is recorded behavior only. The recording shows a second CreateAssistantAssociation for an assistant/knowledge-base pair that is already associated refused with code ConflictException and message exactly "Association already exists for these two resources" — that string is entirely FIXED text, with no variable part taken from the request or from state (no assistant id, knowledge base id or association id appears in it).
Emulator: status 409 and code ConflictException are right, but the message is "The assistant is already associated with this resource."
Why: the status and code match, so the divergence is invisible to a dispatch-on-code client, but any client, test or operator runbook that matches or logs AWS's message text for this conflict class sees different wording, and a message that differs from AWS's wording is itself a defect.
Fix: the CreateAssistantAssociation handler's duplicate-target conflict branch (the literal message it passes to its ConflictException constructor) — the detection logic (keyed per associated target) is correct and should not change.
Traces: qconnect.duplicate-assistant-association-rejected — "unexpected message 'The assistant is already associated with this resource.'"; message literal at services/service-com-amazonaws-qconnect-wisdomservice/src/ops/create_assistant_association.rs:319-327.

Cause 3 of 3: SearchQuickResponses never returns any result, even for a quick response the knowledge base holds.
AWS: Smithy: SearchQuickResponses POST /knowledgeBases/{knowledgeBaseId}/search/quickResponses, output member `results` (QuickResponseSearchResultsList) is @required, paginated on nextToken/maxResults; input searchExpression.queries is a list of QuickResponseQueryField whose documented queryable field names are content, name, description and shortcutKey, with an operator (e.g. CONTAINS). AWS returns the quick responses of that knowledge base which the expression selects, so a quick response created in the knowledge base and observed in status CREATED is found by a CONTAINS query on its own `name`.
Emulator: the handler resolves the parent knowledge base and then hardcodes an EMPTY result list — it never consults the parent's stored quick responses and never evaluates searchExpression at all. Every search of an existing knowledge base answers `{"results": []}` with no nextToken. The handler's own comment justifies this with "CreateQuickResponse ... is still scaffolded in this crate", which is stale: CreateQuickResponse and GetQuickResponse are implemented (the suite creates a quick response and polls it to CREATED successfully), so the store does hold records the search ignores.
Why: a client that creates a quick response and then looks it up through the data-plane search — the normal way agents resolve quick responses by name — gets an empty page forever. A poll-until-visible loop (the recorded client behavior) never terminates; here it consumed the whole check budget and the check timed out after 60s rather than failing on a value.
Fix: the SearchQuickResponses handler's projection and its state read — page the parent knowledge base's stored quick responses and evaluate searchExpression.queries/filters (name/content/description/shortcutKey, the query operator and allowFuzziness) against them, honouring maxResults and minting the nextToken this crate's cursor helper already provides. Remove the stale "create is scaffolded" assumption from the handler's doc comment.
Traces: qconnect.quick-response-search-finds-created — "Check timed out after 60.0s" while polling the search for the created id; hardcoded empty page at services/service-com-amazonaws-qconnect-wisdomservice/src/ops/search_quick_responses.rs:207-219.

## Evidence
Cause 1 — qconnect.assistant-create-active-delete-lifecycle:
  CreateAssistant(name=<unique>, type=AGENT) -> ACTIVE; DeleteAssistant(assistantId); GetAssistant -> ResourceNotFoundException (expected, passes).
  TagResource(resourceArn=<the deleted assistant's ARN>, tags={probe: post-delete}):
    expected: Error.Code == "NotFoundException"
    actual:   Error.Code == "ResourceNotFoundException" (HTTP 404)
  Assertion: AssertionError: expected TagResource on the deleted assistant to fail with NotFoundException, got ResourceNotFoundException

Cause 2 — qconnect.duplicate-assistant-association-rejected:
  CreateAssistantAssociation(assistantId=A, associationType=KNOWLEDGE_BASE, association={knowledgeBaseId: K}) twice.
    second call expected: HTTP 409, Error.Code "ConflictException", Error.Message "Association already exists for these two resources" (fixed text, no interpolated values)
    second call actual:   HTTP 409, Error.Code "ConflictException", Error.Message "The assistant is already associated with this resource."
  Assertion: AssertionError: unexpected message 'The assistant is already associated with this resource.'
  (The code assertion passed; only the wording diverges. The later restore steps — delete the association, re-create it — were never reached.)

Cause 3 — qconnect.quick-response-search-finds-created:
  CreateKnowledgeBase(knowledgeBaseType=QUICK_RESPONSES) -> ACTIVE (ok)
  CreateQuickResponse(knowledgeBaseId=K, name=N, content={content: "..."}) -> quickResponseId=Q; GetQuickResponse polls to status CREATED (ok)
  SearchQuickResponses(knowledgeBaseId=K, searchExpression={queries: [{name: "name", values: [N], operator: "CONTAINS"}]}):
    expected: results contains an entry with quickResponseId == Q
    actual:   results == [] on every call, indefinitely (handler returns Vec::new() unconditionally)
  Result: Check timed out after 60.0s while polling the search to surface Q.

## Steps to Reproduce
Cause 1:
1. qc.create_assistant(name=<unique>, type="AGENT"); poll get_assistant until status == ACTIVE; keep assistantArn.
2. qc.delete_assistant(assistantId=...); poll get_assistant until it raises (ResourceNotFoundException).
3. qc.tag_resource(resourceArn=<assistantArn>, tags={"probe": "post-delete"}) and read Error.Code: emulator says ResourceNotFoundException, AWS says NotFoundException.

Cause 2:
1. Create an ACTIVE assistant A and an ACTIVE CUSTOM knowledge base K.
2. qc.create_assistant_association(assistantId=A, associationType="KNOWLEDGE_BASE", association={"knowledgeBaseId": K}).
3. Issue the identical call again and read Error.Message: emulator says "The assistant is already associated with this resource.", AWS says "Association already exists for these two resources".

Cause 3:
1. Create an ACTIVE knowledge base K with knowledgeBaseType="QUICK_RESPONSES".
2. qc.create_quick_response(knowledgeBaseId=K, name=N, content={"content": "Your order has shipped!"}); poll get_quick_response until status == CREATED.
3. qc.search_quick_responses(knowledgeBaseId=K, searchExpression={"queries": [{"name": "name", "values": [N], "operator": "CONTAINS"}]}) -> results is always [], so the created quick response is never found.
