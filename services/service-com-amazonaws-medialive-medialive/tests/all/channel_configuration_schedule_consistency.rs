// SPDX-License-Identifier: CC0-1.0

//! What a write to a channel's CONFIGURATION does to the schedule that channel
//! already holds, end to end over the restJson1 wire.
//!
//! Rule: a schedule action's channel-scoped references — an input attachment by
//! name, an output by name, a pipeline of the channel's class, a feature the
//! channel has to declare — are resolved against the channel configuration at
//! all times, not only when the action is created, so a write that would leave
//! the channel unable to execute an action its schedule already holds is refused
//! with 422 `UnprocessableEntityException` whose `validationErrors` names the
//! stored action and the member of it that no longer resolves, and nothing is
//! written; a write every stored action survives is applied as usual.
//!
//! This is the UPDATE side of the rule `crate::schedule_channel_references`
//! pins on the create side, so the two suites share one harness (the channel
//! setup, the action builder and the response readers come from there) and this
//! one adds only the writes: `UpdateChannel`, which replaces the
//! `EncoderSettings` and the `InputAttachments` the references resolve against,
//! and `UpdateChannelClass`, which replaces the pipelines they resolve against.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour. The tables span the referencing members
//! (`smithy_query(query="shape", shape="ScheduleActionSettings")` and the
//! referenced shapes `InputAttachment`, `Output`, `FeatureActivations`,
//! `MotionGraphicsConfiguration`, `PipelineId`), both channel classes and both
//! sides of the rule; the issue's input-switch case is ONE row.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::CHANNEL_CONTAINS_ERRORS_MESSAGE;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{describe as describe_channel, wait_for_the_clock};
use crate::schedule_channel_references::{
    action, channel, describe as describe_schedule, element_paths, encoder_settings, features,
    reported_names, schedule_uri, update as create_actions,
};
use crate::wire::{call, error_code, error_message, medialive};

/// `UpdateChannel` as a client sends it. The body is sent EXACTLY as the row
/// built it, so a row that compares the stored members against its patch
/// resolves the patch itself first (`support::resolved_channel_references`).
async fn update_channel(router: &MultiServiceRouter, id: &str, body: Value) -> (u16, Value) {
    call(router, "PUT", &format!("/prod/channels/{id}"), Some(body)).await
}

/// `UpdateChannelClass` as a client sends it.
async fn update_class(router: &MultiServiceRouter, id: &str, class: &str) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/prod/channels/{id}/channelClass"),
        Some(json!({ "channelClass": class })),
    )
    .await
}

/// `BatchUpdateSchedule` deleting the named actions, which is how a caller frees
/// a channel of the actions a configuration write would otherwise strand.
async fn delete_actions(
    router: &MultiServiceRouter,
    id: &str,
    names: &[String],
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &schedule_uri(id),
        Some(json!({ "deletes": { "actionNames": names } })),
    )
    .await
}

/// One channel of `class` declaring `attachments` and `settings`, holding
/// `actions` in its schedule and settled in `IDLE` — the state a configuration
/// write is accepted from.
async fn channel_holding(
    router: &MultiServiceRouter,
    case: &str,
    class: &str,
    attachments: &[&str],
    settings: Option<Value>,
    actions: Vec<Value>,
) -> String {
    let id = channel(router, class, attachments, settings).await;
    let (status, stored) = create_actions(router, &id, actions).await;
    assert_eq!(
        status, 200,
        "case {case:?}: BatchUpdateSchedule expected 200, got {status}: {stored}"
    );
    wait_for_the_clock();
    let (status, idle) = describe_channel(router, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeChannel expected 200, got {status}: {idle}"
    );
    assert_eq!(
        idle["state"],
        json!("IDLE"),
        "case {case:?}: expected the created channel to settle to IDLE, got {:?}",
        idle["state"]
    );
    id
}

/// The `errorMessage` a refusal reported at `element_path` ("Path to the source
/// of the error"), if it reported one there at all.
fn problem_message(body: &Value, element_path: &str) -> Option<String> {
    body["validationErrors"].as_array().and_then(|errors| {
        errors
            .iter()
            .find(|error| error["elementPath"].as_str() == Some(element_path))
            .map(|error| {
                error["errorMessage"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
    })
}

/// The action names one request created, in the order it listed them.
fn action_names(actions: &[Value]) -> Vec<String> {
    actions
        .iter()
        .map(|action| action["actionName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Assert a refusal is the modelled 422 naming `element_path` with `message`.
fn assert_refused(case: &str, status: u16, body: &Value, element_path: &str, message: &str) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, body),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {body}"
    );
    assert_eq!(
        error_message(body),
        CHANNEL_CONTAINS_ERRORS_MESSAGE,
        "case {case:?}: expected the message {CHANNEL_CONTAINS_ERRORS_MESSAGE:?}, got {body}"
    );
    let paths = element_paths(body);
    assert!(
        paths.iter().any(|path| path == element_path),
        "case {case:?}: expected a validationErrors entry at {element_path:?}, got {paths:?}"
    );
    assert_eq!(
        problem_message(body, element_path).as_deref(),
        Some(message),
        "case {case:?}: expected the entry at {element_path:?} to say {message:?}, got {body}"
    );
}

/// `EncoderSettings` declaring motion graphics, with `insertion` as its
/// `MotionGraphicsInsertion` flag (`smithy_query(query="shape",
/// shape="MotionGraphicsConfiguration")`).
fn motion_graphics(insertion: &str) -> Value {
    encoder_settings(
        &[],
        json!({
            "motionGraphicsConfiguration": {
                "motionGraphicsInsertion": insertion,
                "motionGraphicsSettings": { "htmlMotionGraphicsSettings": {} },
            }
        }),
    )
}

/// One `inputAttachments` list naming `names`.
///
/// The `InputId` is SPELLED rather than minted here: an attachment's id has to
/// name an input this account holds, and `support::resolved_channel_references`
/// puts one input behind the spelling, so a row says only which attachment names
/// the patch declares and no row depends on an id something else minted.
fn attachments(names: &[&str]) -> Value {
    Value::Array(
        names
            .iter()
            .map(|name| json!({ "inputAttachmentName": name, "inputId": "attached-input" }))
            .collect(),
    )
}

/// One row of the stranding table: the case, the channel's attachment names and
/// encoder settings, the actions its schedule holds, the patch, and the
/// `elementPath` and `errorMessage` the refusal has to report.
type StrandingCase = (
    &'static str,
    &'static [&'static str],
    Option<Value>,
    Vec<Value>,
    Value,
    String,
    String,
);

/// Rule: `UpdateChannel` refuses a configuration under which an action the
/// channel's schedule already holds is one `BatchUpdateSchedule` would refuse to
/// create — the refusal names the stored action and the member that no longer
/// resolves — and writes nothing, so the channel keeps the configuration its
/// schedule was built against; once the stranded actions are deleted the same
/// patch is accepted.
///
/// The table spans the two members this operation replaces that a reference
/// resolves against (`InputAttachments`, `EncoderSettings`) across every
/// referencing member: the three `inputAttachmentNameReference`s, the two
/// `outputNames` lists, the two `FeatureActivations` flags and the motion
/// graphics configuration — dropped, emptied, renamed and turned off.
#[tokio::test]
async fn an_update_that_would_strand_a_stored_action_is_refused_and_nothing_is_written() {
    let switch = |name: &str, reference: &str| {
        action(
            name,
            json!({ "inputSwitchSettings": { "inputAttachmentNameReference": reference } }),
        )
    };
    let overlay_settings = || Some(encoder_settings(&["hls", "archive"], features("ENABLED", "ENABLED")));
    let unknown_attachment =
        |name: &str| format!("{name} is not the name of an input attachment of this channel");
    let unknown_output = |name: &str| format!("{name} is not the name of an output of this channel");
    let unavailable = |feature: &str| format!("this channel's configuration does not enable {feature}");
    let stored_action = |name: &str, member: &str| {
        format!("schedule/scheduleActions/{name}/scheduleActionSettings/{member}")
    };

    let table: [StrandingCase; 10] = [
        (
            "an input switch whose attachment the patch drops",
            &["in-a", "in-b"],
            None,
            vec![switch("switch-a", "in-a")],
            json!({ "inputAttachments": attachments(&["in-b"]) }),
            stored_action(
                "switch-a",
                "inputSwitchSettings/inputAttachmentNameReference",
            ),
            unknown_attachment("in-a"),
        ),
        (
            "an input switch on a channel the patch leaves no attachment at all",
            &["in-a"],
            None,
            vec![switch("switch-a", "in-a")],
            json!({ "inputAttachments": attachments(&[]) }),
            stored_action(
                "switch-a",
                "inputSwitchSettings/inputAttachmentNameReference",
            ),
            unknown_attachment("in-a"),
        ),
        (
            "an input switch whose attachment the patch renames",
            &["in-a"],
            None,
            vec![switch("switch-a", "in-a")],
            json!({ "inputAttachments": attachments(&["in-renamed"]) }),
            stored_action(
                "switch-a",
                "inputSwitchSettings/inputAttachmentNameReference",
            ),
            unknown_attachment("in-a"),
        ),
        (
            "an input prepare whose attachment the patch drops",
            &["in-a", "in-b"],
            Some(encoder_settings(&[], features("ENABLED", "DISABLED"))),
            vec![action(
                "prepare-a",
                json!({ "inputPrepareSettings": { "inputAttachmentNameReference": "in-a" } }),
            )],
            json!({ "inputAttachments": attachments(&["in-b"]) }),
            stored_action(
                "prepare-a",
                "inputPrepareSettings/inputAttachmentNameReference",
            ),
            unknown_attachment("in-a"),
        ),
        (
            "an input prepare whose feature the patch turns off",
            &["in-a"],
            Some(encoder_settings(&[], features("ENABLED", "DISABLED"))),
            vec![action(
                "prepare-a",
                json!({ "inputPrepareSettings": { "inputAttachmentNameReference": "in-a" } }),
            )],
            json!({ "encoderSettings": encoder_settings(&[], features("DISABLED", "DISABLED")) }),
            stored_action("prepare-a", "inputPrepareSettings"),
            unavailable("input prepare schedule actions"),
        ),
        (
            "a scte-35 input whose attachment the patch drops",
            &["in-a", "in-b"],
            None,
            vec![action(
                "scte-a",
                json!({
                    "scte35InputSettings": {
                        "mode": "FIXED",
                        "inputAttachmentNameReference": "in-a",
                    }
                }),
            )],
            json!({ "inputAttachments": attachments(&["in-b"]) }),
            stored_action(
                "scte-a",
                "scte35InputSettings/inputAttachmentNameReference",
            ),
            unknown_attachment("in-a"),
        ),
        (
            "an output static image activate whose output the patch drops",
            &["in-a"],
            overlay_settings(),
            vec![action(
                "overlay-on",
                json!({
                    "staticImageOutputActivateSettings": {
                        "image": { "uri": "s3://bucket/overlay.png" },
                        "outputNames": ["hls", "archive"],
                    }
                }),
            )],
            json!({ "encoderSettings": encoder_settings(&["hls"], features("ENABLED", "ENABLED")) }),
            stored_action(
                "overlay-on",
                "staticImageOutputActivateSettings/outputNames/1",
            ),
            unknown_output("archive"),
        ),
        (
            "an output static image deactivate whose feature the patch turns off",
            &["in-a"],
            overlay_settings(),
            vec![action(
                "overlay-off",
                json!({ "staticImageOutputDeactivateSettings": { "outputNames": ["hls"] } }),
            )],
            json!({ "encoderSettings": encoder_settings(&["hls", "archive"], features("ENABLED", "DISABLED")) }),
            stored_action("overlay-off", "staticImageOutputDeactivateSettings"),
            unavailable("output static image overlay schedule actions"),
        ),
        (
            "a motion graphics activate whose configuration the patch disables",
            &["in-a"],
            Some(motion_graphics("ENABLED")),
            vec![action(
                "graphics-on",
                json!({ "motionGraphicsImageActivateSettings": { "url": "https://example.test/g.html" } }),
            )],
            json!({ "encoderSettings": motion_graphics("DISABLED") }),
            stored_action("graphics-on", "motionGraphicsImageActivateSettings"),
            unavailable("motion graphics insertion"),
        ),
        (
            "one stranded action of two refuses the whole update",
            &["in-a", "in-b"],
            None,
            vec![switch("switch-a", "in-a"), switch("switch-b", "in-b")],
            json!({ "inputAttachments": attachments(&["in-b"]) }),
            stored_action(
                "switch-a",
                "inputSwitchSettings/inputAttachmentNameReference",
            ),
            unknown_attachment("in-a"),
        ),
    ];

    for (case, declared, settings, actions, patch, element_path, message) in table {
        let router = medialive();
        let id = channel_holding(
            &router,
            case,
            "STANDARD",
            declared,
            settings,
            actions.clone(),
        )
        .await;
        let (_, before) = describe_channel(&router, &id).await;
        // The patch as the account can hold it: one input behind every id it
        // spells, resolved once so the request sent and the members compared
        // against it are the same.
        let patch = crate::support::resolved_channel_references(&router, patch).await;

        let (status, refusal) = update_channel(&router, &id, patch.clone()).await;
        assert_refused(case, status, &refusal, &element_path, &message);

        // Nothing was written: the channel reads back exactly as it was, and the
        // schedule still holds every action it held.
        wait_for_the_clock();
        let (status, after) = describe_channel(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel expected 200, got {status}: {after}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused update to have written nothing, got {after}"
        );
        let (status, read) = describe_schedule(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSchedule expected 200, got {status}: {read}"
        );
        let mut stored = reported_names(&read);
        stored.sort();
        let mut expected = action_names(&actions);
        expected.sort();
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the schedule to be unchanged, got {stored:?}"
        );

        // The refusal is about the schedule, not about the member: once the
        // actions that would have been stranded are gone, the same patch applies.
        let (status, deleted) = delete_actions(&router, &id, &action_names(&actions)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: BatchUpdateSchedule delete expected 200, got {status}: {deleted}"
        );
        let (status, applied) = update_channel(&router, &id, patch).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the same patch to be accepted once the schedule is \
             emptied, got {status}: {applied}"
        );
    }
}

/// One row of the acceptance table: the case, the channel's attachment names and
/// encoder settings, the actions its schedule holds, and the patch.
type SurvivingCase = (
    &'static str,
    &'static [&'static str],
    Option<Value>,
    Vec<Value>,
    Value,
);

/// Rule, the other side: a configuration every stored action still resolves
/// against is applied — the patch is stored, the channel reads it back and the
/// schedule still reports every action.
///
/// The table spans the writes that touch the referenced members without
/// stranding anything: an attachment list that adds, reorders and keeps the
/// referenced name, encoder settings that drop an output no action names, keep a
/// feature enabled and replace settings no action depends on, and a patch that
/// names no configuration member at all.
#[tokio::test]
async fn an_update_every_stored_action_survives_is_applied() {
    let switch = |name: &str, reference: &str| {
        action(
            name,
            json!({ "inputSwitchSettings": { "inputAttachmentNameReference": reference } }),
        )
    };
    let overlay_settings = || Some(encoder_settings(&["hls", "archive"], features("ENABLED", "ENABLED")));

    let table: [SurvivingCase; 7] = [
        (
            "a patch that adds an attachment beside the referenced one",
            &["in-a"],
            None,
            vec![switch("switch-a", "in-a")],
            json!({ "inputAttachments": attachments(&["in-a", "in-b"]) }),
        ),
        (
            "a patch that reorders the attachments",
            &["in-a", "in-b"],
            None,
            vec![switch("switch-a", "in-a"), switch("switch-b", "in-b")],
            json!({ "inputAttachments": attachments(&["in-b", "in-a"]) }),
        ),
        (
            "a patch that names no configuration member at all",
            &["in-a"],
            None,
            vec![switch("switch-a", "in-a")],
            json!({ "name": "renamed" }),
        ),
        (
            "a patch that drops an output no stored action names",
            &["in-a"],
            overlay_settings(),
            vec![action(
                "overlay-on",
                json!({
                    "staticImageOutputActivateSettings": {
                        "image": { "uri": "s3://bucket/overlay.png" },
                        "outputNames": ["hls"],
                    }
                }),
            )],
            json!({ "encoderSettings": encoder_settings(&["hls"], features("ENABLED", "ENABLED")) }),
        ),
        (
            "a patch that keeps the feature enabled and adds an output",
            &["in-a"],
            overlay_settings(),
            vec![action(
                "overlay-off",
                json!({ "staticImageOutputDeactivateSettings": { "outputNames": ["archive"] } }),
            )],
            json!({
                "encoderSettings": encoder_settings(
                    &["hls", "archive", "frame-capture"],
                    features("ENABLED", "ENABLED"),
                )
            }),
        ),
        (
            "a patch that replaces encoder settings no stored action depends on",
            &["in-a"],
            None,
            vec![switch("switch-a", "in-a")],
            json!({ "encoderSettings": encoder_settings(&["hls"], json!({})) }),
        ),
        (
            "an empty attachment list on a channel whose schedule names no attachment",
            &["in-a"],
            None,
            vec![action(
                "pause-0",
                json!({ "pauseStateSettings": { "pipelines": [{ "pipelineId": "PIPELINE_0" }] } }),
            )],
            json!({ "inputAttachments": attachments(&[]) }),
        ),
    ];

    for (case, declared, settings, actions, patch) in table {
        let router = medialive();
        let id = channel_holding(
            &router,
            case,
            "STANDARD",
            declared,
            settings,
            actions.clone(),
        )
        .await;
        let patch = crate::support::resolved_channel_references(&router, patch).await;

        let (status, applied) = update_channel(&router, &id, patch.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {applied}"
        );
        for (member, value) in patch.as_object().expect("the patch is a JSON object") {
            assert_eq!(
                applied["channel"][member], *value,
                "case {case:?}: expected the stored {member} to be the patched one, got {:?}",
                applied["channel"][member]
            );
        }

        wait_for_the_clock();
        let (status, after) = describe_channel(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel expected 200, got {status}: {after}"
        );
        for (member, value) in patch.as_object().expect("the patch is a JSON object") {
            assert_eq!(
                after[member], *value,
                "case {case:?}: expected the channel to read back the patched {member}, got {:?}",
                after[member]
            );
        }
        let (status, read) = describe_schedule(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSchedule expected 200, got {status}: {read}"
        );
        let mut stored = reported_names(&read);
        stored.sort();
        let mut expected = action_names(&actions);
        expected.sort();
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the schedule to still hold {expected:?}, got {stored:?}"
        );
    }
}

/// One row of the class table: the case, the class the channel is created in,
/// the pipelines its stored pause action names, the class the request asks for,
/// and the `elementPath` the refusal has to report (empty for a row the rule
/// accepts).
type ClassCase = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static str,
    &'static str,
);

/// Rule: the same resolution covers the pipelines a class gives a channel, so
/// `UpdateChannelClass` refuses a class under which a stored pause action names
/// a pipeline the channel would not have and keeps the class it had; a class
/// every stored action survives is stored.
///
/// The rows span both classes in both directions: the change that takes a
/// pipeline away from an action that names it, the same change where no action
/// names the pipeline that goes, and the change that adds one.
#[tokio::test]
async fn a_class_change_is_resolved_against_the_schedule_the_channel_holds() {
    let table: [ClassCase; 4] = [
        (
            "a class change that takes away the pipeline a pause action names",
            "STANDARD",
            &["PIPELINE_1"],
            "SINGLE_PIPELINE",
            "schedule/scheduleActions/pause/scheduleActionSettings/pauseStateSettings/pipelines/0/pipelineId",
        ),
        (
            "a class change that takes away one of two pipelines a pause action names",
            "STANDARD",
            &["PIPELINE_0", "PIPELINE_1"],
            "SINGLE_PIPELINE",
            "schedule/scheduleActions/pause/scheduleActionSettings/pauseStateSettings/pipelines/1/pipelineId",
        ),
        (
            "a class change whose lost pipeline no action names",
            "STANDARD",
            &["PIPELINE_0"],
            "SINGLE_PIPELINE",
            "",
        ),
        (
            "a class change that adds a pipeline",
            "SINGLE_PIPELINE",
            &["PIPELINE_0"],
            "STANDARD",
            "",
        ),
    ];

    for (case, class, paused, requested, element_path) in table {
        let router = medialive();
        let pipelines: Vec<Value> = paused
            .iter()
            .map(|pipeline| json!({ "pipelineId": pipeline }))
            .collect();
        let actions = vec![action(
            "pause",
            json!({ "pauseStateSettings": { "pipelines": pipelines } }),
        )];
        let id = channel_holding(&router, case, class, &["in-a"], None, actions).await;

        let (status, answer) = update_class(&router, &id, requested).await;
        if element_path.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {answer}"
            );
            assert_eq!(
                answer["channel"]["channelClass"],
                json!(requested),
                "case {case:?}: expected the class {requested:?} to be stored, got {:?}",
                answer["channel"]["channelClass"]
            );
        } else {
            // The pipeline the action names is one the requested class does not
            // give the channel, which is the refusal `BatchUpdateSchedule`
            // answers a created action with, worded with the pipeline count.
            let message = format!(
                "{} is not a pipeline of this channel, which has 1 pipeline(s)",
                paused
                    .iter()
                    .copied()
                    .next_back()
                    .expect("the row names a paused pipeline")
            );
            assert_refused(case, status, &answer, element_path, &message);
        }

        wait_for_the_clock();
        let (status, after) = describe_channel(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel expected 200, got {status}: {after}"
        );
        let expected_class = if element_path.is_empty() { requested } else { class };
        assert_eq!(
            after["channelClass"],
            json!(expected_class),
            "case {case:?}: expected the channel to report the class {expected_class:?}, got {:?}",
            after["channelClass"]
        );
        let (status, read) = describe_schedule(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSchedule expected 200, got {status}: {read}"
        );
        assert_eq!(
            reported_names(&read),
            vec!["pause".to_string()],
            "case {case:?}: expected the stored action to survive either answer, got {read}"
        );
    }

    // Once the action that named the lost pipeline is gone, the class the rule
    // refused is accepted — the refusal is about the schedule, not the class.
    let router = medialive();
    let actions = vec![action(
        "pause",
        json!({ "pauseStateSettings": { "pipelines": [{ "pipelineId": "PIPELINE_1" }] } }),
    )];
    let case = "the refused class after the action is deleted";
    let id = channel_holding(&router, case, "STANDARD", &["in-a"], None, actions).await;
    let (status, refusal) = update_class(&router, &id, "SINGLE_PIPELINE").await;
    assert_eq!(
        status, 422,
        "case {case:?}: expected the class change to be refused first, got {status}: {refusal}"
    );
    let (status, deleted) = delete_actions(&router, &id, &["pause".to_string()]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: BatchUpdateSchedule delete expected 200, got {status}: {deleted}"
    );
    let (status, applied) = update_class(&router, &id, "SINGLE_PIPELINE").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the class change to be accepted once the action is gone, \
         got {status}: {applied}"
    );
}

/// Rule: `DryRun` is VALIDATE-ONLY, so a dry run earns exactly the refusal the
/// same request without the flag earns — the stranded action is reported the
/// same way — and a dry run whose configuration strands nothing still writes
/// nothing.
#[tokio::test]
async fn a_dry_run_reports_the_same_stranded_action() {
    let case = "a dry run that would strand an input switch";
    let router = medialive();
    let actions = vec![action(
        "switch-a",
        json!({ "inputSwitchSettings": { "inputAttachmentNameReference": "in-a" } }),
    )];
    let id = channel_holding(&router, case, "STANDARD", &["in-a"], None, actions).await;
    let (_, before) = describe_channel(&router, &id).await;

    let stranding = crate::support::resolved_channel_references(
        &router,
        json!({ "inputAttachments": attachments(&[]), "dryRun": true }),
    )
    .await;
    let (status, refusal) = update_channel(&router, &id, stranding).await;
    assert_refused(
        case,
        status,
        &refusal,
        "schedule/scheduleActions/switch-a/scheduleActionSettings/inputSwitchSettings/inputAttachmentNameReference",
        "in-a is not the name of an input attachment of this channel",
    );

    let surviving = crate::support::resolved_channel_references(
        &router,
        json!({ "inputAttachments": attachments(&["in-a", "in-b"]), "dryRun": true }),
    )
    .await;
    let (status, accepted) = update_channel(&router, &id, surviving).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected a dry run that strands nothing to validate, got {status}: \
         {accepted}"
    );
    assert!(
        accepted["channel"].is_null(),
        "case {case:?}: a dry run changes nothing, so it must describe no channel, got {:?}",
        accepted["channel"]
    );

    wait_for_the_clock();
    let (status, after) = describe_channel(&router, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeChannel expected 200, got {status}: {after}"
    );
    assert_eq!(
        after, before,
        "case {case:?}: expected the dry runs to have written nothing, got {after}"
    );
    let (status, read) = describe_schedule(&router, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeSchedule expected 200, got {status}: {read}"
    );
    assert_eq!(
        reported_names(&read),
        vec!["switch-a".to_string()],
        "case {case:?}: expected the stored action to be untouched, got {read}"
    );
}
