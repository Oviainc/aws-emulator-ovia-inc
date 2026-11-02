Issue-Source: trace-sweep-divergence
Issue-Trace: wisdom-quick-response-error-paths
Issue-Step: setup#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wisdom-quick-response-error-paths wisdom-quick-response-lifecycle
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom|CreateQuickResponse|2xx->2xx|$.quickResponse.attributeList
Issue-Key: wisdom|CreateQuickResponse|2xx->2xx|$.quickResponse.channels
Issue-Key: wisdom|CreateQuickResponse|2xx->2xx|$.quickResponse.contentType
Issue-Key: wisdom|CreateQuickResponse|2xx->2xx|$.quickResponse.contents.markdown
Issue-Key: wisdom|CreateQuickResponse|2xx->2xx|$.quickResponse.isActive
Issue-Key: wisdom|CreateQuickResponse|2xx->2xx|$.quickResponse.language
Issue-Key: wisdom|CreateQuickResponse|2xx->2xx|$.quickResponse.lastModifiedBy
Issue-Key: wisdom|GetQuickResponse|2xx->2xx|$.quickResponse.attributeList
Issue-Key: wisdom|GetQuickResponse|2xx->2xx|$.quickResponse.channels
Issue-Key: wisdom|GetQuickResponse|2xx->2xx|$.quickResponse.language
Issue-Key: wisdom|GetQuickResponse|2xx->2xx|$.quickResponse.lastModifiedBy
Issue-Key: wisdom|ListQuickResponses|2xx->2xx|$.quickResponseSummaries[name="sim2real-probe-wisdom-qr-ce634326"].channels
Issue-Key: wisdom|ListQuickResponses|2xx->2xx|$.quickResponseSummaries[name="sim2real-probe-wisdom-qr-ce634326"].lastModifiedBy
Issue-Key: wisdom|GetQuickResponse|2xx->2xx|$.quickResponse.description
Issue-Key: wisdom|GetQuickResponse|2xx->2xx|$.quickResponse.isActive
Issue-Key: wisdom|GetQuickResponse|4xx->2xx|$.message
Issue-Key: wisdom|GetQuickResponse|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: wisdom|GetQuickResponse|4xx->2xx|status
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# Wisdom QuickResponse state not persisted: create/get/list drop modeled members, update and delete ignored

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateQuickResponse

## Description
The emulator's quick-response resource keeps only a small part of the request and never applies mutations: optional request members are dropped on create and therefore absent from every read, UpdateQuickResponse has no effect on stored state, and DeleteQuickResponse does not remove the resource. 6 causes, all in the wisdom service, all in the quick-response state/handlers.

Cause 1 of 6: CreateQuickResponse response omits the content and the optional members it was given, and reports the wrong contentType.
AWS: CreateQuickResponse answers 200 with QuickResponseData echoing what was stored: contents (QuickResponseContents.plainText or .markdown, a QuickResponseContentProvider union with member 'content' carrying the submitted text), contentType exactly as supplied (Smithy: CreateQuickResponse.contentType target QuickResponseType @pattern ^(application/x\.quickresponse;format=(plain|markdown))$, and QuickResponseData.contentType is @required), isActive, channels (Channels, @uniqueItems; supported value 'Chat'), language (LanguageCode, length 2..5, e.g. en_US), and lastModifiedBy (GenericArn — the ARN of the calling identity, taken from the signed request, not a literal). attributeList: recorded behavior only — AWS returns it as an empty list when the quick response has no attributes; the model on disk does not declare it.
Emulator: answers 200 with contentType defaulted to 'application/x.quickresponse;format=plain' even when the request supplied the markdown type and markdown content, and with contents, isActive, channels, language, lastModifiedBy and attributeList all absent.
Why: a client that creates a markdown quick response reads back a plain-text one with no body; it cannot render the response, cannot tell which channels or language it applies to, and sees no author. Round-tripping create -> use is impossible.
Fix: the CreateQuickResponse handler's quick-response state record (store every supplied member, including the content union and contentType) and the QuickResponseData projection it returns.
Traces: wisdom-quick-response-error-paths#setup#3.

Cause 2 of 6: CreateQuickResponse drops channels, language, attributeList and lastModifiedBy even for a plain-text create.
AWS: as cause 1 — the stored record carries channels, language and lastModifiedBy, and attributeList is present (empty when unused).
Emulator: those four members are absent from the 200 body.
Why: same loss of request fidelity on the common path; clients treating channels/language as required configuration see them unset.
Fix: same handler/state as cause 1.
Traces: wisdom-quick-response-lifecycle#task#3.

Cause 3 of 6: GetQuickResponse omits the same members.
AWS: GetQuickResponse returns the full QuickResponseData for the stored resource, including channels, language, lastModifiedBy and attributeList.
Emulator: 200 with those members absent — the read mirrors the truncated state written by cause 2.
Why: a client cannot recover the quick response's channel/language configuration or author at any point in its life.
Fix: quick-response state (what is stored) plus the GetQuickResponse projection.
Traces: wisdom-quick-response-lifecycle#task#4.

Cause 4 of 6: ListQuickResponses summaries omit channels and lastModifiedBy.
AWS: each QuickResponseSummary carries channels and lastModifiedBy alongside the identity/status members.
Emulator: 200, summaries present, but both members absent.
Why: listing cannot be used to filter quick responses by channel or to show who last changed them, forcing a per-item GetQuickResponse that is equally lossy.
Fix: the ListQuickResponses summary serializer plus the state it reads.
Traces: wisdom-quick-response-lifecycle#task#5.

Cause 5 of 6: UpdateQuickResponse does not apply its changes to stored state.
AWS: UpdateQuickResponse (POST /knowledgeBases/{knowledgeBaseId}/quickResponses/{quickResponseId}) persists the supplied members — here isActive=false and a new description — and a following GetQuickResponse returns isActive false and that description (Smithy: UpdateQuickResponse.isActive -> Boolean, description -> QuickResponseDescription length 1..255, plus removeDescription/removeShortcutKey/removeGroupingConfiguration for explicit clears).
Emulator: the subsequent GetQuickResponse still reports isActive true and no description at all, so the update was accepted and discarded.
Why: deactivating or re-describing a quick response silently does nothing; a client that deactivates a response still sees it served as active.
Fix: the UpdateQuickResponse handler must mutate the stored quick-response record (and set lastModifiedTime/lastModifiedBy), not just answer 200.
Traces: wisdom-quick-response-lifecycle#task#7.

Cause 6 of 6: DeleteQuickResponse does not remove the resource.
AWS: after DeleteQuickResponse (DELETE .../quickResponses/{quickResponseId}, 204), GetQuickResponse for that id answers 404 ResourceNotFoundException with header x-amzn-errortype: ResourceNotFoundException and body message 'QuickResponse does not exist.' (fixed text; no variable part). Smithy: GetQuickResponse errors include ResourceNotFoundException @httpError 404, doc 'The specified resource does not exist.'
Emulator: GetQuickResponse after the delete answers 200 with the quick-response body — no error type header, no message.
Why: delete is not observable, so teardown/idempotency logic and any 'exists?' probe built on GetQuickResponse is wrong; deleted content keeps being served to agents.
Fix: the DeleteQuickResponse handler must remove the record from quick-response state, and GetQuickResponse must raise ResourceNotFoundException for a missing id.
Traces: wisdom-quick-response-lifecycle#teardown#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, score 65.9%).

Cause 1 — wisdom-quick-response-error-paths#setup#3, CreateQuickResponse, expected 200 / actual 200:
  $.quickResponse.contentType: expected 'application/x.quickresponse;format=markdown', got 'application/x.quickresponse;format=plain'
  $.quickResponse.contents.markdown: missing (expected {'content': 'First quick response'})
  $.quickResponse.attributeList: missing (expected [])
  $.quickResponse.channels: missing (expected ['Chat'])
  $.quickResponse.isActive: missing (expected true)
  $.quickResponse.language: missing (expected 'en_US')
  $.quickResponse.lastModifiedBy: missing (expected the caller ARN; this environment recorded 'arn:aws:iam::000000000000:user/emulator')

Cause 2 — wisdom-quick-response-lifecycle#task#3, CreateQuickResponse, 200/200: $.quickResponse.attributeList, .channels, .language, .lastModifiedBy all missing.

Cause 3 — wisdom-quick-response-lifecycle#task#4, GetQuickResponse, 200/200: same four members missing.

Cause 4 — wisdom-quick-response-lifecycle#task#5, ListQuickResponses, 200/200: $.quickResponseSummaries[<the created quick response>].channels and .lastModifiedBy missing.

Cause 5 — wisdom-quick-response-lifecycle#task#7, GetQuickResponse after UpdateQuickResponse, 200/200:
  $.quickResponse.isActive: expected false, got true
  $.quickResponse.description: missing (expected the description the update supplied, recorded as 'Now inactive')
  plus .attributeList/.channels/.language/.lastModifiedBy missing as in cause 3.

Cause 6 — wisdom-quick-response-lifecycle#teardown#9, GetQuickResponse after DeleteQuickResponse:
  status: expected 404, got 200
  $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException')
  $.message: missing (expected 'QuickResponse does not exist.')

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: POST /knowledgeBases/{knowledgeBaseId}/quickResponses with contentType 'application/x.quickresponse;format=markdown', content, channels ['Chat'], language 'en_US', isActive true — the 200 body reports contentType '...format=plain' and omits contents, channels, language, isActive, lastModifiedBy, attributeList. Fixture wisdom-quick-response-error-paths step setup#3.
3. Cause 2: same call with plain text — channels, language, lastModifiedBy and attributeList are still omitted. Fixture wisdom-quick-response-lifecycle step task#3.
4. Cause 3: GET /knowledgeBases/{knowledgeBaseId}/quickResponses/{quickResponseId} — the same four members are absent. Fixture wisdom-quick-response-lifecycle step task#4.
5. Cause 4: GET /knowledgeBases/{knowledgeBaseId}/quickResponses — the summary for that quick response lacks channels and lastModifiedBy. Fixture wisdom-quick-response-lifecycle step task#5.
6. Cause 5: POST .../quickResponses/{quickResponseId} (UpdateQuickResponse) with isActive=false and a new description, then GET it — isActive is still true and description is absent. Fixture wisdom-quick-response-lifecycle step task#7.
7. Cause 6: DELETE .../quickResponses/{quickResponseId}, then GET the same id — the emulator answers 200 with the body instead of 404 ResourceNotFoundException. Fixture wisdom-quick-response-lifecycle step teardown#9.
