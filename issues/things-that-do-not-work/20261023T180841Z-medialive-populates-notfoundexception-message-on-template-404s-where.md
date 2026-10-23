Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medialive.eventbridge-rule-template-group-hierarchy-crud
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive.eventbridge-rule-template-group-hierarchy-crud|run#29|AssertionError|assert message is None, f"expected no message reading deleted template, got {message!r}"
Issue-Commit: d46c9ea0387b1ea18140666d9f1aeb4877a1d699

# medialive populates NotFoundException.message on template 404s where AWS returns it unset

> _Note: this issue was identified against the code base at commit `d46c9ea0387b1ea18140666d9f1aeb4877a1d699`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEventBridgeRuleTemplate

## Description
Cause 1 of 1: MediaLive's 404 NotFoundException for the EventBridge rule-template hierarchy carries a populated `message` member in the emulator, where real AWS returns the error with that member absent/null.

AWS: recorded behavior only. Smithy (com.amazonaws.medialive): GetEventBridgeRuleTemplate is GET /prod/eventbridge-rule-templates/{Identifier}, code 200, modelled errors include NotFoundException (errorKind client, httpError 404); NotFoundException is a structure whose single member `Message` (jsonName "message") is OPTIONAL — the model does not require it, and its doc is a placeholder, so the model states no message rule. The recorded AWS behavior this check encodes is that these template-hierarchy 404s are answered with NO message at all: the check comments it as "AWS answers these NotFoundExceptions with a null message, so the exact message is None" and asserts `message is None` for GetEventBridgeRuleTemplate after delete, UpdateEventBridgeRuleTemplate after delete, and GetEventBridgeRuleTemplateGroup after delete. The HTTP status (404) and error code (NotFoundException) are what AWS and the emulator agree on; the divergence is purely whether the `message` member is present in the serialized error.

Emulator: returns 404 / NotFoundException — correct — but with `message` populated. Replay of medialive.eventbridge-rule-template-group-hierarchy-crud fails at the assertion `assert message is None` after DeleteEventBridgeRuleTemplate with: AssertionError: expected no message reading deleted template, got 'eventbridge rule template <id> not found'. The error code assertion immediately before it passed, so the code and status are right and only the message presence diverges. The same unconditional message-filling applies on the sibling post-delete reads/updates in this hierarchy (UpdateEventBridgeRuleTemplate, GetEventBridgeRuleTemplateGroup), which the check asserts identically but never reaches.

Why: a client that reads `e.response["Error"]["Message"]` and branches on its presence — or a conformance/snapshot test asserting the recorded AWS error document — sees a field AWS does not send. Serialized error documents differ from AWS byte-for-byte in the member set, not just the text, so error-document comparisons and golden-file assertions that pass against AWS fail here.

Fix: the error serializer / the NotFoundException construction used by the EventBridge rule-template and rule-template-group handlers (get/update/delete by Identifier) — the not-found path should raise NotFoundException with no message member rather than synthesizing a descriptive one. Scope the change to the MediaLive template-hierarchy not-found paths; do not change the status or the error code, which are already correct.

Note on scope: this is NOT the already-open medialive issue 20261022T171117Z-medialive-template-update-operations-do-not-persist-mutated.md (lost Update writes on 200 responses) nor 20261023T110844Z-medialive-misclassifies-post-delete-mutations-startchannel-409-instead.md (wrong error CLASS after delete). Here the class is right and only the error document's message member diverges.

Traces: medialive.eventbridge-rule-template-group-hierarchy-crud.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, verdict=mismatch:

[mismatch] medialive.eventbridge-rule-template-group-hierarchy-crud
  check_medialive_eventbridge_rule_template_group_hierarchy_crud.py line 72, in run
    assert message is None, f"expected no message reading deleted template, got {message!r}"
  AssertionError: expected no message reading deleted template, got 'eventbridge rule template 6000000 not found'

Expected (AWS, recorded): DeleteEventBridgeRuleTemplate then GetEventBridgeRuleTemplate -> HTTP 404, Error.Code = "NotFoundException", Error.Message absent (botocore reports None).
Actual (emulator): HTTP 404, Error.Code = "NotFoundException" (matches), Error.Message populated with a generated description naming the id (the quoted text is one recording's example, not a requirement — the requirement is that the member is unset).

The preceding assertion in the same check, `assert code == "NotFoundException"`, passed; everything earlier in the check passed too (group create, template create with GroupId linkage, ConflictException with message "Template group must be empty before deletion." on deleting a non-empty group, independent re-reads at both levels, and the EventTargets update round-trip). So the only divergence reached is the presence of the message member.

Smithy: com.amazonaws.medialive#NotFoundException = structure { Message: __string, jsonName "message" } (optional), errorKind client, httpError 404; bound as an error of GetEventBridgeRuleTemplate (GET /prod/eventbridge-rule-templates/{Identifier}), UpdateEventBridgeRuleTemplate (PATCH same URI) and GetEventBridgeRuleTemplateGroup (GET /prod/eventbridge-rule-template-groups/{Identifier}).

## Steps to Reproduce
1. POST /prod/eventbridge-rule-template-groups to create a rule template group; note its id.
2. POST /prod/eventbridge-rule-templates with that groupIdentifier, eventType MEDIALIVE_CHANNEL_ALERT and one eventTarget; note the template id.
3. DELETE /prod/eventbridge-rule-templates/<template-id>.
4. GET /prod/eventbridge-rule-templates/<template-id> — the response is 404 with code NotFoundException (correct) but the error document carries a populated "message" member; AWS returns that member unset.
5. Same for PATCH /prod/eventbridge-rule-templates/<template-id> after the delete, and for GET /prod/eventbridge-rule-template-groups/<group-id> after the group is deleted.
6. Equivalent replay: behavioral check medialive.eventbridge-rule-template-group-hierarchy-crud.
