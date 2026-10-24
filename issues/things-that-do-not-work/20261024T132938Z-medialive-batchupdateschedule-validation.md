Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# Schedule actions are validated only structurally: references into the channel (inputAttachmentNameReference) are accepted even when the channel has no such attachment

_Filed: 2026-10-24T13:29:38Z_
_Operation: BatchUpdateSchedule | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: BatchUpdateSchedule validates each created action against the CHANNEL it is being added to, not just against the request's own shape. An action whose `InputSwitchSettings.InputAttachmentNameReference` ("The name of the input attachment (not the name of the input!) to switch to") or `InputPrepareSettings.InputAttachmentNameReference` names a value that is not one of the channel's `InputAttachments[].InputAttachmentName` cannot be executed by the channel, and the real service refuses the whole batch with 422 UnprocessableEntityException carrying a `validationErrors` entry whose `elementPath` points at the offending member. Nothing is written.

Actual: the handler resolves the channel only to get at its schedule and then calls `channel.schedule.apply(&creates, &deletes, now)`. `ChannelSchedule::apply` has no access to the channel record at all (it is a method on the schedule struct, which owns only the action map), so the only rules enforced are request-internal: action-name non-empty / unique / not already in the schedule, exactly one `ScheduleActionSettings` selected, exactly one start mode, fixed-mode time parseable, and follow-mode `ReferenceActionName` naming another action of the post-apply schedule. An input-switch action pointing at an attachment name the channel does not have is therefore stored and answered 200, and `DescribeSchedule` reports it forever as a legitimate queued action; the caller never learns the action can never fire.

The same hole covers the other channel-scoped references the request can carry (e.g. a motion-graphics or static-image action on a channel whose EncoderSettings declare no such feature) — all of them are symptoms of the one missing step: the request is never cross-checked against the stored channel configuration.

## Evidence

Spec: BatchUpdateSchedule models UnprocessableEntityException (422) whose `ValidationErrors` is "A collection of validation error responses" with `ElementPath` = "Path to the source of the error" (smithy_query operation=BatchUpdateSchedule) — a per-member validation channel that only makes sense for requests the service can inspect against state. `InputSwitchScheduleActionSettings$InputAttachmentNameReference` is documented as naming an input ATTACHMENT of the channel, and `Channel$InputAttachments[].InputAttachmentName` is where those names live, so the reference is resolvable server-side. Handler: src/ops/batch_update_schedule.rs:187-207 (channel resolved, then `channel.schedule.apply(&creates, &deletes, now)`); validation lives entirely in src/state/schedules.rs:199-330 (`apply`), which never sees the channel's `input_attachments`. Tests: no test in services/service-com-amazonaws-medialive-medialive/tests/all/channel_schedule.rs (or anywhere in the crate) mentions inputAttachmentNameReference / input-switch actions, so nothing asserts the current acceptance either.

## Steps to Reproduce

1. POST /prod/channels with a single inputAttachment whose inputAttachmentName is "primary" (and an attached input); note the channel id.
2. PUT /prod/channels/<id>/schedule with creates.scheduleActions = [{actionName: "switch1", scheduleActionStartSettings: {immediateModeScheduleActionStartSettings: {}}, scheduleActionSettings: {inputSwitchSettings: {inputAttachmentNameReference: "does-not-exist"}}}].
3. Expected: 422 UnprocessableEntityException whose validationErrors names the inputAttachmentNameReference member, and the schedule unchanged.
4. Actual: 200 with the action echoed back in creates.scheduleActions, and GET /prod/channels/<id>/schedule lists the unexecutable action as part of the channel's schedule.
