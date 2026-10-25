// SPDX-License-Identifier: CC0-1.0

//! What a created schedule action references IN THE CHANNEL it is added to,
//! end to end over the restJson1 wire.
//!
//! Rule: `BatchUpdateSchedule` resolves every channel-scoped member of a created
//! action against the stored channel configuration — an input attachment by name,
//! an output by name, a pipeline of the channel's class, a feature the channel has
//! to declare — and accepts the action only when the channel declares what it
//! names; a reference the channel does not declare refuses the WHOLE request with
//! 422 `UnprocessableEntityException` whose `validationErrors` names the offending
//! member, and nothing is written.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted is
//! the emulator's wire behaviour. The tables span the members that carry such a
//! reference (`smithy_query(query="shape", shape="ScheduleActionSettings")` and the
//! referenced shapes `InputAttachment`, `Output`, `FeatureActivations`,
//! `MotionGraphicsConfiguration`, `PipelineId`), the two channel classes and both
//! sides of every rule; the issue's input-switch case is ONE row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, medialive};

/// An input to attach to a channel, so an attachment names a real input the way a
/// client's does.
async fn input(router: &MultiServiceRouter) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/prod/inputs",
        Some(json!({
            "name": "feed",
            "type": "URL_PULL",
            "sources": [{ "url": "https://example.test/feed.m3u8" }],
        })),
    )
    .await;
    assert_eq!(status, 201, "CreateInput: expected 201, got {status}: {created}");
    created["input"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A channel carrying `attachments` (each entry is an attachment name), the
/// `encoder_settings` the case needs and `class` as its `ChannelClass`.
async fn channel(
    router: &MultiServiceRouter,
    class: &str,
    attachments: &[&str],
    encoder_settings: Option<Value>,
) -> String {
    let input_id = input(router).await;
    let attachments: Vec<Value> = attachments
        .iter()
        .map(|name| json!({ "inputAttachmentName": name, "inputId": input_id }))
        .collect();
    let mut body = json!({
        "name": "referenced-channel",
        "channelClass": class,
        "inputAttachments": attachments,
    });
    if let Some(encoder_settings) = encoder_settings {
        body["encoderSettings"] = encoder_settings;
    }
    let (status, created) = call(router, "POST", "/prod/channels", Some(body)).await;
    assert_eq!(status, 201, "CreateChannel: expected 201, got {status}: {created}");
    created["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `EncoderSettings` with the members the model marks required
/// (`smithy_query(query="shape", shape="EncoderSettings")`: AudioDescriptions,
/// OutputGroups, TimecodeConfig, VideoDescriptions), the outputs named by
/// `outputs`, and whatever `extra` members the case adds (feature activations,
/// motion graphics).
fn encoder_settings(outputs: &[&str], extra: Value) -> Value {
    let outputs: Vec<Value> = outputs
        .iter()
        .map(|name| json!({ "outputName": name, "outputSettings": {} }))
        .collect();
    let mut settings = json!({
        "audioDescriptions": [],
        "videoDescriptions": [],
        "timecodeConfig": { "source": "EMBEDDED" },
        "outputGroups": [{
            "name": "group",
            "outputGroupSettings": {},
            "outputs": outputs,
        }],
    });
    if let Some(extra) = extra.as_object() {
        for (member, value) in extra {
            settings[member] = value.clone();
        }
    }
    settings
}

/// `FeatureActivations` with both schedule-action features turned on or off
/// (`smithy_query(query="shape", shape="FeatureActivations")`).
fn features(input_prepare: &str, output_static_image_overlay: &str) -> Value {
    json!({
        "featureActivations": {
            "inputPrepareScheduleActions": input_prepare,
            "outputStaticImageOverlayScheduleActions": output_static_image_overlay,
        }
    })
}

fn schedule_uri(channel_id: &str) -> String {
    format!("/prod/channels/{channel_id}/schedule")
}

/// One immediate-mode `ScheduleAction` carrying `settings`.
fn action(name: &str, settings: Value) -> Value {
    json!({
        "actionName": name,
        "scheduleActionSettings": settings,
        "scheduleActionStartSettings": { "immediateModeScheduleActionStartSettings": {} },
    })
}

async fn update(router: &MultiServiceRouter, channel_id: &str, actions: Vec<Value>) -> (u16, Value) {
    call(
        router,
        "PUT",
        &schedule_uri(channel_id),
        Some(json!({ "creates": { "scheduleActions": actions } })),
    )
    .await
}

async fn describe(router: &MultiServiceRouter, channel_id: &str) -> (u16, Value) {
    call(router, "GET", &schedule_uri(channel_id), None).await
}

/// The `elementPath`s a refusal's `validationErrors` named ("Path to the source of
/// the error").
fn element_paths(body: &Value) -> Vec<String> {
    body["validationErrors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .map(|error| error["elementPath"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The action names a `DescribeSchedule` read reported.
fn reported_names(body: &Value) -> Vec<String> {
    body["scheduleActions"]
        .as_array()
        .map(|actions| {
            actions
                .iter()
                .map(|action| action["actionName"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the refusal table: the case, the channel's `ChannelClass`, the
/// attachment names it declares, its `EncoderSettings`, the actions the request
/// creates, and the `elementPath` the refusal has to name.
type RefusedCase = (
    &'static str,
    &'static str,
    &'static [&'static str],
    Option<Value>,
    Vec<Value>,
    &'static str,
);

/// One row of the acceptance table: [`RefusedCase`] without the element path,
/// since nothing is refused.
type AcceptedCase = (
    &'static str,
    &'static str,
    &'static [&'static str],
    Option<Value>,
    Vec<Value>,
);

/// Rule: a created action naming something the channel does not declare is
/// refused with 422 `UnprocessableEntityException`, the refusal's
/// `validationErrors` names the member the reference sits at, and the schedule is
/// left as it was.
///
/// The table spans every channel-scoped member an action can carry: the three
/// `inputAttachmentNameReference`s, the two `outputNames` lists, the pause
/// action's `pipelineId`, and the actions a channel feature has to be declared
/// for — against a channel that declares other names, a channel that declares
/// none, and the one-pipeline class.
#[tokio::test]
async fn an_action_the_channel_cannot_execute_is_refused_and_nothing_is_written() {
    let overlay_channel = || {
        Some(encoder_settings(
            &["hls-output", "archive-output"],
            features("ENABLED", "ENABLED"),
        ))
    };

    // (case, channel class, attachment names, encoder settings, created actions,
    //  the element path the refusal must name)
    let table: [RefusedCase; 11] = [
        (
            "input switch to an attachment the channel does not have",
            "STANDARD",
            &["primary"],
            None,
            vec![action(
                "switch1",
                json!({ "inputSwitchSettings": { "inputAttachmentNameReference": "does-not-exist" } }),
            )],
            "creates/scheduleActions/0/scheduleActionSettings/inputSwitchSettings/inputAttachmentNameReference",
        ),
        (
            "input switch on a channel that declares no attachment at all",
            "STANDARD",
            &[],
            None,
            vec![action(
                "switch1",
                json!({ "inputSwitchSettings": { "inputAttachmentNameReference": "primary" } }),
            )],
            "creates/scheduleActions/0/scheduleActionSettings/inputSwitchSettings/inputAttachmentNameReference",
        ),
        (
            "input switch to an unnamed attachment's empty name",
            "STANDARD",
            &["primary"],
            None,
            vec![action(
                "switch1",
                json!({ "inputSwitchSettings": { "inputAttachmentNameReference": "" } }),
            )],
            "creates/scheduleActions/0/scheduleActionSettings/inputSwitchSettings/inputAttachmentNameReference",
        ),
        (
            "input prepare of an attachment the channel does not have",
            "STANDARD",
            &["primary"],
            Some(encoder_settings(&[], features("ENABLED", "DISABLED"))),
            vec![action(
                "prepare1",
                json!({ "inputPrepareSettings": { "inputAttachmentNameReference": "secondary" } }),
            )],
            "creates/scheduleActions/0/scheduleActionSettings/inputPrepareSettings/inputAttachmentNameReference",
        ),
        (
            "input prepare on a channel that does not enable the feature",
            "STANDARD",
            &["primary"],
            Some(encoder_settings(&[], features("DISABLED", "ENABLED"))),
            vec![action(
                "prepare1",
                json!({ "inputPrepareSettings": { "inputAttachmentNameReference": "primary" } }),
            )],
            "creates/scheduleActions/0/scheduleActionSettings/inputPrepareSettings",
        ),
        (
            "fixed scte-35 input naming an attachment the channel does not have",
            "STANDARD",
            &["primary"],
            None,
            vec![action(
                "scte1",
                json!({
                    "scte35InputSettings": {
                        "mode": "FIXED",
                        "inputAttachmentNameReference": "secondary",
                    }
                }),
            )],
            "creates/scheduleActions/0/scheduleActionSettings/scte35InputSettings/inputAttachmentNameReference",
        ),
        (
            "output static image activate naming an output the channel does not have",
            "STANDARD",
            &["primary"],
            overlay_channel(),
            vec![action(
                "overlay1",
                json!({
                    "staticImageOutputActivateSettings": {
                        "image": { "uri": "s3://bucket/overlay.png" },
                        "outputNames": ["hls-output", "ghost-output"],
                    }
                }),
            )],
            "creates/scheduleActions/0/scheduleActionSettings/staticImageOutputActivateSettings/outputNames/1",
        ),
        (
            "output static image deactivate on a channel that does not enable the feature",
            "STANDARD",
            &["primary"],
            Some(encoder_settings(&["hls-output"], features("ENABLED", "DISABLED"))),
            vec![action(
                "overlay-off",
                json!({
                    "staticImageOutputDeactivateSettings": { "outputNames": ["hls-output"] }
                }),
            )],
            "creates/scheduleActions/0/scheduleActionSettings/staticImageOutputDeactivateSettings",
        ),
        (
            "pause of a pipeline the one-pipeline class does not have",
            "SINGLE_PIPELINE",
            &["primary"],
            None,
            vec![action(
                "pause1",
                json!({ "pauseStateSettings": { "pipelines": [{ "pipelineId": "PIPELINE_1" }] } }),
            )],
            "creates/scheduleActions/0/scheduleActionSettings/pauseStateSettings/pipelines/0/pipelineId",
        ),
        (
            "motion graphics activate on a channel that declares the feature disabled",
            "STANDARD",
            &["primary"],
            Some(encoder_settings(
                &[],
                json!({
                    "motionGraphicsConfiguration": {
                        "motionGraphicsInsertion": "DISABLED",
                        "motionGraphicsSettings": { "htmlMotionGraphicsSettings": {} },
                    }
                }),
            )),
            vec![action(
                "graphics1",
                json!({ "motionGraphicsImageActivateSettings": { "url": "https://example.test/g.html" } }),
            )],
            "creates/scheduleActions/0/scheduleActionSettings/motionGraphicsImageActivateSettings",
        ),
        (
            "one bad reference refuses the whole batch, not only its own action",
            "STANDARD",
            &["primary"],
            None,
            vec![
                action(
                    "fine",
                    json!({ "inputSwitchSettings": { "inputAttachmentNameReference": "primary" } }),
                ),
                action(
                    "broken",
                    json!({ "inputSwitchSettings": { "inputAttachmentNameReference": "primary2" } }),
                ),
            ],
            "creates/scheduleActions/1/scheduleActionSettings/inputSwitchSettings/inputAttachmentNameReference",
        ),
    ];

    for (case, class, attachments, settings, actions, expected_path) in table {
        let router = medialive();
        let channel_id = channel(&router, class, attachments, settings).await;

        let (status, refusal) = update(&router, &channel_id, actions).await;
        assert_eq!(
            status, 422,
            "case {case:?}: expected 422, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {refusal}"
        );
        let paths = element_paths(&refusal);
        assert!(
            paths.iter().any(|path| path == expected_path),
            "case {case:?}: expected a validationErrors entry at {expected_path:?}, got {paths:?}"
        );

        let (status, read) = describe(&router, &channel_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSchedule expected 200, got {status}: {read}"
        );
        let stored = reported_names(&read);
        assert!(
            stored.is_empty(),
            "case {case:?}: expected the schedule to be unchanged, got {stored:?}"
        );
    }
}

/// Rule, the other side: an action whose channel-scoped references all resolve
/// against the stored channel is stored and read back whole.
///
/// The table spans the same members against a channel that declares what they
/// name: both attachments of a two-attachment channel, an input prepare with and
/// without a reference (the member is optional — "If no name is provided, the
/// action will stop the most recent prepare"), a SCTE-35 input action in each
/// mode, several outputs at once, each pipeline of each class, and motion graphics
/// declared with and without an explicit insertion flag.
#[tokio::test]
async fn an_action_the_channel_can_execute_round_trips_through_the_schedule() {
    let overlay_channel = || {
        Some(encoder_settings(
            &["hls-output", "archive-output"],
            features("ENABLED", "ENABLED"),
        ))
    };

    // (case, channel class, attachment names, encoder settings, created actions)
    let table: [AcceptedCase; 11] = [
        (
            "input switch to the first attachment",
            "STANDARD",
            &["primary", "backup"],
            None,
            vec![action(
                "switch-primary",
                json!({ "inputSwitchSettings": { "inputAttachmentNameReference": "primary" } }),
            )],
        ),
        (
            "input switch to the second attachment, with a clip and a url path",
            "STANDARD",
            &["primary", "backup"],
            None,
            vec![action(
                "switch-backup",
                json!({
                    "inputSwitchSettings": {
                        "inputAttachmentNameReference": "backup",
                        "urlPath": ["live/1"],
                    }
                }),
            )],
        ),
        (
            "input prepare of a declared attachment, feature enabled",
            "STANDARD",
            &["primary", "backup"],
            Some(encoder_settings(&[], features("ENABLED", "DISABLED"))),
            vec![action(
                "prepare-backup",
                json!({ "inputPrepareSettings": { "inputAttachmentNameReference": "backup" } }),
            )],
        ),
        (
            "input prepare naming no attachment at all, feature enabled",
            "STANDARD",
            &["primary"],
            Some(encoder_settings(&[], features("ENABLED", "DISABLED"))),
            vec![action("prepare-none", json!({ "inputPrepareSettings": {} }))],
        ),
        (
            "fixed scte-35 input naming a declared attachment",
            "STANDARD",
            &["primary"],
            None,
            vec![action(
                "scte-fixed",
                json!({
                    "scte35InputSettings": {
                        "mode": "FIXED",
                        "inputAttachmentNameReference": "primary",
                    }
                }),
            )],
        ),
        (
            "scte-35 input following the active input, naming no attachment",
            "STANDARD",
            &["primary"],
            None,
            vec![action(
                "scte-active",
                json!({ "scte35InputSettings": { "mode": "FOLLOW_ACTIVE" } }),
            )],
        ),
        (
            "output static image activate naming every declared output",
            "STANDARD",
            &["primary"],
            overlay_channel(),
            vec![action(
                "overlay-on",
                json!({
                    "staticImageOutputActivateSettings": {
                        "image": { "uri": "s3://bucket/overlay.png" },
                        "outputNames": ["archive-output", "hls-output"],
                        "layer": 3,
                    }
                }),
            )],
        ),
        (
            "output static image deactivate naming one declared output",
            "STANDARD",
            &["primary"],
            overlay_channel(),
            vec![action(
                "overlay-off",
                json!({
                    "staticImageOutputDeactivateSettings": {
                        "outputNames": ["hls-output"],
                        "layer": 3,
                    }
                }),
            )],
        ),
        (
            "pause of both pipelines of the two-pipeline class",
            "STANDARD",
            &["primary"],
            None,
            vec![action(
                "pause-both",
                json!({
                    "pauseStateSettings": {
                        "pipelines": [
                            { "pipelineId": "PIPELINE_0" },
                            { "pipelineId": "PIPELINE_1" },
                        ]
                    }
                }),
            )],
        ),
        (
            "pause of the only pipeline of the one-pipeline class, and motion \
             graphics on a channel that declares it",
            "SINGLE_PIPELINE",
            &["primary"],
            Some(encoder_settings(
                &[],
                json!({
                    "motionGraphicsConfiguration": {
                        "motionGraphicsInsertion": "ENABLED",
                        "motionGraphicsSettings": { "htmlMotionGraphicsSettings": {} },
                    }
                }),
            )),
            vec![
                action(
                    "pause-only",
                    json!({
                        "pauseStateSettings": { "pipelines": [{ "pipelineId": "PIPELINE_0" }] }
                    }),
                ),
                action(
                    "graphics-on",
                    json!({
                        "motionGraphicsImageActivateSettings": {
                            "url": "https://example.test/g.html",
                            "duration": 5000,
                        }
                    }),
                ),
            ],
        ),
        (
            "motion graphics on a channel that declares the configuration with no \
             explicit insertion flag",
            "STANDARD",
            &["primary"],
            Some(encoder_settings(
                &[],
                json!({
                    "motionGraphicsConfiguration": {
                        "motionGraphicsSettings": { "htmlMotionGraphicsSettings": {} },
                    }
                }),
            )),
            vec![action(
                "graphics-on",
                json!({
                    "motionGraphicsImageActivateSettings": {
                        "url": "https://example.test/g.html"
                    }
                }),
            )],
        ),
    ];

    for (case, class, attachments, settings, actions) in table {
        let router = medialive();
        let channel_id = channel(&router, class, attachments, settings).await;
        let expected: Vec<String> = actions
            .iter()
            .map(|action| action["actionName"].as_str().unwrap_or_default().to_string())
            .collect();

        let (status, accepted) = update(&router, &channel_id, actions.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {accepted}"
        );
        let created = accepted["creates"]["scheduleActions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            created, actions,
            "case {case:?}: expected the created actions echoed back as sent, got {created:?}"
        );

        let (status, read) = describe(&router, &channel_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSchedule expected 200, got {status}: {read}"
        );
        let mut stored = reported_names(&read);
        stored.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the schedule to hold {expected:?}, got {stored:?}"
        );
    }
}

/// Rule: the references are resolved against the channel the request names, so
/// the same action is stored on a channel that declares the attachment and
/// refused on one that does not — the check is per channel, not per request shape.
#[tokio::test]
async fn the_same_action_is_resolved_against_each_channel_it_is_added_to() {
    let router = medialive();
    let declaring = channel(&router, "STANDARD", &["primary"], None).await;
    let other = channel(&router, "STANDARD", &["secondary"], None).await;
    let switch = action(
        "switch1",
        json!({ "inputSwitchSettings": { "inputAttachmentNameReference": "primary" } }),
    );

    let (status, accepted) = update(&router, &declaring, vec![switch.clone()]).await;
    assert_eq!(
        status, 200,
        "declaring channel: expected 200, got {status}: {accepted}"
    );

    let (status, refusal) = update(&router, &other, vec![switch]).await;
    assert_eq!(
        status, 422,
        "other channel: expected 422, got {status}: {refusal}"
    );
    let paths = element_paths(&refusal);
    assert!(
        paths.iter().any(|path| path.ends_with("inputAttachmentNameReference")),
        "other channel: expected the refusal to name the reference member, got {paths:?}"
    );

    let (status, read) = describe(&router, &other).await;
    assert_eq!(
        status, 200,
        "other channel: DescribeSchedule expected 200, got {status}: {read}"
    );
    let stored = reported_names(&read);
    assert!(
        stored.is_empty(),
        "other channel: expected an unchanged schedule, got {stored:?}"
    );

    let (status, read) = describe(&router, &declaring).await;
    assert_eq!(
        status, 200,
        "declaring channel: DescribeSchedule expected 200, got {status}: {read}"
    );
    assert_eq!(
        reported_names(&read),
        vec!["switch1".to_string()],
        "declaring channel: expected the stored action to be reported, got {read}"
    );
}
