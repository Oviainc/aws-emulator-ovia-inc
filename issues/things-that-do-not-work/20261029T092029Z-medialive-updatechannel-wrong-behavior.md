Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 511f9d11774345a530a86fa6799cabc26a1e1916

# UpdateChannel replaces InputAttachments and EncoderSettings without revisiting the schedule, leaving actions the crate's own validator says the channel cannot execute

_Filed: 2026-10-29T09:20:29Z_
_Operation: UpdateChannel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `511f9d11774345a530a86fa6799cabc26a1e1916`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateChannel` writes the request's `InputAttachments` and `EncoderSettings` straight onto the stored `ChannelRecord`, and the schedule lives on that same record. Nothing in the handler, and nothing in the configuration rule it runs (`channel_configuration_problems`), looks at `channel.schedule`.

But the schedule's channel-scoped references are resolved AGAINST exactly those two members: `ChannelScheduleScope::of(channel)` reads `attachment_names` off `channel.input_attachments`, `output_names` off `encoder_settings.output_groups`, and the `input_prepare_actions` / `output_static_image_overlay_actions` / `motion_graphics` flags off `encoder_settings`. `BatchUpdateSchedule` refuses a created action whose reference the channel does not declare (`unknown_attachment`: "<name> is not the name of an input attachment of this channel"; `unavailable_feature`: "this channel's configuration does not enable <feature>").

So a caller can reach a stored state the service itself calls impossible, entirely through the public API: create a channel with an attachment named `in-a`, add an `inputSwitchSettings` action referencing `in-a` (accepted), stop the channel, then `UpdateChannel` with an `inputAttachments` list that no longer declares `in-a` (accepted — only the attachment count bound, the stopped-state conflict and the merged-configuration rule are checked). `DescribeSchedule` goes on reporting an input-switch action for an attachment the channel does not have, and re-sending that very action is answered 422. The same holds for `encoderSettings`: an update that drops an output named by a `staticImageOutputActivateSettings` action, or that flips `featureActivations.inputPrepareScheduleActions` back to `DISABLED`, leaves actions behind that the crate refuses to create and refuses to re-create.

## Evidence

The two writes, with no schedule reconciliation anywhere in the handler — services/service-com-amazonaws-medialive-medialive/src/ops/update_channel.rs:342-347:
```
        if let Some(encoder_settings) = input.encoder_settings {
            channel.encoder_settings = Some(encoder_settings);
        }
        if let Some(attachments) = input.input_attachments {
            channel.input_attachments = attachments;
        }
```
(the whole `handle` body is lines 207-401: the only refusals are the two list bounds at 226-243, the "must have stopped" conflict at 267-275 and the merged-configuration rule at 285-314.)

The configuration rule it runs does not know about the schedule — `grep -n -i schedule services/service-com-amazonaws-medialive-medialive/src/state/channel_configurations.rs` yields exactly one hit, a doc reference to the error shape (line 42: "(`BatchUpdateSchedule`, `CreateChannelPlacementGroup`)"); the struct it validates carries only `encoder_settings`, `input_attachments`, `channel_security_groups`, `anywhere_settings` and `held` (src/state/channel_configurations.rs:57-76).

The scope that is read off those very members — src/state/schedules.rs:132-150:
```
    pub fn of(channel: &super::ChannelRecord) -> Self {
        let encoder = channel.encoder_settings.as_ref();
        let features = encoder.and_then(|settings| settings.feature_activations.as_ref());
        Self {
            attachment_names: channel
                .input_attachments
                .iter()
                .filter_map(|attachment| attachment.input_attachment_name.clone())
                .collect(),
            output_names: encoder
                .map(|settings| settings.output_groups.as_slice())
                ...
```

and the refusal it produces — src/state/schedules.rs:170-183:
```
    /// The problem with `reference` as an input attachment name of this channel,
    /// if any: a reference "specified in the channel configuration" that the
    /// channel does not declare names no attachment the channel can switch to or
    /// prepare.
    fn unknown_attachment(&self, element_path: &str, reference: &str) -> Option<ValidationError> {
        (!self.attachment_names.contains(reference)).then(|| {
            problem(
                element_path,
                &format!(
                    "{reference} is not the name of an input attachment of this channel"
                ),
            )
        })
    }
```
applied to every created input-switch action — src/state/schedules.rs:260-265:
```
    if let Some(switch) = settings.input_switch_settings.as_ref() {
        problems.extend(scope.unknown_attachment(
            &format!("{settings_path}/inputSwitchSettings/inputAttachmentNameReference"),
            &switch.input_attachment_name_reference,
        ));
    }
```

The model sentence the crate is enforcing, quoted in the slice's own header — src/state/schedules.rs:37-42: "`InputSwitchScheduleActionSettings$InputAttachmentNameReference`: \"The name of the input attachment (not the name of the input!) to switch to. The name is specified in the channel configuration.\""

The stored action is then reported unchanged — src/ops/describe_schedule.rs:199-205:
```
        let ordered = channel.schedule.in_start_order();
        let schedule_actions: Vec<ScheduleAction> = ordered
            .iter()
            .skip(already_reported)
            .take(page_size)
            .map(|record| record.action.clone())
```
and the checked path is only the create path — src/ops/batch_update_schedule.rs:199-203:
```
        let scope = crate::state::ChannelScheduleScope::of(channel);
        let applied = channel
            .schedule
            .apply(&creates, &deletes, &scope, now)
            .map_err(refused)?;
```

Test coverage: the create-side refusals are pinned by tests/all/schedule_channel_references.rs and tests/all/channel_schedule.rs:438 (`batch_update_schedule_refuses_a_request_the_schedule_rules_reject`). Nothing pins the update side: `grep -n -i schedule tests/all/channel_update.rs` finds only `maintenanceScheduledDate` rows (lines 201-211), so no test covers what UpdateChannel does to a channel that already holds a schedule.

## Steps to Reproduce

1. POST /prod/channels with an `inputAttachments` entry whose `inputAttachmentName` is "in-a" (plus the usual encoderSettings) -> 201; note the channel id C and wait for it to settle to IDLE.
2. PUT /prod/channels/C/schedule with {"creates":{"scheduleActions":[{"actionName":"switch-a","scheduleActionStartSettings":{"immediateModeScheduleActionStartSettings":{}},"scheduleActionSettings":{"inputSwitchSettings":{"inputAttachmentNameReference":"in-a"}}}]}} -> 200, the action is stored.
3. PUT /prod/channels/C with {"inputAttachments":[]} (or a list that declares some other name) -> 200; the channel now declares no attachment called "in-a".
4. GET /prod/channels/C/schedule -> the input-switch action referencing "in-a" is still reported, i.e. the channel holds an action its own validator calls "not the name of an input attachment of this channel".
5. Re-send that action under a new actionName: PUT /prod/channels/C/schedule with the body from step 2 -> 422 UnprocessableEntityException with that message, so the stored schedule is one the service itself says cannot be created.
6. Second path: give the channel `encoderSettings.featureActivations.inputPrepareScheduleActions = "ENABLED"`, store an `inputPrepareSettings` action, then PUT /prod/channels/C with an `encoderSettings` whose flag is "DISABLED" -> 200, and the prepare action survives a configuration that the crate's `unavailable_feature` check says does not enable it.
