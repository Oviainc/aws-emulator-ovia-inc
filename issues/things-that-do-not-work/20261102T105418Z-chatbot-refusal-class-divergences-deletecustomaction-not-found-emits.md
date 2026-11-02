Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chatbot.custom-action-create-get-update-delete-lifecycle chatbot.custom-action-delete-removes-resource chatbot.tag-resource-exceeds-max-tags-rejected
Issue-Shape-Id: com.amazonaws.chatbot#WheatleyOrchestration_20171011
Issue-Key: chatbot.custom-action-create-get-update-delete-lifecycle|run#19|AssertionError|assert message2 is None, f"unexpected message={message2!r}"
Issue-Key: chatbot.custom-action-delete-removes-resource|run#14|AssertionError|assert message2 is None, f"expected the second DeleteCustomAction to carry no message, got {message2!r}"
Issue-Key: chatbot.tag-resource-exceeds-max-tags-rejected|run#13|AssertionError|assert (code, message) == ("InvalidRequestException", None), f"unexpected refusal {hit!r}"
Issue-Commit: e9e518e3fed0eb72155b08c250942d32d0463cfb

# Chatbot refusal-class divergences: DeleteCustomAction not-found emits a surplus Message, and the TagResource tag-cap refusal uses the wrong error code

> _Note: this issue was identified against the code base at commit `e9e518e3fed0eb72155b08c250942d32d0463cfb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCustomAction

## Description
Two net-new root causes in the chatbot service, both on refusal paths that the emulator answers (the operations are implemented and the happy paths pass) but renders with the wrong error class or the wrong error body. Model grounding is services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011/smithy-model/com-amazonaws-chatbot-wheatleyorchestration-20171011.json (restJson1, sdkId chatbot).

Cause 1 of 2: DeleteCustomAction on an already-deleted / never-created custom action returns ResourceNotFoundException WITH a Message member, where AWS sends that refusal with no message at all.
AWS: Smithy: DeleteCustomAction (POST /delete-custom-action, success code 204, @idempotent) takes only CustomActionArn (required, @length 1..1011, pattern ^arn:aws:chatbot:[A-Za-z0-9_/.-]{0,63}:[A-Za-z0-9_/.-]{0,63}:custom-action/[a-zA-Z0-9_-]{1,64}$) and models errors InternalServiceError (500), InvalidRequestException (400), ResourceNotFoundException (errorKind client, httpError 404), UnauthorizedException (403). structure ResourceNotFoundException { Message: ErrorMessage } — Message is OPTIONAL, so the model permits an empty error body, and for the DELETE path AWS exercises that: recorded behavior only — the recorded 404 for a delete of an absent custom action carries x-amzn-errortype ResourceNotFoundException and NO Message member. The wording is per-operation on this resource: GetCustomAction's not-found message is the fixed text `sdk action [` … `] not found for account [` … `]` (the bracketed parts are the action name from the request's CustomActionArn and the caller's account id), and the replay confirms the emulator already gets GetCustomAction right — both checks assert that message and both get past it. DeleteCustomAction simply has no message.
Emulator: status 404 and error code ResourceNotFoundException are correct, but the body additionally carries Message = the fixed text `Sdk action with id [` … `] does not exist in account [` … `]`, i.e. the UpdateCustomAction-shaped formatter reused for delete. In the replay output the bracketed parts were the requested action name and the emulator's account id — those are per-run values filling the template, not part of the requirement; the requirement violated is the member's PRESENCE (expected absent, got a string).
Why: a client that branches on the presence of a message in a 404 (treating a message-bearing not-found as a different condition from a bare one, or asserting that delete is message-silent while get/update are not) takes the wrong branch, and recorded-response/error-shape comparisons against AWS fail on the surplus key. It also masks the real requirement that Delete, Get and Update each render not-found differently rather than sharing one string.
Fix: the chatbot DeleteCustomAction handler's not-found branch / its error serializer — raise ResourceNotFoundException with no message for this operation instead of reusing the shared `Sdk action with id [...] does not exist in account [...]` formatter, so the optional Message member is omitted from the serialized body.
Traces: chatbot.custom-action-create-get-update-delete-lifecycle, chatbot.custom-action-delete-removes-resource.
Related open issue, which does NOT carry these behavioral check ids: 20261102T062656Z-chatbot-deletecustomaction-not-found-error-emits-a-message.md describes the same surplus-Message defect from the WIRE corpus (fixture chatbot-resource-not-found-error-paths#task#2); the two behavioral checks here are additional, independently failing evidence of it and must be re-replayed to confirm the fix. 20261030T215059Z-chatbot-custom-action-read-and-update-paths-drop.md covers the opposite direction on UpdateCustomAction/GetCustomAction and does not cover delete's body; 20261029T220801Z-chatbot-deletecustomaction-stub.md only asks for the operation to exist.

Cause 2 of 2: TagResource past the 200-tag cap is refused with TooManyTagsException and a message, where AWS refuses it with InvalidRequestException and no message.
AWS: Smithy: TagResource (POST /tag-resource) takes ResourceARN (required, @length 1..1011, pattern ^arn:aws:(wheatley|chatbot):...) and Tags (required, TagList, @length min 0 max 200, dataClassification tagging), and models errors InternalServiceError (500), ResourceNotFoundException (404), ServiceUnavailableException (429), TooManyTagsException (errorKind client, httpError 400, doc "The supplied list of tags contains too many tags."). InvalidRequestException (errorKind client, httpError 400, doc "Your request input doesn't meet the constraints required by AWS Chatbot.") is defined in the model but is not bound to TagResource, yet recorded behavior only: AWS refuses the request that would take a resource past 200 stored tags with error code InvalidRequestException at HTTP 400 and NO message member. Hypothesis: the 200 in the model is the per-REQUEST list length, while the per-RESOURCE total is enforced as a generic request-constraint violation rather than as the list-shape error. The requirement is the refusal class: code InvalidRequestException, no message.
Emulator: refuses the tag that would make the 201st stored tag with code TooManyTagsException and message `a resource carries at most 200 tags, and this request would leave 201` — a code AWS does not use for this condition, plus a message AWS does not send at all. (The 200/201 figures are the cap and the resulting count from the request and the emulator's state; the whole string is emulator-authored text with no AWS counterpart.) The cap itself and the no-partial-application behaviour are correct — the check got past the count assertions and failed only on the refusal class.
Why: a client that retries or reports on tag-cap failures keyed on the AWS error code never matches TooManyTagsException on this operation, so cap-exhaustion handling (surfacing a quota error, pruning tags, aborting) is not triggered; and a client reading error.Message on a 400 sees emulator-authored prose where AWS sends none.
Fix: the chatbot TagResource handler's tag-cap validation — when the merged tag set would exceed the per-resource cap, raise InvalidRequestException (400) with no message instead of TooManyTagsException with a formatted message.
Traces: chatbot.tag-resource-exceeds-max-tags-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout: 26/42 passed, 7 mismatched, 9 errored, 0 skipped. All 42 checks are chatbot. Of the 16 failures, 10 are marked ALREADY FILED (blocked by unimplemented AssociateToConfiguration, ListAssociations, ListCustomActions, ListMicrosoftTeamsChannelConfigurations stubs) and 3 are marked OWNED by 20261029T220801Z-chatbot-association-authorization-and-refusal-class-divergences-across.md; the 3 below are the remainder.

Cause 1 — DeleteCustomAction surplus Message:
[mismatch] chatbot.custom-action-create-get-update-delete-lifecycle
  check_chatbot_custom_action_create_get_update_delete_lifecycle.py:65
  assert message2 is None  (comment in the check: "AWS's second-delete refusal carries no message at all.")
  AssertionError: unexpected message='Sdk action with id [<actionName>] does not exist in account [<accountId>]'
  Note: the preceding assertions passed — code2 == 'ResourceNotFoundException' is correct, and GetCustomAction's message `sdk action [<actionName>] not found for account [<accountId>]` matched exactly.
[mismatch] chatbot.custom-action-delete-removes-resource
  check_chatbot_custom_action_delete_removes_resource.py:53
  AssertionError: expected the second DeleteCustomAction to carry no message, got 'Sdk action with id [<actionName>] does not exist in account [<accountId>]'
  Expected: 404, x-amzn-errortype ResourceNotFoundException, body with no Message member.
  Actual:   404, ResourceNotFoundException, body with Message populated by the UpdateCustomAction-shaped formatter.

Cause 2 — TagResource cap refusal class:
[mismatch] chatbot.tag-resource-exceeds-max-tags-rejected
  check_chatbot_tag_resource_exceeds_max_tags_rejected.py:64
  assert (code, message) == ("InvalidRequestException", None)  (comment in the check: "AWS's tag-cap refusal carries no message at all.")
  AssertionError: unexpected refusal ('TooManyTagsException', 'a resource carries at most 200 tags, and this request would leave 201')
  Expected: 400, error code InvalidRequestException, no message.
  Actual:   400, error code TooManyTagsException, message 'a resource carries at most 200 tags, and this request would leave 201'.
  Earlier assertions in the same check passed: 200 tags were accepted in batches of 10, ListTagsForResource returned exactly the accepted count, and the rejected key was not stored — only the refusal class diverges.

Model grounding: ResourceNotFoundException { Message: ErrorMessage } (Message optional, httpError 404); TagResource errors = InternalServiceError|ResourceNotFoundException|ServiceUnavailableException|TooManyTagsException, Tags @length max 200; InvalidRequestException httpError 400.

## Steps to Reproduce
Prerequisite: boot the emulator from this checkout and point a botocore chatbot client at it.

Cause 1 (DeleteCustomAction surplus Message) — replay chatbot.custom-action-delete-removes-resource or chatbot.custom-action-create-get-update-delete-lifecycle, or by hand:
1. CreateCustomAction with any ActionName and Definition={"CommandText": "echo transient"}; keep the returned CustomActionArn.
2. DeleteCustomAction(CustomActionArn=arn) — succeeds.
3. GetCustomAction(CustomActionArn=arn) — observe 404 ResourceNotFoundException with message `sdk action [<ActionName>] not found for account [<account>]` (correct today).
4. DeleteCustomAction(CustomActionArn=arn) a second time.
5. Observe 404 with x-amzn-errortype ResourceNotFoundException and a body containing Message = `Sdk action with id [<ActionName>] does not exist in account [<account>]`.
6. Expected: 404 ResourceNotFoundException with no Message member in the body.

Cause 2 (TagResource cap refusal class) — replay chatbot.tag-resource-exceeds-max-tags-rejected, or by hand:
1. CreateCustomAction and keep its CustomActionArn.
2. TagResource(ResourceARN=arn, Tags=[...]) repeatedly in batches of 10 distinct keys until 200 tags are stored; ListTagsForResource confirms 200.
3. TagResource(ResourceARN=arn, Tags=[{"TagKey": "k200", "TagValue": "v200"}]) — one brand-new key past the cap.
4. Observe the refusal: error code TooManyTagsException with message 'a resource carries at most 200 tags, and this request would leave 201'.
5. Expected: HTTP 400 with error code InvalidRequestException and no message member.
6. Positive controls that must keep working: after UntagResource of one key, the same one-new-key TagResource succeeds; replacing an existing key's value while at the cap succeeds.
