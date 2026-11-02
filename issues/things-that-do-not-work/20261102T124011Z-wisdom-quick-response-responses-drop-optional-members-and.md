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
Issue-Key: wisdom|GetQuickResponse|2xx->2xx|$.quickResponse.attributeList
Issue-Key: wisdom|GetQuickResponse|2xx->2xx|$.quickResponse.channels
Issue-Key: wisdom|GetQuickResponse|2xx->2xx|$.quickResponse.language
Issue-Key: wisdom|ListQuickResponses|2xx->2xx|$.quickResponseSummaries[name="sim2real-probe-wisdom-qr-ce634326"].channels
Issue-Key: wisdom|UpdateQuickResponse|2xx->2xx|$.quickResponse.attributeList
Issue-Key: wisdom|UpdateQuickResponse|2xx->2xx|$.quickResponse.channels
Issue-Key: wisdom|UpdateQuickResponse|2xx->2xx|$.quickResponse.language
Issue-Commit: afb41014a6bc1e75e8336e677aedbec1633a6643

# Wisdom quick-response responses drop optional members and ignore the request contentType

> _Note: this issue was identified against the code base at commit `afb41014a6bc1e75e8336e677aedbec1633a6643`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateQuickResponse

## Description
Every 200 answer on the wisdom quick-response surface is structurally short of the AWS answer: the members a create supplies for `contentType`, `isActive`, `channels` and `language` never reach the served `QuickResponseData`, and `attributeList` is never served at all. All five replayed quick-response steps diverge the same way, on all four operations that serve a quick response (CreateQuickResponse, GetQuickResponse, UpdateQuickResponse, ListQuickResponses). 5 causes, all in the wisdom service.

Related but NOT covering: the open issue 20261030T000858Z-wisdom-quick-response-content-data-plane-createquickresponse-is.md covers the knowledge-base-type gate, SearchQuickResponses being a stub and UpdateContent's revisionId precondition — it says nothing about members missing from a 200 quick-response body. The issue 20261029T192454Z-wisdom-quickresponse-state-not-persisted-create-get-list.md is RESOLVED (state is now persisted and the handlers do copy these members, see Fix below), yet the members are still absent on the wire, so this is net-new and not covered by it.

Cause 1 of 5: CreateQuickResponse ignores the request's `contentType`, so a markdown quick response is created and served as plain text, and its content lands under the wrong `contents` member.
AWS: Smithy: CreateQuickResponse (POST /knowledgeBases/{knowledgeBaseId}/quickResponses) takes `contentType` (QuickResponseType, pattern ^(application/x\.quickresponse;format=(plain|markdown))$) and answers 200 with QuickResponseData whose `contentType` is @required. The member's own documentation fixes the rule: `application/x.quickresponse;format=plain` is a quick response written in plain text, `application/x.quickresponse;format=markdown` one written in richtext. AWS echoes back the media type the request named, and `QuickResponseContents` carries the content under the member that media type names — `markdown` (a QuickResponseContentProvider union with member `content`) for a markdown quick response, `plainText` for a plain one. Also recorded: the same create returns `isActive` and `language` as the request supplied them.
Emulator: a create that names `contentType` `application/x.quickresponse;format=markdown` answers 200 with `contentType` `application/x.quickresponse;format=plain`, with `$.quickResponse.contents.markdown` absent (the content is served under `plainText` instead), and with `isActive` and `language` absent.
Why: the media type is what tells a client whether the reply body is plain text or richtext, and which `contents` member to read. A client that creates a markdown quick response gets back a plain-text one and finds nothing under `contents.markdown`, so richtext replies are unreadable against the emulator while they work in production — and the reverse once the code ships: markup that the emulator never flagged is rendered verbatim to agents.
Fix: the CreateQuickResponse request path — the handler already honours the member it is given (services/service-com-amazonaws-wisdom-wisdomservice/src/ops/create_quick_response.rs lines 317-319 fall back to the plain media type ONLY when `input.content_type` is `None`, and state.rs's `QuickResponse::contents()` already selects `markdown` vs `plainText` off the stored media type), so `contentType` is being lost before the handler runs: the operation's request-body deserialization / INPUT_SCHEMA wiring is what must carry it.
Traces: wisdom-quick-response-error-paths#setup#3.

Cause 2 of 5: CreateQuickResponse's 200 body omits `channels`, `language` and `attributeList`.
AWS: Smithy: QuickResponseData declares `channels` (Channels, a uniqueItems list of Channel; the supported contact-channel types include `Chat`), `language` (LanguageCode, length 2..5) and `isActive` (Boolean), and CreateQuickResponse accepts all three on the request. AWS echoes each one back on the created resource — a create naming channels `["Chat"]` and language `en_US` answers with exactly those. `attributeList` is recorded behavior only: this checkout's model declares no such member on QuickResponseData, and the recording shows AWS serving `attributeList` as an empty list on the created quick response (and NOT on QuickResponseSummary, which is consistent with the list step below, where only `channels` is missing).
Emulator: answers 200 with `$.quickResponse.channels`, `$.quickResponse.language` and `$.quickResponse.attributeList` all absent, for a create that supplied channels and language.
Why: a client cannot read back the channel scoping or the language of the quick response it just created, so a UI that filters quick responses by contact channel or language sees every response as unscoped, and a client that requires `attributeList` to be present (AWS always serves it, empty when there are no attributes) fails on a missing key.
Fix: the quick-response write/serialize path for these members — the stored record already carries them (services/service-com-amazonaws-wisdom-wisdomservice/src/state.rs, `QuickResponse.channels` / `.language` / `.is_active`, lines 918-947) and `QuickResponse::to_data` already copies them onto the output shape (lines 971-994), so the loss is on the edges: the request deserialization that should populate them (same edge as Cause 1) and the QuickResponseData output serializer. `attributeList` additionally needs a representation on QuickResponseData and a default of the empty list.
Traces: wisdom-quick-response-lifecycle#task#3.

Cause 3 of 5: GetQuickResponse serves the same truncated QuickResponseData — `channels`, `language` and `attributeList` are absent on the read path too.
AWS: Smithy: GetQuickResponse (GET /knowledgeBases/{knowledgeBaseId}/quickResponses/{quickResponseId}, @readonly) answers 200 with the same QuickResponseData shape, so a read reports the channels, language and attribute list of the stored quick response — the read and the create of one record agree member for member.
Emulator: 200 with `$.quickResponse.channels`, `$.quickResponse.language` and `$.quickResponse.attributeList` absent, on a quick response created with channels `["Chat"]` and language `en_US`; the divergence repeats on the second read of the same record later in the fixture.
Why: the read path is where a client reloads a quick response it did not create, so the channel and language scoping is invisible to any client that did not keep the create response — and because the create is equally short (Cause 2), the values are unobservable anywhere.
Fix: same edge as Cause 2 — GetQuickResponse's handler (src/ops/get_quick_response.rs lines 180-182) returns `quick_response.to_data()` unchanged, so fixing the stored-record→wire serialization of QuickResponseData fixes this read with it; no per-operation logic is involved.
Traces: wisdom-quick-response-lifecycle#task#4 (and the repeat read at task#7 in the same fixture).

Cause 4 of 5: ListQuickResponses summaries omit `channels`.
AWS: Smithy: ListQuickResponses (GET /knowledgeBases/{knowledgeBaseId}/quickResponses, @readonly, paginated on quickResponseSummaries) answers 200 with `quickResponseSummaries` (@required) of QuickResponseSummary, which declares `channels` (Channels, uniqueItems) and `isActive` but — unlike QuickResponseData — no `language`. A listed quick response carries the channels it was created with, so the summary of a quick response scoped to `["Chat"]` reports `["Chat"]`.
Emulator: the summary for the listed quick response is served with `channels` absent. The recording confirms the shape split: on this step only `channels` is missing, and no `attributeList` or `language` is expected on a summary.
Why: list is the enumeration path an agent-assist UI pages through to decide which quick responses apply to the current contact channel; with `channels` absent, every quick response looks applicable to every channel and the per-channel filter silently matches nothing or everything.
Fix: the QuickResponseSummary serializer — `QuickResponse::to_summary` already copies `channels` from the stored record (src/state.rs lines 1001-1017), so the member is dropped on the state→wire edge for the summary shape, the same defect as Cause 2 one shape over.
Traces: wisdom-quick-response-lifecycle#task#5.

Cause 5 of 5: UpdateQuickResponse's 200 body is truncated the same way — `channels`, `language` and `attributeList` absent.
AWS: Smithy: UpdateQuickResponse (POST /knowledgeBases/{knowledgeBaseId}/quickResponses/{quickResponseId}) accepts `channels`, `language` and `isActive` and answers 200 with the updated QuickResponseData, so the update's answer reports the post-update channels and language — a member the update did not name keeps the value the record held, and a member it did name is reported with its new value.
Emulator: 200 with `$.quickResponse.channels`, `$.quickResponse.language` and `$.quickResponse.attributeList` absent, so an update cannot be confirmed to have taken effect on any of them.
Why: update-then-read is how a client confirms a write landed; with these members missing from the answer, a client that sets a quick response's channels or language has no way to tell whether the write was applied, and a read-modify-write loop silently drops them.
Fix: same state→wire serialization of QuickResponseData as Causes 2 and 3 — the update handler already applies and persists all three members (src/ops/update_quick_response.rs lines 333-341) and then answers from the stored record (lines 351-353), so no handler logic change is needed for this cause.
Traces: wisdom-quick-response-lifecycle#task#6.

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against the emulator built from this checkout (14 fixtures replayed, 73/91 steps passed, 8 divergence clusters; the other 3 clusters are already owned by open issues). All five steps below answered HTTP 200 where AWS answered HTTP 200 — the divergence is entirely in the response body. The quick-response name, ids, ARNs, account, region and timestamps in these fixtures are per-run values and are not requirements.

Cause 1 — wisdom-quick-response-error-paths#setup#3, CreateQuickResponse, expected 200 / actual 200 [mismatch]:
  $.quickResponse.contentType: value_mismatch (expected 'application/x.quickresponse;format=markdown', got 'application/x.quickresponse;format=plain')
  $.quickResponse.contents.markdown: missing (expected {'content': <the content the create supplied>}, got None)
  $.quickResponse.attributeList: missing (expected [], got None)
  $.quickResponse.channels: missing (expected ['Chat'], got None)
  $.quickResponse.isActive: missing (expected True, got None)
  $.quickResponse.language: missing (expected 'en_US', got None)

Cause 2 — wisdom-quick-response-lifecycle#task#3, CreateQuickResponse, expected 200 / actual 200 [mismatch]:
  $.quickResponse.attributeList: missing (expected [], got None)
  $.quickResponse.channels: missing (expected ['Chat'], got None)
  $.quickResponse.language: missing (expected 'en_US', got None)

Cause 3 — wisdom-quick-response-lifecycle#task#4, GetQuickResponse, expected 200 / actual 200 [mismatch] (cluster also covers task#7, the second read of the same record):
  $.quickResponse.attributeList: missing (expected [], got None)
  $.quickResponse.channels: missing (expected ['Chat'], got None)
  $.quickResponse.language: missing (expected 'en_US', got None)

Cause 4 — wisdom-quick-response-lifecycle#task#5, ListQuickResponses, expected 200 / actual 200 [mismatch]:
  $.quickResponseSummaries[name=<the created quick response's name>].channels: missing (expected ['Chat'], got None)

Cause 5 — wisdom-quick-response-lifecycle#task#6, UpdateQuickResponse, expected 200 / actual 200 [mismatch]:
  $.quickResponse.attributeList: missing (expected [], got None)
  $.quickResponse.channels: missing (expected ['Chat'], got None)
  $.quickResponse.language: missing (expected 'en_US', got None)

Code read while diagnosing (handlers already do the right thing, which is why the loss is placed on the request-deserialize / response-serialize edges): src/ops/create_quick_response.rs:311-375, src/ops/get_quick_response.rs:180-182, src/ops/update_quick_response.rs:309-353, src/state.rs:918-1018.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire trace set simparity-wire-aws-v1 (fixtures wisdom-quick-response-error-paths and wisdom-quick-response-lifecycle).
2. Cause 1: CreateKnowledgeBase with knowledgeBaseType QUICK_RESPONSES, then POST /knowledgeBases/{knowledgeBaseId}/quickResponses with name, content {"content": "<text>"}, contentType 'application/x.quickresponse;format=markdown', isActive true, channels ['Chat'] and language 'en_US'. The 200 body reports contentType 'application/x.quickresponse;format=plain', serves the content under contents.plainText instead of contents.markdown, and omits attributeList, channels, isActive and language. Fixture wisdom-quick-response-error-paths, step setup#3.
3. Cause 2: the same create with channels ['Chat'] and language 'en_US' (no contentType) — the 200 body omits attributeList, channels and language. Fixture wisdom-quick-response-lifecycle, step task#3.
4. Cause 3: GET /knowledgeBases/{knowledgeBaseId}/quickResponses/{quickResponseId} for that quick response — the 200 body omits attributeList, channels and language. Fixture wisdom-quick-response-lifecycle, steps task#4 and task#7.
5. Cause 4: GET /knowledgeBases/{knowledgeBaseId}/quickResponses — the summary for that quick response omits channels. Fixture wisdom-quick-response-lifecycle, step task#5.
6. Cause 5: POST /knowledgeBases/{knowledgeBaseId}/quickResponses/{quickResponseId} updating the quick response — the 200 body omits attributeList, channels and language. Fixture wisdom-quick-response-lifecycle, step task#6.
