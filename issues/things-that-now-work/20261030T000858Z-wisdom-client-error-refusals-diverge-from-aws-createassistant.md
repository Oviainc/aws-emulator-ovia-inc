Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wisdom.create-assistant-invalid-type-enum-rejected wisdom.create-content-against-quick-responses-kb-rejected wisdom.tag-resource-nonexistent-arn-rejected
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom.create-assistant-invalid-type-enum-rejected|run#5|AssertionError|assert message == "Invalid request body", f"expected the invalid-request-body message, got {message!r}"
Issue-Key: wisdom.create-content-against-quick-responses-kb-rejected|run#7|AssertionError|assert message == "Unsupported ContentType for KnowledgeBaseType: quick_replies", (
Issue-Key: wisdom.tag-resource-nonexistent-arn-rejected|run#11|AssertionError|assert code == "NotFoundException", f"code={code}"
Issue-Commit: de414c514180eddfb96a534a3f6304a808cc952a

# wisdom: client-error refusals diverge from AWS — CreateAssistant enum rejection message, StartContentUpload knowledge-base-type message, and TagResource not-found error code (3 causes)

> _Note: this issue was identified against the code base at commit `de414c514180eddfb96a534a3f6304a808cc952a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssistant

## Description
Three wisdom refusals are classified or worded by the emulator rather than taken from AWS. In every case the emulator does refuse the request, so the operations are reachable (the scaffold-stub issues for TagResource and StartContentUpload are already marked fixed); what diverges is the error code and the message text a client branches on. 3 causes, all in the wisdom service. The open issue 20261029T192454Z-wisdom-error-message-text-diverges-from-aws-wording.md covers GetQuickResponse and NotifyRecommendationsReceived only — it does NOT cover any of the three refusal sites below, which are distinct handlers and distinct messages.

Cause 1 of 3: CreateAssistant with a `type` outside the AssistantType enum answers the generic framework constraint sentence instead of AWS's fixed refusal message.
AWS: Smithy: CreateAssistant.type is @required and targets the AssistantType enum (only value AGENT); the operation models ValidationException @httpError 400. Recorded behavior for an out-of-enum value: HTTP 400 with error code BadRequestException and message exactly `Invalid request body` — the whole message is fixed text, no part of it is taken from the request (the offending value and the member name are NOT echoed), and no assistant is created for that name.
Emulator: same error code (BadRequestException) but the message is the generic server-side constraint rendering `1 validation error detected: Value 'NOT_A_REAL_ASSISTANT_TYPE' at 'type' failed to satisfy constraint: Member must satisfy enum value set: [AGENT]` — i.e. a template filled from the request value and member name, which AWS never emits here.
Why: a client or test that matches on the exception message to distinguish a malformed body from other 400s sees a string AWS never produces; the code looks right while the message is from a different service family's validation layer.
Fix: the CreateAssistant path's request-body validation refusal — the message constructed for a modeled-enum violation on this operation (the handler's validation, not the status mapping).
Traces: check wisdom.create-assistant-invalid-type-enum-rejected; handler services/service-com-amazonaws-wisdom-wisdomservice/src/ops/create_assistant.rs and the shared validation path that renders the constraint sentence.

Cause 2 of 3: StartContentUpload against a QUICK_RESPONSES-type knowledge base is refused with an emulator-authored message.
AWS: Smithy: StartContentUpload models ValidationException @httpError 400 ('The input fails to satisfy the constraints specified by a service') and is documented to work only against a CUSTOM knowledge base. Recorded behavior: HTTP 400 ValidationException with message exactly `Unsupported ContentType for KnowledgeBaseType: quick_replies` — fixed text ending in the service-internal knowledge-base-type token for the target knowledge base (here `quick_replies` for a QUICK_RESPONSES knowledge base); only that type token varies with the request's knowledge base.
Emulator: correct code (ValidationException) but message `Content can only be uploaded to a knowledge base of type CUSTOM.` — emulator wording, and it drops the knowledge-base-type token that identifies which type was rejected.
Why: the refusal is the only signal that tells a client which knowledge-base type it pointed at; message-based handling of this upload refusal fails, and the diagnostic value of naming the offending type is lost.
Fix: the StartContentUpload handler's knowledge-base-type gate — the ValidationException message it constructs.
Traces: check wisdom.create-content-against-quick-responses-kb-rejected (the positive control, start-upload-then-CreateContent against a CUSTOM knowledge base, is not in question); handler services/service-com-amazonaws-wisdom-wisdomservice/src/ops/start_content_upload.rs.

Cause 3 of 3: TagResource on a well-formed ARN that names no wisdom resource answers the wrong error code and message.
AWS: Smithy: TagResource (POST /tags/{resourceArn}, resourceArn @httpLabel @pattern ^arn:[a-z-]*?:wisdom:...) models ResourceNotFoundException @httpError 404 and TooManyTagsException @httpError 400. Recorded behavior only for the unknown-ARN case: AWS answers error code `NotFoundException` with message exactly `Resource was not found` — both are fixed text, nothing from the request is substituted into the message. Hypothesis: the tagging front end of this service answers with its own NotFoundException shape rather than the modeled ResourceNotFoundException; the recording, not the model, is authoritative here.
Emulator: refuses, but with error code `ResourceNotFoundException` (and its own not-found message), so the code a client catches never matches.
Why: a client that catches NotFoundException around TagResource/UntagResource — the documented-by-behavior class for an unknown ARN — does not catch the emulator's exception at all and the call propagates as an unexpected error; retry/skip logic for 'resource gone' never triggers.
Fix: the TagResource handler's unknown-ARN refusal — the error shape it selects for an ARN that resolves to no resource, and its message text.
Traces: check wisdom.tag-resource-nonexistent-arn-rejected (the fabricated ARN is the real assistant ARN with the id replaced by the all-zero UUID; the real resource must be left untagged, and tagging the real ARN must still succeed); handler services/service-com-amazonaws-wisdom-wisdomservice/src/ops/tag_resource.rs.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout (wisdom 26/47 checks passed).

Cause 1 — wisdom.create-assistant-invalid-type-enum-rejected [mismatch]:
  AssertionError: expected the invalid-request-body message, got "1 validation error detected: Value 'NOT_A_REAL_ASSISTANT_TYPE' at 'type' failed to satisfy constraint: Member must satisfy enum value set: [AGENT]"
  expected code BadRequestException + message 'Invalid request body'; code matched, message diverged.

Cause 2 — wisdom.create-content-against-quick-responses-kb-rejected [mismatch]:
  AssertionError: expected the unsupported-content-type message, got 'Content can only be uploaded to a knowledge base of type CUSTOM.'
  expected code ValidationException + message 'Unsupported ContentType for KnowledgeBaseType: quick_replies'; code matched, message diverged.

Cause 3 — wisdom.tag-resource-nonexistent-arn-rejected [mismatch]:
  AssertionError: code=ResourceNotFoundException
  expected code NotFoundException + message 'Resource was not found'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite simparity-behavioral-aws-v1 (wisdom checks).
2. Cause 1: POST /assistants with {"name": <fresh name>, "type": "NOT_A_REAL_ASSISTANT_TYPE"} — the 400 body's message is the generic '1 validation error detected: ...' sentence instead of 'Invalid request body'. Check wisdom.create-assistant-invalid-type-enum-rejected.
3. Cause 2: CreateKnowledgeBase with knowledgeBaseType QUICK_RESPONSES, then POST /knowledgeBases/{id}/upload with contentType text/plain — the 400 ValidationException message reads 'Content can only be uploaded to a knowledge base of type CUSTOM.' instead of 'Unsupported ContentType for KnowledgeBaseType: quick_replies'. Check wisdom.create-content-against-quick-responses-kb-rejected.
4. Cause 3: CreateAssistant, take its assistantArn, replace the trailing id with 00000000-0000-0000-0000-000000000000, then POST /tags/{thatArn} with {"tags": {"k": "v"}} — the refusal carries code ResourceNotFoundException instead of NotFoundException with message 'Resource was not found'. Check wisdom.tag-resource-nonexistent-arn-rejected.
