Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wisdom.assistant-association-client-token-idempotent-conflict-rejected wisdom.assistant-association-delete-transitions-to-not-found wisdom.assistant-association-multiple-distinct-knowledge-bases-allowed wisdom.assistant-create-active-lifecycle wisdom.assistant-delete-removes-and-not-found wisdom.assistant-knowledge-base-content-session-end-to-end-workflow wisdom.concurrent-assistant-association-race-single-winner wisdom.content-delete-removes-and-not-found wisdom.content-metadata-and-linkouturi-persisted wisdom.content-search-finds-created-content wisdom.content-update-persists-title-and-bumps-revision wisdom.content-update-stale-revision-precondition-failed-rejected wisdom.content-upload-and-download-round-trip wisdom.create-assistant-client-token-idempotent-conflict-rejected wisdom.create-assistant-invalid-type-enum-rejected wisdom.create-assistant-with-tags-persisted wisdom.create-content-against-quick-responses-kb-rejected wisdom.create-content-duplicate-name-conflict-rejected wisdom.create-content-invalid-upload-id-not-found wisdom.create-knowledge-base-with-tags-persisted wisdom.create-quick-response-against-custom-kb-rejected wisdom.create-quick-response-invalid-content-type-rejected wisdom.create-session-lifecycle-and-get wisdom.delete-knowledge-base-with-active-association-succeeds wisdom.delete-quick-response-removes-from-search wisdom.get-assistant-not-found wisdom.get-content-summary-matches-full-content-and-omits-url wisdom.get-recommendations-nonexistent-session-rejected wisdom.knowledge-base-create-active-lifecycle wisdom.knowledge-base-delete-removes-and-not-found wisdom.list-assistants-pagination-covers-all-created wisdom.notify-recommendations-received-nonexistent-assistant-rejected wisdom.quick-response-channels-language-fields-persisted wisdom.quick-response-create-created-lifecycle wisdom.quick-response-delete-removes-and-not-found wisdom.quick-response-search-finds-created wisdom.quick-response-shortcut-key-conflict-rejected wisdom.quick-response-shortcut-key-race-concurrency wisdom.search-quick-responses-name-prefix-filter wisdom.tag-resource-list-and-untag-round-trip wisdom.tag-resource-nonexistent-arn-rejected wisdom.too-many-tags-rejected wisdom.untag-resource-nonexistent-arn-rejected wisdom.untag-resource-removes-tag wisdom.update-content-removes-override-link-out-uri wisdom.update-content-revisionid-race-concurrency wisdom.update-quick-response-concurrent-race
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom|CreateAssistant|AmbiguousService
Issue-Key: wisdom|CreateKnowledgeBase|AmbiguousService
Issue-Key: wisdom.create-assistant-invalid-type-enum-rejected|run#4|AssertionError|assert code == "BadRequestException", f"code={code}"
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# All Wisdom requests rejected with AmbiguousService: two emulated services claim the 'wisdom' signing name (wisdom + qconnect)

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssistant

## Description
Cause 1 of 1: the emulator's request router cannot attribute a request addressed to the Wisdom endpoint, because two separately modelled services in this repo resolve to the same routing key, and it fails the request closed instead of resolving it. Every single behavioral check for `wisdom` (47/47) dies on its very first API call, so no Wisdom operation is reachable at all.

AWS: Amazon Connect Wisdom is reachable at the `wisdom.{region}.amazonaws.com` endpoint and signs with SigV4 signing name `wisdom`. Smithy (services/service-com-amazonaws-wisdom-wisdomservice/smithy-model/com-amazonaws-wisdom-wisdomservice.json, query=service_info): service shape `WisdomService`, version 2020-10-19, sdkId `Wisdom`, arnNamespace `wisdom`, sigv4Name `wisdom`, protocol restJson1, 41 operations. The repo also carries a SECOND model, services/service-com-amazonaws-qconnect-wisdomservice/smithy-model/com-amazonaws-qconnect-wisdomservice.json, whose service_info is service shape `WisdomService`, version 2020-10-19, sdkId `QConnect`, arnNamespace `wisdom`, sigv4Name `wisdom`, 94 operations — i.e. Amazon Q Connect, the successor API, deliberately shares Wisdom's endpoint prefix and signing name. On real AWS this is not ambiguous: a request is dispatched by its HTTP method + request URI (restJson1 has no X-Amz-Target), so `POST /assistants` (CreateAssistant) and `POST /knowledgeBases` (CreateKnowledgeBase) are served normally; the shared signing name is never a reason to refuse service. AWS NEVER returns an error code `AmbiguousService` — it is not in either model's error set.

Emulator: every Wisdom request is answered with a synthetic routing error, retried to exhaustion by botocore and surfaced as `ClientError (AmbiguousService) ... (reached max retries: 3): 2 emulated services all claim requests for 'wisdom': wisdom, wisdom; the request is rejected rather than served by an arbitrary one`. Both claimants print as `wisdom`, consistent with the router keying on sigv4Name/endpointPrefix/arnNamespace (all `wisdom` for both models) rather than on sdkId or the operation's HTTP route. The error is also apparently retryable, which multiplies each failure into 4 requests. Observed on CreateAssistant and CreateKnowledgeBase — whichever operation the check calls first — so it is route-level, not operation-level. The one non-`error` verdict, wisdom.create-assistant-invalid-type-enum-rejected, is the same cause seen through an assertion: it expected `BadRequestException` for a type outside the AssistantType enum and got `code=AmbiguousService`.

Why: a client gets an error code that does not exist in the AWS API for any Wisdom call, on every call, after 4 attempts. Nothing can be created, read, listed, tagged or deleted; retry logic burns its budget on a non-transient condition; and error-handling paths that branch on modelled codes (ResourceNotFoundException, ConflictException, PreconditionFailedException, BadRequestException, ValidationException) never see them, so negative-path behavior is unverifiable too. The service is effectively 100% down while reporting as bootstrapped.

Fix: the shared-routing-key resolution in the request router / service-registry dispatch layer, not in any Wisdom handler. The registry must admit two services that share an endpoint prefix and signing name and pick one deterministically — by distinguishing the registration identity (sdkId `Wisdom` vs `QConnect`) and/or by matching the concrete restJson1 method+URI against each registered service's routes — instead of counting claimants and refusing. Resolving the duplicate claim is what unblocks all 47 checks; until then none of the Wisdom contract behavior they assert has been exercised even once. Note the related open issue 20261021T231833Z-bootstrap-service-com-amazonaws-qconnect-wisdomservice.md (bootstrap the QConnect/Wisdom emulator) — it covers standing up qconnect, NOT this routing collision, which breaks the already-bootstrapped wisdom service and must be fixed so wisdom stays reachable however qconnect is registered.

Traces: all 47 wisdom checks, listed in trace_ids; representative ids wisdom.assistant-create-active-lifecycle (CreateAssistant) and wisdom.knowledge-base-create-active-lifecycle (CreateKnowledgeBase), plus the mismatch variant wisdom.create-assistant-invalid-type-enum-rejected.

## Evidence
Suite summary: wisdom 0/47 passed (0%), 47 failed (46 error, 1 mismatch). All 47 failures carry the same AmbiguousService detail.

Expected (AWS, restJson1, sdkId Wisdom, version 2020-10-19):
  POST /assistants {"name":"...","type":"AGENT"} -> 200 with {"assistant":{...assistantId, status ACTIVE...}}
  POST /assistants {"name":"...","type":"NOT_A_REAL_ASSISTANT_TYPE"} -> 4xx with Error.Code = BadRequestException

Actual (emulator), for every Wisdom operation attempted:
  botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateAssistant operation (reached max retries: 3): 2 emulated services all claim requests for 'wisdom': wisdom, wisdom; the request is rejected rather than served by an arbitrary one
  ... identical message with 'when calling the CreateKnowledgeBase operation' for checks whose first call is CreateKnowledgeBase
  wisdom.create-assistant-invalid-type-enum-rejected: AssertionError: code=AmbiguousService (expected BadRequestException)

Error code `AmbiguousService` appears in neither model's error set; it is emulator-internal. Both claimants are reported with the identical token `wisdom`.

Model evidence for the collision (smithy_query query=service_info):
  services/service-com-amazonaws-wisdom-wisdomservice/...: sdkId Wisdom,   sigv4Name wisdom, arnNamespace wisdom, WisdomService 2020-10-19, 41 ops
  services/service-com-amazonaws-qconnect-wisdomservice/...: sdkId QConnect, sigv4Name wisdom, arnNamespace wisdom, WisdomService 2020-10-19, 94 ops

## Steps to Reproduce
1. Boot the emulator from mainline (both the wisdom and qconnect service crates are present in the registry).
2. Point a botocore/boto3 `wisdom` client at the emulator endpoint with any region and credentials.
3. Call ANY Wisdom operation, e.g. create_assistant(name='sim2real-asst-1', type='AGENT') or create_knowledge_base(name='sim2real-kb-1', knowledgeBaseType='CUSTOM').
4. Observe the call is retried 4 times and then raises ClientError with code `AmbiguousService` and the message '2 emulated services all claim requests for \'wisdom\': wisdom, wisdom; ...' instead of a 200 response.
5. Equivalently, replay any of the 47 wisdom behavioral checks in trace_ids — each fails on its first API call.
