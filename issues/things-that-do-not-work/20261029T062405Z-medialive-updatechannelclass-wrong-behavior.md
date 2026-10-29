Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9348029012dfe744d83ddaf48083c442eb7e04ad

# A class change to SINGLE_PIPELINE leaves the channel's schedule holding a PIPELINE_1 action the crate's own validator says the channel cannot execute

_Filed: 2026-10-29T06:24:05Z_
_Operation: UpdateChannelClass | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9348029012dfe744d83ddaf48083c442eb7e04ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateChannelClass` writes the new `ChannelClass` onto the stored channel and re-provisions the pipelines and the egress addresses, but it never revisits the state that was validated AGAINST the old class — above all the channel's own schedule, which lives on the same `ChannelRecord`.

The class-to-pipeline rule is single-homed in `ChannelRecord::pipelines()`, whose doc says it is what decides "whether a request may name a pipeline" and "how a schedule action's pause target is resolved". `BatchUpdateSchedule` therefore REFUSES a `pauseStateSettings` action naming `PIPELINE_1` on a one-pipeline channel: `ChannelScheduleScope::unknown_pipeline` reports "<id> is not a pipeline of this channel, which has N pipeline(s)" as an `UnprocessableEntityException` validation error (pinned by a test).

A caller can still reach exactly that stored state through the public API, because the class change is not held to the same rule: create a STANDARD channel, add a pause action on `PIPELINE_1` (accepted — the channel has two pipelines), then `UpdateChannelClass` to `SINGLE_PIPELINE` (accepted — the handler checks only the destinations bound and that the channel has stopped). The channel now reports `pipelinesRunningCount` 1 and a single `PipelineDetail`, while `DescribeSchedule` keeps reporting a pause action for a pipeline the channel does not have — an action the same crate refuses to create and refuses to re-create, so a caller who re-sends its own stored schedule is answered 422.

The same asymmetry holds for the channel's alert log: `ChannelRecord::raise_alert` is documented as the one way an alert enters the log precisely so that "the `PipelineId` rule holds for every alert in it" — a pipeline the class does not give the channel "is not one an alert can be associated with" — yet a class downgrade leaves any already-recorded `PIPELINE_1` alert in the log for `ListAlerts` to report. Both symptoms are one defect: the class write does not consult the pipeline rule the rest of the crate derives from the class.

## Evidence

The class write, with no schedule (or alert) reconciliation — services/service-com-amazonaws-medialive-medialive/src/ops/update_channel_class.rs:240-255:
```
        // The class is what this operation exists to write; the destinations are
        // replaced only when the request names them.
        channel.channel_class = input.channel_class;
        if let Some(destinations) = input.destinations {
            channel.destinations = destinations;
        }

        // The new class has been applied, which is what `UPDATING` reports; the
        // channel is still stopped, so the clock owes it the `IDLE` the change
        // started from.
        crate::state::begin_update(channel, now);

        // A class that adds a pipeline adds the public address that pipeline
        // egresses from; one that drops a pipeline leaves the addresses the
        // channel already holds alone, so going back reports the same ones.
        state.allocate_channel_egress_addresses(&input.channel_id);
```
(the whole `handle` body is lines 186-265: the only refusals are the destinations bound at 200-207 and the "must have stopped" conflict at 230-238.)

The rule the class governs — services/service-com-amazonaws-medialive-medialive/src/state.rs:1348-1357:
```
    /// The pipelines this channel's class gives it, numbered as
    /// `PipelineDetail$PipelineId` numbers them (the `ChannelClass` doc: a
    /// `STANDARD` channel has two, a `SINGLE_PIPELINE` channel one).
    ///
    /// The one place the class-to-pipeline rule lives: every other reading of it
    /// — whether a request may name a pipeline, which of them are up, how a
    /// schedule action's pause target is resolved — derives from here.
    pub fn pipelines(&self) -> ::std::ops::Range<i32> {
        0..self.pipeline_count()
    }
```

The schedule side, which refuses what the class change creates — services/service-com-amazonaws-medialive-medialive/src/state/schedules.rs:195-211:
```
    /// The problem with `reference` as a pipeline of this channel, if any: the
    /// class fixes how many pipelines a channel has ("a single pipeline channel
    /// only has one"), so a pipeline it does not have names nothing that could
    /// pause.
    fn unknown_pipeline(&self, element_path: &str, reference: &str) -> Option<ValidationError> {
        let known = super::pipeline_index(reference)
            .is_some_and(|pipeline| self.pipelines.contains(&pipeline));
        (!known).then(|| {
            problem(
                element_path,
                &format!(
                    "{reference} is not a pipeline of this channel, which has {} pipeline(s)",
                    self.pipelines.len()
                ),
            )
        })
    }
```
and the scope it reads is taken from the channel's CURRENT class — src/state/schedules.rs:148-150: `// The channel's own pipelines, so `PIPELINE_1` names one of them / // only in the two-pipeline class.` / `pipelines: channel.pipelines().collect(),`; the pause check is src/state/schedules.rs:275-282.

The stored action is then reported unchanged — src/ops/describe_schedule.rs:199-205:
```
        let ordered = channel.schedule.in_start_order();
        let schedule_actions: Vec<ScheduleAction> = ordered
            .iter()
            .skip(already_reported)
            .take(page_size)
            .map(|record| record.action.clone())
```

The alert copy of the same rule — src/state.rs:1377-1386: "The one way an alert enters a channel's log, so the `PipelineId` rule holds for every alert in it: `ChannelAlert$PipelineId` is \"The ID of the pipeline this alert is associated with\", and a pipeline this channel's class does not give it ([`Self::has_pipeline`]) is not one an alert can be associated with — so there is no such alert to record".

Test coverage: the create-side refusal IS pinned — tests/all/schedule_channel_references.rs:315-325 (row "pause of a pipeline the one-pipeline class does not have", `SINGLE_PIPELINE`, `{ "pauseStateSettings": { "pipelines": [{ "pipelineId": "PIPELINE_1" }] } }`, expected element path "creates/scheduleActions/0/scheduleActionSettings/pauseStateSettings/pipelines/0/pipelineId"). The class-update suite asserts only the pipeline count, the pipeline details and the egress endpoints across a change (tests/all/channel_class_update.rs:150-246, `a_class_update_re_provisions_the_pipelines_the_new_class_gives_the_channel`); `grep -n 'schedule' tests/all/channel_class_update.rs` finds nothing, so no test pins the current behaviour of the schedule across a class change.

## Steps to Reproduce

1. POST /prod/channels with {"name":"two-pipelines","channelClass":"STANDARD","encoderSettings":{...}} -> 201; note the channel id C, and wait for it to settle to IDLE.
2. PUT /prod/channels/C/schedule with {"creates":{"scheduleActions":[{"actionName":"pause1","scheduleActionStartSettings":{"immediateModeScheduleActionStartSettings":{}},"scheduleActionSettings":{"pauseStateSettings":{"pipelines":[{"pipelineId":"PIPELINE_1"}]}}}]}} -> 200, the action is stored (the channel has two pipelines).
3. PUT /prod/channels/C/channelClass with {"channelClass":"SINGLE_PIPELINE"} -> 200; the channel now reports one pipeline (pipelineDetails has one entry, pipelinesRunningCount 1 once running).
4. GET /prod/channels/C/schedule -> the pause action for PIPELINE_1 is still reported, i.e. the channel holds an action its own validator calls "not a pipeline of this channel, which has 1 pipeline(s)".
5. Re-send that very action: PUT /prod/channels/C/schedule with the body from step 2 (after deleting the name, or under a new actionName) -> 422 UnprocessableEntityException with that message, so the stored schedule is one the service itself says cannot be created.
