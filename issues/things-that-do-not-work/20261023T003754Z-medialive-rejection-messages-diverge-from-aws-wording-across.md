Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medialive.delete-input-attached-to-channel-rejected medialive.delete-running-channel-rejected medialive.eventbridge-rule-template-duplicate-name-rejected medialive.eventbridge-rule-template-group-hierarchy-crud medialive.input-security-group-invalid-cidr-rejected
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive.delete-input-attached-to-channel-rejected|run#12|AssertionError|assert message == f"Input {input_obj['Id']} is busy, it cannot be deleted", message
Issue-Key: medialive.delete-running-channel-rejected|run#14|AssertionError|assert message == "Cannot delete a channel that has not been stopped yet. Please stop the channel and try again.", (
Issue-Key: medialive.eventbridge-rule-template-duplicate-name-rejected|run#14|AssertionError|assert message == "Name already in use", message
Issue-Key: medialive.eventbridge-rule-template-group-hierarchy-crud|run#17|AssertionError|assert conflict_message == "Template group must be empty before deletion.", conflict_message
Issue-Key: medialive.input-security-group-invalid-cidr-rejected|run#4|AssertionError|assert message == "Every whitelist needs to have a valid IPv4 CIDR.", message
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# medialive rejection messages diverge from AWS wording across five rejection classes (correct codes, wrong Message)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteInput (bundle also covers DeleteChannel, StartChannel, CreateEventBridgeRuleTemplate, DeleteEventBridgeRuleTemplateGroup, CreateInputSecurityGroup)

## Description
The medialive emulator raises the right error shape and HTTP status for five distinct rejection classes, but each one carries an emulator-authored Message instead of the fixed message AWS produces for that class. In each case the recorded AWS message is deterministic for the rejection class (a constant sentence, or a constant template parameterized only by the identifier in the request), so the message is part of the contract a caller can assert; the emulator instead emits descriptive prose of its own, in several cases leaking internal/emulated resource ids and region text that AWS does not mention. This bundle has FIVE causes; all must be closed.

Cause 1 of 5: DeleteInput on an input attached to a channel.
AWS: Smithy: DeleteInput is DELETE /prod/inputs/{InputId}; ConflictException is modeled with httpError 409 and a single member Message (jsonName 'message') and the model carries only placeholder docs, so the message itself is recorded behavior only. The recording shows HTTP 409, code ConflictException, Message built from the request's InputId: "Input <InputId> is busy, it cannot be deleted".
Emulator: 409 ConflictException (correct) with Message "Input 1000000 is attached to channel(s) 1000000 and cannot be deleted" — different template, and it exposes the attaching channel id.
Why: a caller that distinguishes MediaLive's conflict reasons by message (the code is the same ConflictException for every busy/wrong-state conflict, so the message is the only discriminator AWS gives) classifies the rejection wrongly, and assertions written against AWS fail.
Fix: the DeleteInput handler's conflict path — emit AWS's message template for the attached-input conflict, parameterized by the request's InputId only.
Traces: medialive.delete-input-attached-to-channel-rejected.

Cause 2 of 5: DeleteChannel on a RUNNING channel.
AWS: Smithy: DeleteChannel is DELETE /prod/channels/{ChannelId}, ConflictException httpError 409, member Message only (placeholder docs), so the text is recorded behavior only. Recording: 409 ConflictException with the constant Message "Cannot delete a channel that has not been stopped yet. Please stop the channel and try again." — a fixed sentence for this class, with no ids or state names interpolated.
Emulator: 409 ConflictException (correct) with Message "Channel 1000000 is in state RUNNING and must be IDLE before it can be deleted" — an invented template that interpolates the channel id and state.
Why: same discriminator problem as cause 1; additionally a client that surfaces the message to an operator shows text that never appears in AWS, and the leaked state/id wording differs per run so it cannot be matched at all.
Fix: the DeleteChannel handler's not-stopped conflict path — return AWS's constant message for this class.
Traces: medialive.delete-running-channel-rejected.

Cause 3 of 5: CreateEventBridgeRuleTemplate with a name already used in the group.
AWS: Smithy: CreateEventBridgeRuleTemplate is POST /prod/eventbridge-rule-templates (success 201) with ConflictException httpError 409. Recording: 409 ConflictException with the constant Message "Name already in use".
Emulator: 409 ConflictException (correct) with Message "An eventbridge rule template named <name> already exists in us-west-2 (id 6000000)" — invented prose that interpolates the name, a region and the colliding template's internal id.
Why: the duplicate-name reason is reported with text AWS never emits, and the interpolated id/region make the message unmatched and environment-dependent for any caller asserting on it.
Fix: the CreateEventBridgeRuleTemplate handler's name-collision path — return AWS's constant message.
Traces: medialive.eventbridge-rule-template-duplicate-name-rejected.

Cause 4 of 5: DeleteEventBridgeRuleTemplateGroup on a non-empty group.
AWS: Smithy: DeleteEventBridgeRuleTemplateGroup is DELETE /prod/eventbridge-rule-template-groups/{Identifier} (success 204) with ConflictException httpError 409. Recording: 409 ConflictException with the constant Message "Template group must be empty before deletion."
Emulator: 409 ConflictException (correct) with Message "eventbridge rule template group 5000000 still holds the templates 6000000; detach the group from all signal maps and move its existing templates to another group or delete them before deleting it" — invented prose listing internal group/template ids.
Why: as above — the hierarchy-violation reason is not reported in AWS's words, and the enumerated ids differ every run.
Fix: the DeleteEventBridgeRuleTemplateGroup handler's non-empty-group path — return AWS's constant message.
Traces: medialive.eventbridge-rule-template-group-hierarchy-crud.

Cause 5 of 5: CreateInputSecurityGroup with a malformed CIDR.
AWS: Smithy: CreateInputSecurityGroup is POST /prod/inputSecurityGroups; its modeled client errors are BadRequestException (400), ForbiddenException (403) and TooManyRequestsException (429) — no ConflictException; WhitelistRules targets __listOfInputWhitelistRuleCidr whose member Cidr is an unconstrained __string ("The IPv4 CIDR to whitelist."), so the validity rule and its message are recorded behavior only. Recording: 400 BadRequestException with the constant Message "Every whitelist needs to have a valid IPv4 CIDR."
Emulator: 400 BadRequestException (correct) with Message "WhitelistRules[0].Cidr must be an IPv4 CIDR with a prefix length of 0 to 32 (for example 10.0.0.0/16); got \"not-a-cidr\"" — a member-path validation message AWS does not produce for this operation.
Why: a caller validating its own input-handling against the AWS message (or mapping it to a user-facing error) gets text that never occurs in AWS; the index-qualified member path also implies a per-member error contract MediaLive does not offer.
Fix: the CreateInputSecurityGroup whitelist-rule validation path — keep the 400/BadRequestException classification and return AWS's constant message for an invalid CIDR.
Traces: medialive.input-security-group-invalid-cidr-rejected.

Notes: in all five causes the HTTP status and the error code already match AWS, and the rejection itself is correctly refused (no leaked side effect was observed before the assertion), so the defect is confined to the Message rendering. Open issue 20261022T171117Z-medialive-template-update-operations-do-not-persist-mutated.md concerns Update persistence on the same template resources and does NOT cover these rejection messages; 20261022T213347Z-medialive-updateinput-on-a-deleted-input-succeeds-instead.md owns medialive.input-lifecycle-url-pull and is excluded here.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator (medialive 10/21 passed). All five failures are assertion mismatches on the error Message only; every expected error code was returned.
- medialive.delete-input-attached-to-channel-rejected: expected code ConflictException + Message "Input <id> is busy, it cannot be deleted"; got ConflictException + "Input 1000000 is attached to channel(s) 1000000 and cannot be deleted".
- medialive.delete-running-channel-rejected: expected ConflictException + "Cannot delete a channel that has not been stopped yet. Please stop the channel and try again."; got ConflictException + "Channel 1000000 is in state RUNNING and must be IDLE before it can be deleted". (The check's later assertions — DescribeChannel still RUNNING, stop-then-delete, NotFoundException on StartChannel of a deleted channel — were never reached.)
- medialive.eventbridge-rule-template-duplicate-name-rejected: expected ConflictException + "Name already in use"; got ConflictException + "An eventbridge rule template named sim2real-medialive-ebtpl-7a6f08561b59 already exists in us-west-2 (id 6000000)".
- medialive.eventbridge-rule-template-group-hierarchy-crud: expected ConflictException + "Template group must be empty before deletion."; got ConflictException + "eventbridge rule template group 5000000 still holds the templates 6000000; detach the group from all signal maps and move its existing templates to another group or delete them before deleting it".
- medialive.input-security-group-invalid-cidr-rejected: expected BadRequestException + "Every whitelist needs to have a valid IPv4 CIDR."; got BadRequestException + "WhitelistRules[0].Cidr must be an IPv4 CIDR with a prefix length of 0 to 32 (for example 10.0.0.0/16); got \"not-a-cidr\"".
Smithy (services/service-com-amazonaws-medialive-medialive/smithy-model/com-amazonaws-medialive-medialive.json): ConflictException errorKind=client httpError=409, single member Message (jsonName 'message'); BadRequestException errorKind=client httpError=400, single member Message; DeleteInput DELETE /prod/inputs/{InputId}; DeleteChannel DELETE /prod/channels/{ChannelId}; CreateEventBridgeRuleTemplate POST /prod/eventbridge-rule-templates (201); DeleteEventBridgeRuleTemplateGroup DELETE /prod/eventbridge-rule-template-groups/{Identifier} (204); CreateInputSecurityGroup POST /prod/inputSecurityGroups with no ConflictException modeled.

## Steps to Reproduce
Cause 1: CreateInput (URL_PULL) + CreateChannel attaching it, wait for the input to read ATTACHED, then DeleteInput on that input -> 409 ConflictException; inspect Error.Message.
Cause 2: create the same input/channel, StartChannel, wait for RUNNING, then DeleteChannel -> 409 ConflictException; inspect Error.Message.
Cause 3: CreateEventBridgeRuleTemplateGroup, CreateEventBridgeRuleTemplate with name N in that group, then CreateEventBridgeRuleTemplate again with the same name N and group -> 409 ConflictException; inspect Error.Message.
Cause 4: CreateEventBridgeRuleTemplateGroup, CreateEventBridgeRuleTemplate inside it, then DeleteEventBridgeRuleTemplateGroup on the still-populated group -> 409 ConflictException; inspect Error.Message.
Cause 5: CreateInputSecurityGroup with WhitelistRules=[{"Cidr": "not-a-cidr"}] -> 400 BadRequestException; inspect Error.Message.
Equivalent replay: the five check ids in trace_ids.
