// SPDX-License-Identifier: CC0-1.0

//! End-to-end cases pinning the WORDING of the refusal `DeleteDashboard` answers a
//! termination-protected dashboard with.
//!
//! Every case goes over the wire through the shared harness (`tests/all/emulator.rs`):
//! an `awsJson1_1` `POST /` with this service's published `X-Amz-Target` prefix,
//! dispatched through the same `MultiServiceRouter` the production router serves, so
//! what is asserted here is what a client sees — the HTTP status, the error code and
//! the message bytes.
//!
//! The rule under test: "You cannot delete a dashboard that has termination protection
//! enabled" (the model doc for `DeleteDashboard`) is refused with HTTP 400 and the error
//! code `ValidationException`, carrying the ONE fixed sentence the service publishes for
//! that refusal — no dashboard name, ARN, Region or account id is interpolated into it,
//! so every protected dashboard is refused with the same bytes and a client that matches
//! on the text recognizes the refusal.
//!
//! The rows span what the dashboard IS and how the request addresses it — how its
//! protection came to be on (the service default, the create stating it, a later
//! update turning it on), the Region it lives in, the form of the identifier (bare name
//! or ARN), the length and character set of its name, how many widgets and tags it
//! carries, whether it has a refresh schedule, and the dashboard type (the reserved
//! Highlights name names the dashboard CloudTrail manages) — because the rule
//! generalises over every one of them: the sentence does not change.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state;

use super::emulator::{Emulator, Reply, HOME_REGION};

/// A second Region, so the table spans the Region a dashboard lives in.
const OTHER_REGION: &str = "us-west-2";

/// The status and code CloudTrail refuses a delete of a protected dashboard with: the
/// recorded refusal is HTTP 400 with the error code `ValidationException` (none of the
/// three errors `DeleteDashboard` models carries it).
const PROTECTED_STATUS: u16 = 400;
/// See [`PROTECTED_STATUS`].
const PROTECTED_CODE: &str = "ValidationException";

/// The one fixed sentence that refusal carries, written out here rather than read from
/// the production constant: an expectation computed by the code under test could not
/// catch that code wording its own refusal.
const PROTECTED_MESSAGE: &str = "This dashboard has termination protection enabled.";

/// The account id every ARN this emulator mints carries, written out rather than built
/// by the production helper, so a row can assert the refusal reports no account.
const ACCOUNT_ID: &str = "000000000000";

/// One widget, so a row can differ in widget COUNT without repeating the shape.
/// `ViewProperties` is required on `RequestWidget`.
fn widget(index: usize) -> Value {
    json!({
        "QueryStatement": format!("SELECT eventSource, count(*) FROM EDS GROUP BY eventSource -- {index}"),
        "QueryParameters": ["$StartTime$", "$EndTime$"],
        "ViewProperties": {"type": "bar", "colors": format!("{index}")},
    })
}

/// `count` widgets, as a request carries them.
fn widgets(count: usize) -> Value {
    Value::Array((0..count).map(widget).collect())
}

/// `count` tags, as a request carries them.
fn tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"Key": format!("key-{index}"), "Value": format!("value-{index}")}))
            .collect(),
    )
}

/// The ARN a dashboard called `name` in `region` carries, written out from the ARN
/// format the model documents for `DashboardArn` rather than built by the production
/// helper.
fn dashboard_arn(region: &str, name: &str) -> String {
    format!("arn:aws:cloudtrail:{region}:{ACCOUNT_ID}:dashboard/{name}")
}

/// Create a dashboard called `name` in `region` with `extra` merged in beside `Name`.
async fn create(emulator: &Emulator, region: &str, name: &str, extra: &Value) -> Reply {
    let mut request = json!({"Name": name});
    if let Some(members) = extra.as_object() {
        for (key, value) in members {
            request[key] = value.clone();
        }
    }
    emulator.call_in("CreateDashboard", region, &request).await
}

/// Delete the dashboard `dashboard_id` addresses, calling from `region`.
async fn delete(emulator: &Emulator, region: &str, dashboard_id: &str) -> Reply {
    emulator
        .call_in(
            "DeleteDashboard",
            region,
            &json!({"DashboardId": dashboard_id}),
        )
        .await
}

/// Patch a dashboard through `UpdateDashboard`, calling from `region`.
async fn update(emulator: &Emulator, region: &str, dashboard_id: &str, patch: &Value) -> Reply {
    let mut request = json!({"DashboardId": dashboard_id});
    if let Some(members) = patch.as_object() {
        for (key, value) in members {
            request[key] = value.clone();
        }
    }
    emulator.call_in("UpdateDashboard", region, &request).await
}

/// One row: a dashboard, how its protection came to be on, and how the delete addresses
/// it.
struct Row {
    case: &'static str,
    region: &'static str,
    name: String,
    /// Members merged into the create request beside `Name`.
    extra: Value,
    /// Whether the row turns protection on through a later `UpdateDashboard` rather
    /// than through the create.
    protect_by_update: bool,
    /// Whether the delete addresses the dashboard by ARN rather than by name.
    by_arn: bool,
}

/// RULE: a delete refused because the addressed dashboard has termination protection
/// enabled carries HTTP 400, the error code `ValidationException` and ONE fixed
/// sentence, with no part of the request or of the dashboard interpolated into it.
#[tokio::test]
async fn the_protected_delete_refusal_is_one_fixed_sentence() {
    let emulator = Emulator::new();

    let rows: Vec<Row> = vec![
        Row {
            case: "protected by the service default, addressed by name",
            region: HOME_REGION,
            name: "default-protected".to_string(),
            extra: json!({}),
            protect_by_update: false,
            by_arn: false,
        },
        Row {
            case: "protected by the create, addressed by ARN, widgets and tags",
            region: HOME_REGION,
            name: "stated_protected".to_string(),
            extra: json!({
                "TerminationProtectionEnabled": true,
                "Widgets": widgets(2),
                "TagsList": tags(3),
            }),
            protect_by_update: false,
            by_arn: true,
        },
        Row {
            case: "protected by a later update, in another Region, addressed by ARN",
            region: OTHER_REGION,
            name: "updated-protected".to_string(),
            extra: json!({"TerminationProtectionEnabled": false, "Widgets": widgets(1)}),
            protect_by_update: true,
            by_arn: true,
        },
        Row {
            case: "protected by a later update, in another Region, addressed by name, \
                   with a refresh schedule",
            region: OTHER_REGION,
            name: "scheduled_protected".to_string(),
            extra: json!({
                "TerminationProtectionEnabled": false,
                "Widgets": widgets(1),
                "RefreshSchedule": {"Frequency": {"Unit": "HOURS", "Value": 12}, "Status": "ENABLED"},
            }),
            protect_by_update: true,
            by_arn: false,
        },
        Row {
            // The shortest name the model admits (`DashboardName` length 3..=128).
            case: "the shortest name the model admits, addressed by name",
            region: HOME_REGION,
            name: "d".repeat(state::DASHBOARD_NAME_MIN_LEN),
            extra: json!({"TerminationProtectionEnabled": true}),
            protect_by_update: false,
            by_arn: false,
        },
        Row {
            // The longest name the model admits: a long name is not reported either.
            case: "the longest name the model admits, addressed by ARN",
            region: HOME_REGION,
            name: "d".repeat(state::DASHBOARD_NAME_MAX_LEN),
            extra: json!({"TerminationProtectionEnabled": true}),
            protect_by_update: false,
            by_arn: true,
        },
        Row {
            // As many widgets as a custom dashboard admits.
            case: "as many widgets as a custom dashboard admits, addressed by name",
            region: HOME_REGION,
            name: "ten-widgets".to_string(),
            extra: json!({
                "TerminationProtectionEnabled": true,
                "Widgets": widgets(state::CUSTOM_DASHBOARD_WIDGET_LIMIT),
            }),
            protect_by_update: false,
            by_arn: false,
        },
        Row {
            // The one dashboard CloudTrail manages: its name is reserved, it carries no
            // caller-composed widgets and it needs the documented refresh schedule.
            case: "the CloudTrail-managed Highlights dashboard, addressed by ARN",
            region: OTHER_REGION,
            name: state::HIGHLIGHTS_DASHBOARD_NAME.to_string(),
            extra: json!({
                "TerminationProtectionEnabled": true,
                "RefreshSchedule": {
                    "Frequency": {
                        "Unit": "HOURS",
                        "Value": state::HIGHLIGHTS_REFRESH_FREQUENCY_VALUE,
                    },
                    "Status": "ENABLED",
                },
            }),
            protect_by_update: false,
            by_arn: true,
        },
    ];

    for row in &rows {
        let Row {
            case,
            region,
            name,
            extra,
            protect_by_update,
            by_arn,
        } = row;

        create(&emulator, region, name, extra).await.expect_ok(case);
        if *protect_by_update {
            update(
                &emulator,
                region,
                name,
                &json!({"TerminationProtectionEnabled": true}),
            )
            .await
            .expect_ok(case);
        }

        let arn = dashboard_arn(region, name);
        let identifier = if *by_arn { arn.clone() } else { name.clone() };
        let refused = delete(&emulator, region, &identifier).await;

        assert_eq!(
            (refused.status, refused.error_code(), refused.message()),
            (PROTECTED_STATUS, PROTECTED_CODE, PROTECTED_MESSAGE),
            "case {case:?}: expected HTTP {PROTECTED_STATUS} {PROTECTED_CODE} \
             {PROTECTED_MESSAGE:?}, got {refused:?}"
        );
        // Stated separately from the equality above so a future wording change cannot
        // quietly start reporting the resource: the refusal names no part of the
        // request or of the dashboard it addressed.
        for (what, value) in [
            ("the dashboard name", name.as_str()),
            ("the dashboard ARN", arn.as_str()),
            ("the Region", region),
            ("the account id", ACCOUNT_ID),
        ] {
            assert!(
                !refused.message().contains(value),
                "case {case:?}: the refusal must not report {what} ({value}), got {:?}",
                refused.message()
            );
        }

        // The refusal released nothing: turning protection off makes the SAME delete go
        // through, so every row above was refused for the protection alone.
        update(
            &emulator,
            region,
            &identifier,
            &json!({"TerminationProtectionEnabled": false}),
        )
        .await
        .expect_ok(case);
        delete(&emulator, region, &identifier)
            .await
            .expect_ok(&format!("{case}: the delete once protection is off"));
    }
}
