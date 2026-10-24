// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteDashboard`.
//!
//! Every case goes over the wire through the shared harness (`tests/all/emulator.rs`):
//! an `awsJson1_1` `POST /` with this service's published `X-Amz-Target` prefix,
//! dispatched through the same `MultiServiceRouter` the production router serves, so
//! what is asserted here is what a client sees — the HTTP status, the error code and the
//! message.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * `DeleteDashboard` releases exactly the dashboard its identifier addresses, and the
//!   identifier is "the name or ARN for the dashboard"
//!   (`DeleteDashboardRequest$DashboardId`) — a bare name addressing the dashboard of
//!   that name in the Region the call was made in, an ARN the one in the Region it
//!   carries;
//! * an identifier that names no stored dashboard is `ResourceNotFoundException` ("thrown
//!   when the specified resource is not found") with the one fixed sentence every
//!   dashboard operation answers a miss with, and a refused delete releases nothing: so
//!   deleting is not idempotent, and the second delete of one dashboard is that refusal.
//!   An identifier that can name no dashboard at all — a value outside the modelled
//!   `DashboardName` rules, or an `arn:` value that is not a dashboard ARN — is one of
//!   those misses;
//! * "You cannot delete a dashboard that has termination protection enabled": the delete
//!   is refused with the `ConflictException` this operation models and the dashboard is
//!   left exactly as it was, until `UpdateDashboard` turns termination protection off;
//! * a resource-based policy belongs to the resource it is attached to, so the policy
//!   attached to a dashboard goes with the dashboard and one later created with the same
//!   name in the same Region inherits nothing.
//!
//! A delete is read back through `GetDashboard` and `GetResourcePolicy` — the resource as
//! a later caller sees it — and through the second delete's refusal, which is the same
//! fact over the wire.

use serde_json::{json, Value};
use std::thread::sleep;
use std::time::Duration;

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state;

// The emulator-under-test harness is shared by this crate's suites; see
// `tests/all/emulator.rs`.
use super::emulator::{Emulator, Reply, HOME_REGION};

/// Another Region, for the cases that pin a dashboard to the Region it was made in.
const OTHER_REGION: &str = "us-west-2";

/// The status and code a dashboard operation answers an identifier that names no
/// dashboard with: `ResourceNotFoundException`, at the status the model annotates it with
/// (`smithy_query(query="operation", operation="DeleteDashboard")` reports
/// `httpError: 400`), which is also the status the recorded refusal carries.
const NOT_FOUND_STATUS: u16 = 400;

/// The one fixed sentence CloudTrail answers a dashboard miss with, written out here
/// rather than taken from the production constant: an expectation computed by the code
/// under test could not catch that code wording its own refusal.
const NOT_FOUND_MESSAGE: &str = "No matching dashboard found.";

/// The status this service answers its dashboard `ConflictException` with: 400 on the
/// wire, not the 409 the shape's `@httpError` trait annotates (see
/// `state::DASHBOARD_CONFLICT_HTTP_STATUS`, which cites the recorded conflict).
const CONFLICT_STATUS: u16 = 400;

/// A margin on top of [`state::DASHBOARD_SETTLE_AFTER`], so a case that has to wait for
/// the clock does not race the instant a handler reads.
fn past_settle() -> Duration {
    state::DASHBOARD_SETTLE_AFTER + Duration::from_millis(250)
}

/// The ARN a dashboard called `name` in `region` carries, written out from the ARN format
/// the model documents for `DashboardArn` and the emulator's canonical account rather
/// than built by the production helper.
fn dashboard_arn(region: &str, name: &str) -> String {
    format!("arn:aws:cloudtrail:{region}:000000000000:dashboard/{name}")
}

/// One widget, built so the rows below can differ in widget COUNT without repeating the
/// shape. `ViewProperties` is required on `RequestWidget`.
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

/// Create a dashboard called `name` in `region` with `extra` merged into the request.
///
/// A free function rather than an inherent method on the shared [`Emulator`]: the other
/// dashboard suites already extend that type, and two inherent methods of one name on one
/// type cannot both be reached.
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
        .call_in("DeleteDashboard", region, &json!({"DashboardId": dashboard_id}))
        .await
}

/// Read a dashboard back by name or ARN, calling from `region`.
async fn read(emulator: &Emulator, region: &str, dashboard_id: &str) -> Reply {
    emulator
        .call_in("GetDashboard", region, &json!({"DashboardId": dashboard_id}))
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

/// Every dashboard key the registry holds, in registry order — the SAME store the
/// handler writes, since the engine is shared.
fn dashboard_keys(emulator: &Emulator) -> Vec<state::DashboardKey> {
    let state = emulator.state();
    let store = state.lock();
    store.dashboards.keys().cloned().collect()
}

/// Assert `reply` is the modelled failure `code` with HTTP `status` AND the exact
/// `message`.
fn expect_refusal(case: &str, reply: &Reply, status: u16, code: &str, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code(), reply.message()),
        (status, code, message),
        "case {case:?}: expected HTTP {status} {code} {message:?}, got {reply:?}"
    );
}

/// RULE: `DeleteDashboard` releases exactly the dashboard its identifier addresses — by
/// name out of the Region the call was made in, or by ARN out of the Region the ARN pins
/// — and leaves every other dashboard in the registry. Every later read of it, and a
/// second delete of it, is the not-found refusal.
///
/// The rows differ in the FORM of the identifier (bare name or ARN), in the Region the
/// dashboard lives in, in the dashboard TYPE (the reserved Highlights name names the
/// dashboard CloudTrail manages), in how many widgets and tags it carries, in whether it
/// has a refresh schedule and in the length of its name.
#[tokio::test]
async fn deleting_a_dashboard_releases_exactly_that_dashboard() {
    let emulator = Emulator::new();

    // (case, Region the dashboard lives in, name, address by ARN, extra request members)
    let rows: Vec<(&str, &str, String, bool, Value)> = vec![
        (
            "by name, no optional member at all",
            HOME_REGION,
            "plain-dashboard".to_string(),
            false,
            json!({}),
        ),
        (
            "by ARN, one widget",
            HOME_REGION,
            "one_widget".to_string(),
            true,
            json!({"Widgets": widgets(1)}),
        ),
        (
            "by name, in another Region, widgets and tags",
            OTHER_REGION,
            "other-region-dash".to_string(),
            false,
            json!({"Widgets": widgets(2), "TagsList": tags(1)}),
        ),
        (
            "by ARN, in another Region, tags only",
            OTHER_REGION,
            "other_region_arn".to_string(),
            true,
            json!({"TagsList": tags(3)}),
        ),
        (
            "by name, as many widgets as a custom dashboard admits",
            HOME_REGION,
            "ten-widgets".to_string(),
            false,
            json!({"Widgets": widgets(state::CUSTOM_DASHBOARD_WIDGET_LIMIT)}),
        ),
        (
            "by ARN, the shortest name the model admits, with a refresh schedule",
            HOME_REGION,
            "d".repeat(state::DASHBOARD_NAME_MIN_LEN),
            true,
            json!({
                "RefreshSchedule": {"Frequency": {"Unit": "HOURS", "Value": 12}, "Status": "ENABLED"},
                "Widgets": widgets(1),
            }),
        ),
        (
            "by name, the longest name the model admits",
            HOME_REGION,
            "d".repeat(state::DASHBOARD_NAME_MAX_LEN),
            false,
            json!({}),
        ),
        (
            // The one managed dashboard: its name is reserved, it carries no
            // caller-composed widgets and it needs the documented refresh schedule.
            "by name, the CloudTrail-managed Highlights dashboard",
            OTHER_REGION,
            state::HIGHLIGHTS_DASHBOARD_NAME.to_string(),
            false,
            json!({
                "RefreshSchedule": {
                    "Frequency": {
                        "Unit": "HOURS",
                        "Value": state::HIGHLIGHTS_REFRESH_FREQUENCY_VALUE,
                    },
                    "Status": "ENABLED",
                },
            }),
        ),
    ];

    // Create every row's dashboard first, so each delete has to pick its own out of a
    // registry that holds the others.
    let mut seeded: Vec<(&str, String, &str, String)> = Vec::new();
    for (case, region, name, by_arn, extra) in &rows {
        let created = create(&emulator, region, name, extra).await;
        let body = created.expect_ok(case);
        let expected_arn = dashboard_arn(region, name);
        assert_eq!(
            body.get("DashboardArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: the created dashboard must carry its own ARN, got {body:?}"
        );
        let identifier = if *by_arn {
            expected_arn
        } else {
            name.clone()
        };
        seeded.push((case, name.clone(), region, identifier));
    }
    // One more dashboard sharing a name with a seeded one but living in the other
    // Region: deleting in one Region must not touch it.
    let shared = "shared-name";
    for region in [HOME_REGION, OTHER_REGION] {
        create(&emulator, region, shared, &json!({}))
            .await
            .expect_ok("seed the dashboard that shares a name across Regions");
    }

    for (case, name, region, identifier) in &seeded {
        let before = dashboard_keys(&emulator);
        let deleted = delete(&emulator, region, identifier).await;
        let body = deleted.expect_ok(case);
        assert_eq!(
            body,
            &json!({}),
            "case {case:?}: DeleteDashboard reports no members, got {body:?}"
        );

        let after = dashboard_keys(&emulator);
        let key: state::DashboardKey = (name.clone(), (*region).to_string());
        let expected: Vec<state::DashboardKey> =
            before.iter().filter(|held| **held != key).cloned().collect();
        assert_eq!(
            after, expected,
            "case {case:?}: deleting {identifier} must release exactly {key:?}, got {after:?}"
        );

        // The dashboard is gone for every later caller, and deleting is not idempotent.
        expect_refusal(
            case,
            &read(&emulator, region, identifier).await,
            NOT_FOUND_STATUS,
            "ResourceNotFoundException",
            NOT_FOUND_MESSAGE,
        );
        expect_refusal(
            case,
            &delete(&emulator, region, identifier).await,
            NOT_FOUND_STATUS,
            "ResourceNotFoundException",
            NOT_FOUND_MESSAGE,
        );
    }

    // The two same-name dashboards in different Regions are untouched by all of the
    // above: a dashboard is addressed in one Region only.
    assert_eq!(
        dashboard_keys(&emulator),
        vec![
            (shared.to_string(), HOME_REGION.to_string()),
            (shared.to_string(), OTHER_REGION.to_string()),
        ],
        "the dashboards no row addressed must survive every delete"
    );
}

/// RULE: an identifier that names no stored dashboard is `ResourceNotFoundException` with
/// the service's fixed sentence, and the refused request releases nothing. A value that
/// can name no dashboard at all — one outside the modelled `DashboardName` rules, or an
/// `arn:` value that is not a dashboard ARN — is one of those misses rather than a
/// separate refusal.
///
/// The rows differ in the FORM of the identifier, in the Region it pins, in which of the
/// `DashboardName` rules the bare value breaks (its length, below the minimum and past
/// the maximum, and its character set), in the case of its letters, and in WHICH part of
/// the ARN form is wrong (the resource type, the resource prefix, the field count).
/// Every row satisfies the modelled `DashboardArn` pattern `^[a-zA-Z0-9._/\-:]+$`, so the
/// boundary validator passes it to the handler.
#[tokio::test]
async fn an_identifier_naming_no_dashboard_is_not_found() {
    let emulator = Emulator::new();
    create(&emulator, HOME_REGION, "existing-dash", &json!({}))
        .await
        .expect_ok("the dashboard the rows below fail to name");
    create(&emulator, OTHER_REGION, "other-dash", &json!({}))
        .await
        .expect_ok("a dashboard in another Region");
    let before = dashboard_keys(&emulator);

    let rows: Vec<(&str, String)> = vec![
        ("a name no dashboard carries", "absent-dash".to_string()),
        (
            "an ARN in the calling Region for an absent dashboard",
            dashboard_arn(HOME_REGION, "absent-dash"),
        ),
        (
            "the ARN of a dashboard that lives in another Region",
            dashboard_arn(OTHER_REGION, "existing-dash"),
        ),
        (
            "the bare name of a dashboard that lives in another Region",
            "other-dash".to_string(),
        ),
        (
            "the stored name in upper case, which is a different name",
            "EXISTING-DASH".to_string(),
        ),
        (
            // `DashboardName` is `^[a-zA-Z0-9_\-]+$`, so a dot can name no dashboard —
            // though `DashboardArn`, the member's own target, admits it.
            "a bare value outside the DashboardName character set",
            "existing.dash".to_string(),
        ),
        (
            "a bare value one character below the modelled minimum name length",
            "d".repeat(state::DASHBOARD_NAME_MIN_LEN - 1),
        ),
        (
            "a bare value one character past the modelled maximum name length",
            "d".repeat(state::DASHBOARD_NAME_MAX_LEN + 1),
        ),
        (
            "an ARN naming another CloudTrail resource type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:channel/01234567890"),
        ),
        (
            "an ARN whose resource is not dashboard/<name>",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:dashboardwrong"),
        ),
        (
            "an ARN with an empty dashboard name",
            dashboard_arn(HOME_REGION, ""),
        ),
        (
            "an ARN with one field too few",
            format!("arn:aws:cloudtrail:{HOME_REGION}:dashboard/existing-dash"),
        ),
        (
            "an ARN naming another service",
            format!("arn:aws:s3:{HOME_REGION}:000000000000:dashboard/existing-dash"),
        ),
    ];

    for (case, identifier) in &rows {
        expect_refusal(
            case,
            &delete(&emulator, HOME_REGION, identifier).await,
            NOT_FOUND_STATUS,
            "ResourceNotFoundException",
            NOT_FOUND_MESSAGE,
        );
        assert_eq!(
            dashboard_keys(&emulator),
            before,
            "case {case:?}: a refused delete must release nothing, got {:?}",
            dashboard_keys(&emulator)
        );
    }

    // The control: the dashboard those identifiers failed to name is still deletable by
    // the identifier that does name it.
    delete(&emulator, HOME_REGION, "existing-dash")
        .await
        .expect_ok("the control");
}

/// RULE: a dashboard with termination protection enabled cannot be deleted — the delete
/// is refused with the modelled `ConflictException` and the dashboard is left exactly as
/// it was — and turning termination protection off through `UpdateDashboard` makes it
/// deletable again.
///
/// The rows differ in HOW termination protection came to be on (set by the create, or
/// turned on later by an update), in the Region the dashboard lives in, in the form of
/// the identifier the delete addresses it by, and in what else the dashboard carries.
#[tokio::test]
async fn termination_protection_refuses_the_delete() {
    let emulator = Emulator::new();

    // (case, Region, name, protected by the create itself, address by ARN, extra members)
    let rows: Vec<(&str, &str, &str, bool, bool, Value)> = vec![
        (
            "protected by the create, addressed by name",
            HOME_REGION,
            "protected-at-create",
            true,
            false,
            json!({}),
        ),
        (
            "protected by a later update, addressed by ARN",
            HOME_REGION,
            "protected_by_update",
            false,
            true,
            json!({"Widgets": widgets(2), "TagsList": tags(2)}),
        ),
        (
            "protected by the create in another Region, addressed by ARN",
            OTHER_REGION,
            "protected-other-region",
            true,
            true,
            json!({"Widgets": widgets(1)}),
        ),
    ];

    for (case, region, name, protected_at_create, by_arn, extra) in &rows {
        let mut request = extra.clone();
        if *protected_at_create {
            request["TerminationProtectionEnabled"] = json!(true);
        }
        create(&emulator, region, name, &request)
            .await
            .expect_ok(case);
        if !protected_at_create {
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
        let identifier = if *by_arn { arn.clone() } else { name.to_string() };
        let refused = delete(&emulator, region, &identifier).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (CONFLICT_STATUS, "ConflictException"),
            "case {case:?}: a protected dashboard cannot be deleted, got {refused:?}"
        );
        // AWS publishes no wording for this refusal, so what is asserted is that the
        // message names the dashboard the request addressed — the caller has to know
        // which resource refused.
        assert!(
            refused.message().contains(&arn),
            "case {case:?}: the refusal must name the dashboard {arn}, got {:?}",
            refused.message()
        );

        // Nothing was released: the dashboard reads back exactly as it was, still
        // protected.
        let read_back = read(&emulator, region, &identifier).await;
        let body = read_back.expect_ok(case);
        assert_eq!(
            body.get("TerminationProtectionEnabled"),
            Some(&json!(true)),
            "case {case:?}: the refused delete must leave the dashboard protected, got {body:?}"
        );

        // Termination protection off, and the same delete goes through.
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
            .expect_ok(case);
        expect_refusal(
            case,
            &read(&emulator, region, &identifier).await,
            NOT_FOUND_STATUS,
            "ResourceNotFoundException",
            NOT_FOUND_MESSAGE,
        );
    }
}

/// RULE: the resource-based policy attached to a dashboard belongs to that dashboard, so
/// deleting the dashboard releases its policy too: the policy is unreachable afterwards,
/// and a dashboard later created with the same name in the same Region carries no policy
/// nobody attached to it.
///
/// The rows differ in the Region the dashboard lives in, in the Region the policy and the
/// delete are called from, in the form of the identifier the delete addresses the
/// dashboard by, and in the policy document.
#[tokio::test]
async fn deleting_a_dashboard_releases_its_resource_policy() {
    let emulator = Emulator::new();

    // (case, Region the dashboard lives in, name, address the delete by ARN, the policy)
    let rows: Vec<(&str, &str, &str, bool, String)> = vec![
        (
            "a dashboard in the calling Region, deleted by name",
            HOME_REGION,
            "policy-holder",
            false,
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowOneAccount",
                    "Effect": "Allow",
                    "Principal": {"AWS": "arn:aws:iam::111122223333:root"},
                    "Action": "cloudtrail:GetDashboard",
                }],
            })
            .to_string(),
        ),
        (
            "a dashboard in another Region, deleted by ARN",
            OTHER_REGION,
            "policy_holder_other",
            true,
            json!({
                "Statement": [{
                    "Effect": "Deny",
                    "NotPrincipal": {"AWS": ["arn:aws:iam::111122223333:root"]},
                    "NotAction": "cloudtrail:StartDashboardRefresh",
                }],
            })
            .to_string(),
        ),
    ];

    for (case, region, name, by_arn, document) in &rows {
        let arn = dashboard_arn(region, name);
        create(&emulator, region, name, &json!({"Widgets": widgets(1)}))
            .await
            .expect_ok(case);
        // A write to a resource CloudTrail is still loading is a conflict, so the policy
        // is attached once the dashboard has finished loading.
        sleep(past_settle());

        emulator
            .call(
                "PutResourcePolicy",
                &json!({"ResourceArn": arn, "ResourcePolicy": document}),
            )
            .await
            .expect_ok(case);
        let stored = emulator
            .call("GetResourcePolicy", &json!({"ResourceArn": arn}))
            .await;
        let body = stored.expect_ok(case);
        assert_eq!(
            body.get("ResourcePolicy").and_then(Value::as_str),
            Some(document.as_str()),
            "case {case:?}: the policy must be attached before the delete, got {body:?}"
        );

        let identifier = if *by_arn { arn.clone() } else { name.to_string() };
        delete(&emulator, region, &identifier)
            .await
            .expect_ok(case);

        // The dashboard is gone, so its policy is not addressable at all: the
        // resource-policy surface words a dashboard it cannot find in its own sentence.
        expect_refusal(
            case,
            &emulator
                .call("GetResourcePolicy", &json!({"ResourceArn": arn}))
                .await,
            NOT_FOUND_STATUS,
            "ResourceNotFoundException",
            "The dashboard was not found.",
        );

        // A dashboard created with the same name in the same Region is a NEW dashboard
        // and inherits nothing: it carries no policy at all, which is the one 404 this
        // surface models.
        create(&emulator, region, name, &json!({}))
            .await
            .expect_ok(case);
        sleep(past_settle());
        let reused = emulator
            .call("GetResourcePolicy", &json!({"ResourceArn": arn}))
            .await;
        assert_eq!(
            (reused.status, reused.error_code()),
            (404, "ResourcePolicyNotFoundException"),
            "case {case:?}: the new dashboard must carry no inherited policy, got {reused:?}"
        );
    }
}
