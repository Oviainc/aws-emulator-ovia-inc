// SPDX-License-Identifier: CC0-1.0

//! How many alarms a deployment group's `alarmConfiguration` may be written
//! with, driven over the wire.
//!
//! RULE: whether the documented alarm figure is applied as an ADMISSION gate is
//! per operation — an update of an existing deployment group's alarm
//! configuration is admitted past that figure, storing every entry so a later
//! read answers them all, while the figure still gates what a NEW group is
//! admitted with (`CreateDeploymentGroup`) and what a deployment may override
//! its group's configuration with (`CreateDeployment`), each refused past its
//! own figure with `AlarmsLimitExceededException` (HTTP 400) stating it. What an
//! update is bounded by instead is what one group KEEPS
//! (`state::MAX_RETAINED_ALARMS_PER_DEPLOYMENT_GROUP`), which the same refusal
//! states. The configuration's FORM is refused the same way on every
//! operation that accepts one: `InvalidAlarmConfigException` for an enabled
//! configuration with no alarms, a nameless alarm, an over-long name, or two
//! alarms of one name.
//!
//! The suites are TABLES: the count rows span one alarm, the documented figure,
//! one past it, and well past it in both directions, and the form rows span
//! each case `InvalidAlarmConfigException` documents.
//!
//! The harness is [`crate::codedeploy_lifecycle`]'s, so every case goes through
//! the same front door a client uses.

use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006::state::{
    MAX_ALARM_NAME_LENGTH, MAX_RETAINED_ALARMS_PER_DEPLOYMENT_GROUP,
};

use crate::codedeploy_lifecycle::{error_code, seeded_group, Emulator};

/// The alarm figure the model documents for a deployment group
/// (`smithy_query(query="shape",shape="AlarmConfiguration")`: "A maximum of 10
/// alarms can be added", and `AlarmsLimitExceededException`: "The maximum
/// number of alarms for a deployment group (10) was exceeded").
const DOCUMENTED_GROUP_FIGURE: usize = 10;

/// An enabled alarm configuration carrying `count` distinctly named alarms.
fn alarms(count: usize) -> Value {
    let names: Vec<Value> = (0..count)
        .map(|index| json!({"name": format!("alarm-{index:03}")}))
        .collect();
    json!({"enabled": true, "alarms": names})
}

/// The alarm names the stored deployment group reports.
async fn stored_alarms(emulator: &Emulator, application: &str, group: &str) -> Vec<String> {
    emulator
        .ok(
            "GetDeploymentGroup",
            json!({"applicationName": application, "deploymentGroupName": group}),
        )
        .await["deploymentGroupInfo"]["alarmConfiguration"]["alarms"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|alarm| alarm["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `$.message` of a failure body.
fn message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a string $.message, got {body}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// An update is admitted whatever the count
// ---------------------------------------------------------------------------

/// RULE: `UpdateDeploymentGroup` accepts an alarm configuration past the
/// documented figure, up to what one group keeps, and stores every entry it
/// names, in the order the request named them, so the following read answers
/// all of them.
#[tokio::test]
async fn an_update_stores_every_alarm_it_names_whatever_the_count() {
    let emulator = Emulator::new();
    let application = "alarm-count-app";
    let group = seeded_group(&emulator, application, "Server").await;

    // Rows differ along the number of alarms the update names: one, the
    // documented figure, one past it, well past it, the retention bound itself,
    // and back down again — the last row proves the stored set is REPLACED
    // rather than accumulated.
    let counts: &[usize] = &[
        1,
        DOCUMENTED_GROUP_FIGURE,
        DOCUMENTED_GROUP_FIGURE + 1,
        DOCUMENTED_GROUP_FIGURE * 5,
        64,
        MAX_RETAINED_ALARMS_PER_DEPLOYMENT_GROUP,
        2,
    ];

    for count in counts {
        let case = format!("{count} alarms named by an update");
        let (status, body) = emulator
            .call(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "alarmConfiguration": alarms(*count),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to be accepted, got {status} {body:?}"
        );
        let expected: Vec<String> = (0..*count).map(|index| format!("alarm-{index:03}")).collect();
        let stored = stored_alarms(&emulator, application, &group).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: expected every alarm named to be stored in order, got {stored:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The form rules still hold on an update
// ---------------------------------------------------------------------------

/// RULE: an update is still refused for the FORM of the configuration — the
/// cases `InvalidAlarmConfigException` documents — whatever its length, and a
/// refused update leaves the stored alarm set exactly as it was.
#[tokio::test]
async fn an_update_is_still_refused_for_the_form_of_its_configuration() {
    let emulator = Emulator::new();
    let application = "alarm-form-app";
    let group = seeded_group(&emulator, application, "Server").await;
    emulator
        .ok(
            "UpdateDeploymentGroup",
            json!({
                "applicationName": application,
                "currentDeploymentGroupName": group,
                "alarmConfiguration": alarms(1),
            }),
        )
        .await;
    let before = stored_alarms(&emulator, application, &group).await;

    // A name one character past the 255 the model's `Alarm$name` publishes
    // ("Maximum length is 255 characters").
    let over_long = "a".repeat(MAX_ALARM_NAME_LENGTH + 1);
    // Rows span each documented malformed case, at a length under the
    // documented figure AND at one well past it, so the form gate cannot be
    // mistaken for a count gate.
    let mut duplicate_past_figure: Vec<Value> = (0..DOCUMENTED_GROUP_FIGURE + 4)
        .map(|index| json!({"name": format!("alarm-{index:03}")}))
        .collect();
    duplicate_past_figure.push(json!({"name": "alarm-000"}));
    let cases: Vec<(String, Value)> = vec![
        (
            "enabled with no alarms at all".to_string(),
            json!({"enabled": true, "alarms": []}),
        ),
        (
            "two alarms of one name".to_string(),
            json!({"enabled": true, "alarms": [{"name": "same"}, {"name": "same"}]}),
        ),
        (
            format!(
                "a repeated name in a list of {} entries",
                duplicate_past_figure.len()
            ),
            json!({"enabled": true, "alarms": duplicate_past_figure}),
        ),
        (
            "an alarm with no name".to_string(),
            json!({"enabled": true, "alarms": [{"name": ""}]}),
        ),
        (
            format!("a name of {} characters", over_long.chars().count()),
            json!({"enabled": true, "alarms": [{"name": over_long}]}),
        ),
    ];

    for (case, configuration) in &cases {
        let (status, body) = emulator
            .call(
                "UpdateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "currentDeploymentGroupName": group,
                    "alarmConfiguration": configuration,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidAlarmConfigException"),
            "case {case:?}: expected 400 InvalidAlarmConfigException, got {status} {body:?}"
        );
        let after = stored_alarms(&emulator, application, &group).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused update to leave the alarm set alone, got \
             {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Where the figure IS an admission gate
// ---------------------------------------------------------------------------

/// RULE: the admission gates that do apply are unchanged — a NEW deployment
/// group past the documented figure, and a deployment's override past the
/// deployment figure, are each refused `AlarmsLimitExceededException` (HTTP 400)
/// with the sentence stating the figure that refused it, and nothing is created
/// — and an update past what one group KEEPS is refused the same way.
#[tokio::test]
async fn the_admission_gates_that_apply_still_refuse() {
    let emulator = Emulator::new();
    let application = "alarm-gate-app";
    let group = seeded_group(&emulator, application, "Server").await;

    // A new group: at the documented figure it is admitted, past it refused.
    for count in [DOCUMENTED_GROUP_FIGURE, DOCUMENTED_GROUP_FIGURE + 1] {
        let case = format!("a new group with {count} alarms");
        let name = format!("dg-create-{count}");
        let (status, body) = emulator
            .call(
                "CreateDeploymentGroup",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": name,
                    "serviceRoleArn": "arn:aws:iam::123456789012:role/CodeDeployRole",
                    "alarmConfiguration": alarms(count),
                }),
            )
            .await;
        if count <= DOCUMENTED_GROUP_FIGURE {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the documented figure to be admitted, got {status} \
                 {body:?}"
            );
            let stored = stored_alarms(&emulator, application, &name).await;
            assert_eq!(
                stored.len(),
                count,
                "case {case:?}: expected every alarm to be stored, got {stored:?}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AlarmsLimitExceededException"),
            "case {case:?}: expected the group figure to refuse it, got {status} {body:?}"
        );
        assert_eq!(
            message(&case, &body),
            format!(
                "The maximum number of alarms for a deployment group ({DOCUMENTED_GROUP_FIGURE}) \
                 was exceeded: {count} alarms were specified"
            ),
            "case {case:?}: expected the sentence stating the figure, got {body:?}"
        );
        let (read_status, read_body) = emulator
            .call(
                "GetDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": name}),
            )
            .await;
        assert_eq!(
            (read_status, error_code(&read_body).as_str()),
            (400, "DeploymentGroupDoesNotExistException"),
            "case {case:?}: expected the refused create to have created nothing, got \
             {read_status} {read_body:?}"
        );
    }

    // An update past what one group KEEPS: refused with the sentence stating
    // the figure that refused it, and the stored alarm set is left alone.
    emulator
        .ok(
            "UpdateDeploymentGroup",
            json!({
                "applicationName": application,
                "currentDeploymentGroupName": group,
                "alarmConfiguration": alarms(3),
            }),
        )
        .await;
    let before = stored_alarms(&emulator, application, &group).await;
    let past_retention = MAX_RETAINED_ALARMS_PER_DEPLOYMENT_GROUP + 1;
    let case = format!("an update of {past_retention} alarms");
    let (status, body) = emulator
        .call(
            "UpdateDeploymentGroup",
            json!({
                "applicationName": application,
                "currentDeploymentGroupName": group,
                "alarmConfiguration": alarms(past_retention),
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "AlarmsLimitExceededException"),
        "case {case:?}: expected the retention bound to refuse it, got {status} {body:?}"
    );
    assert_eq!(
        message(&case, &body),
        format!(
            "The maximum number of alarms for a deployment group \
             ({MAX_RETAINED_ALARMS_PER_DEPLOYMENT_GROUP}) was exceeded: {past_retention} alarms \
             were specified"
        ),
        "case {case:?}: expected the sentence stating the figure that refused it, got {body:?}"
    );
    let after = stored_alarms(&emulator, application, &group).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected the refused update to leave the alarm set alone, got {after:?}"
    );

    // A deployment's override past the deployment figure, which the sentence
    // the refusal carries states.
    let (status, body) = emulator
        .call(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "revision": crate::codedeploy_lifecycle::s3_revision("artifacts", "app.zip"),
                "overrideAlarmConfiguration": alarms(51),
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "AlarmsLimitExceededException"),
        "case \"an override of 51 alarms\": expected the deployment figure to refuse it, got \
         {status} {body:?}"
    );
    assert_eq!(
        message("an override of 51 alarms", &body),
        "Deployments cannot contain more than 50 alarms.",
        "case \"an override of 51 alarms\": expected the sentence stating the deployment figure, \
         got {body:?}"
    );
}
