Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wisdom.create-quick-response-against-custom-kb-rejected wisdom.quick-response-search-finds-created wisdom.search-quick-responses-name-prefix-filter wisdom.delete-quick-response-removes-from-search wisdom.update-content-revisionid-race-concurrency
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom.create-quick-response-against-custom-kb-rejected|_error#6|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: wisdom.quick-response-search-finds-created|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: wisdom.search-quick-responses-name-prefix-filter|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: wisdom.delete-quick-response-removes-from-search|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: wisdom.update-content-revisionid-race-concurrency|run#17|AssertionError|assert outcomes.count("ok") == 1, f"expected exactly one winner, got {results}"
Issue-Commit: de414c514180eddfb96a534a3f6304a808cc952a

# wisdom quick-response/content data plane: CreateQuickResponse is not gated by knowledge-base type, SearchQuickResponses is an unimplemented stub, and UpdateContent never reaches its revisionId precondition (3 causes)

> _Note: this issue was identified against the code base at commit `de414c514180eddfb96a534a3f6304a808cc952a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchQuickResponses

## Description
Three independent defects on the wisdom quick-response and content data plane. 3 causes, all in the wisdom service. Related but NOT covering: the open issue 20261029T192454Z-wisdom-quickresponse-state-not-persisted-create-get-list.md is about members dropped from Create/Get/ListQuickResponses and ignored update/delete — it says nothing about the knowledge-base-type gate, nothing about SearchQuickResponses, and nothing about UpdateContent.

Cause 1 of 3: CreateQuickResponse is accepted against a CUSTOM-type knowledge base, which AWS refuses.
AWS: quick responses live only in a QUICK_RESPONSES-type knowledge base. Smithy: CreateQuickResponse (POST /knowledgeBases/{knowledgeBaseId}/quickResponses) models ValidationException @httpError 400 ('The input fails to satisfy the constraints specified by a service'); ListQuickResponses models the same error. Recorded behavior: against a knowledge base whose knowledgeBaseType is CUSTOM, CreateQuickResponse answers HTTP 400 ValidationException with message exactly `QuickResponse APIs are not supported for CUSTOM knowledge bases` — fixed text naming the knowledge-base type taken from the target knowledge base's state (CUSTOM here), nothing else substituted — and the whole quick-response surface for that knowledge base is refused the same way, so ListQuickResponses against it answers the identical 400/message.
Emulator: answers HTTP 200 and mints a quick response in the CUSTOM knowledge base (status CREATED, a quickResponseArn under that knowledge base), i.e. it never consults the parent knowledge base's type.
Why: the emulator is overly permissive — it creates a resource AWS would never create, so code written against the emulator ships a quick-response flow that fails in production the first time it is pointed at a CUSTOM knowledge base, and the 'wrong knowledge-base type' misconfiguration is never surfaced to the developer.
Fix: the CreateQuickResponse handler's validation must gate on the resolved parent knowledge base's knowledgeBaseType before creating anything, and the same gate belongs on the rest of the quick-response surface the check reads next (ListQuickResponses, and by extension the other quick-response operations) so a CUSTOM knowledge base exposes no quick-response plane at all.
Traces: check wisdom.create-quick-response-against-custom-kb-rejected (it asserts the create refusal first and the ListQuickResponses refusal immediately after, then runs the identical flow against a QUICK_RESPONSES knowledge base as a positive control); handlers services/service-com-amazonaws-wisdom-wisdomservice/src/ops/create_quick_response.rs and .../list_quick_responses.rs; knowledge-base type is already stored in .../src/state.rs (KnowledgeBase.knowledge_base_type).

Cause 2 of 3: SearchQuickResponses is an unimplemented scaffold, so nothing on the quick-response search path can be observed and three checks run to their deadline.
AWS: Smithy: SearchQuickResponses (POST /knowledgeBases/{knowledgeBaseId}/search/quickResponses) answers 200 with `results` (@required QuickResponseSearchResultsList, paginated with nextToken/maxResults) selected by searchExpression.queries (a query names a field such as `name` or `content`, carries values, an operator CONTAINS or CONTAINS_AND_PREFIX, and allowFuzziness); it models ValidationException 400, ResourceNotFoundException 404, AccessDeniedException 403 and RequestTimeoutException 408. A quick response created in a QUICK_RESPONSES knowledge base becomes findable by a CONTAINS query on its name or content, a CONTAINS_AND_PREFIX query on a shared name prefix returns exactly the quick responses carrying that prefix and excludes unrelated ones, and after DeleteQuickResponse the deleted id stops being returned.
Emulator: the handler is the generated scaffold — it answers 501 NotImplemented for every request and asserts nothing about search (`services/service-com-amazonaws-wisdom-wisdomservice/src/ops/search_quick_responses.rs` still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and returns `Scaffolded::not_implemented("SearchQuickResponses")`). In replay the three checks that poll this operation never obtain a result and are killed by the harness's 60s deadline instead of completing.
Why: the quick-response data plane is unusable — an agent-assist client cannot look up a quick response by name, content or shortcut, which is the only read path these resources exist for, and deletion cannot be confirmed through search.
Fix: implement the SearchQuickResponses handler over the stored quick responses (field-scoped CONTAINS / CONTAINS_AND_PREFIX matching, maxResults/nextToken paging, 404 for an unknown knowledge base) and delete the SCAFFOLDED marker line.
Traces: checks wisdom.quick-response-search-finds-created (CONTAINS on name must find the just-created quick response), wisdom.search-quick-responses-name-prefix-filter (CONTAINS_AND_PREFIX on a shared name prefix returns both prefixed quick responses and excludes the unrelated one), wisdom.delete-quick-response-removes-from-search (the deleted id must disappear from search and GetQuickResponse must then answer ResourceNotFoundException with message 'QuickResponse does not exist.'); file services/service-com-amazonaws-wisdom-wisdomservice/src/ops/search_quick_responses.rs.

Cause 3 of 3: UpdateContent answers 501, so its revisionId precondition — the only concurrency control on content — does not exist.
AWS: Smithy: UpdateContent (POST /knowledgeBases/{knowledgeBaseId}/contents/{contentId}) takes an optional `revisionId` (NonEmptyString, length 1..4096) and models PreconditionFailedException @httpError 412 alongside ValidationException 400 and ResourceNotFoundException 404. Two concurrent UpdateContent calls that both carry the SAME revisionId resolve with exactly one winner: that one answers 200 with the new ContentData and a new revisionId, and the loser is refused 412 PreconditionFailedException because the revision it named is no longer current.
Emulator: both concurrent calls are refused with 501 NotImplemented (replay: outcomes [('error', 'NotImplemented'), ('error', 'NotImplemented')]), so there is no winner and no precondition check at all.
Why: a client that uses revisionId for optimistic concurrency on content cannot write at all, and once the operation is implemented without the precondition, concurrent writers would silently clobber each other instead of one being told to re-read.
Fix: the UpdateContent handler — it must mutate stored content state and compare the supplied revisionId against the stored one, refusing the stale writer with PreconditionFailedException (412). The bare 501 is additionally covered by the scaffold-stub issue filed for UpdateContent in this same run; this cause records the revisionId concurrency contract that stub issue does not state, and both must hold before the check passes.
Traces: check wisdom.update-content-revisionid-race-concurrency; handler services/service-com-amazonaws-wisdom-wisdomservice/src/ops/update_content.rs.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout (wisdom 26/47 checks passed).

Cause 1 — wisdom.create-quick-response-against-custom-kb-rejected [mismatch]:
  AssertionError: expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'quickResponse': {... 'knowledgeBaseId': <the CUSTOM knowledge base>, 'quickResponseArn': 'arn:aws:wisdom:<region>:<account>:quick-response/<kbId>/<qrId>', 'status': 'CREATED', ...}}
  expected HTTP 400 ValidationException, message 'QuickResponse APIs are not supported for CUSTOM knowledge bases' (the ids, name, region and account in the body above are per-run values, not requirements).

Cause 2 — three checks that poll SearchQuickResponses [mismatch]: 'Check timed out after 60.0s' for wisdom.quick-response-search-finds-created, wisdom.search-quick-responses-name-prefix-filter and wisdom.delete-quick-response-removes-from-search; the operation's handler is the untouched scaffold that returns Scaffolded::not_implemented("SearchQuickResponses") (HTTP 501 NotImplemented), so no search result is ever served. The surrounding setup these checks perform (CreateKnowledgeBase QUICK_RESPONSES reaching ACTIVE, CreateQuickResponse reaching CREATED, GetQuickResponse) does answer.

Cause 3 — wisdom.update-content-revisionid-race-concurrency [mismatch]:
  AssertionError: expected exactly one winner, got [('error', 'NotImplemented'), ('error', 'NotImplemented')]
  expected exactly one 200 and one 412 PreconditionFailedException.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite simparity-behavioral-aws-v1 (wisdom checks).
2. Cause 1: CreateKnowledgeBase with knowledgeBaseType CUSTOM, then POST /knowledgeBases/{id}/quickResponses with a name and content — the emulator answers 200 and creates the quick response instead of 400 ValidationException 'QuickResponse APIs are not supported for CUSTOM knowledge bases'; GET /knowledgeBases/{id}/quickResponses against the same CUSTOM knowledge base must be refused the same way. Check wisdom.create-quick-response-against-custom-kb-rejected.
3. Cause 2: CreateKnowledgeBase with knowledgeBaseType QUICK_RESPONSES, CreateQuickResponse in it, then POST /knowledgeBases/{id}/search/quickResponses with searchExpression {"queries": [{"name": "name", "values": [<token from the created name>], "operator": "CONTAINS"}]} — the operation answers 501 NotImplemented and never returns results, so the checks poll until the 60s deadline. Repeat with operator CONTAINS_AND_PREFIX over two quick responses sharing a name prefix plus one unrelated, and again after DeleteQuickResponse. Checks wisdom.quick-response-search-finds-created, wisdom.search-quick-responses-name-prefix-filter, wisdom.delete-quick-response-removes-from-search.
4. Cause 3: create a content item in a CUSTOM knowledge base, read its revisionId, then issue two concurrent POST /knowledgeBases/{kbId}/contents/{contentId} calls both carrying that same revisionId — both answer 501 NotImplemented; AWS answers one 200 (new revisionId) and one 412 PreconditionFailedException. Check wisdom.update-content-revisionid-race-concurrency.
