// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a dashboard's termination protection and for the
//! resource-policy surface of a dashboard the create has just returned.
//!
//! Every case goes over the wire through the shared [`super::emulator::Emulator`]: an
//! `awsJson1_1` `POST /` with the `X-Amz-Target` an AWS SDK sends, dispatched through
//! the same `MultiServiceRouter` the production router serves — so what is asserted is
//! what a client sees, including the HTTP status, the error code and the message.
//!
//! The rules under test:
//!
//! * `TerminationProtectionEnabled` is the request's value, and a request that states
//!   nothing leaves the dashboard PROTECTED; the create response, every later read and
//!   the stored record report that one value;
//! * "You cannot delete a dashboard that has termination protection enabled": the
//!   delete is refused with the error code `ValidationException` at HTTP 400, the
//!   dashboard is left exactly as it was, and clearing the flag through
//!   `UpdateDashboard` makes the same delete go through;
//! * a dashboard `CreateDashboard` has returned is READY: a resource-based policy
//!   attaches to it with no wait, is reported back and read back byte for byte, and is
//!   detached again — while a document that is not a policy is refused with
//!   `ResourcePolicyNotValidException`;
//! * a resource that carries no policy answers one FIXED sentence, from
//!   `GetResourcePolicy` and `DeleteResourcePolicy` alike, whatever kind of resource it
//!   is.
//!
//! A channel and an event data store are SEEDED through the shared seam in
//! `super::resource_policy`, because `CreateChannel` and `CreateEventDataStore` are
//! still scaffold stubs.

use serde_json::{json, Value};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state;

use super::emulator::{Emulator, Reply, HOME_REGION};
use super::resource_policy::{channel, event_data_store, get, put};

/// A second Region, so a table spans the Region a dashboard lives in as well as the
/// members of its request.
const OTHER_REGION: &str = "us-west-2";

/// The status and code CloudTrail refuses a delete its target's own configuration
/// forbids with, from the recorded delete of a protected dashboard.
const PROTECTED_STATUS: u16 = 400;
/// See [`PROTECTED_STATUS`].
const PROTECTED_CODE: &str = "ValidationException";

/// The one fixed sentence a resource carrying no resource-based policy is refused
/// with, written out here rather than read from the production constant: an
/// expectation computed by the code under test could not catch that code wording its
/// own refusal.
const NO_POLICY_MESSAGE: &str = "There is no policy attached to the given resource.";

/// The one fixed sentence a document that is not a resource-based policy is refused
/// with, written out here for the same reason as [`NO_POLICY_MESSAGE`].
const NOT_A_POLICY_MESSAGE: &str = "The given resource policy is not valid.";

/// One widget, so a row can carry widgets without repeating the shape.
/// `ViewProperties` is required on `RequestWidget`.
fn widget(index: usize) -> Value {
    json!({
        "QueryStatement": format!("SELECT eventSource, count(*) FROM EDS GROUP BY eventSource -- {index}"),
        "QueryParameters": ["$StartTime$", "$EndTime$"],
        "ViewProperties": {"type": "bar", "colors": format!("{index}")},
    })
}

/// The refresh schedule the Highlights dashboard requires, from the model doc for
/// `CreateDashboardRequest$RefreshSchedule`.
fn highlights_schedule() -> Value {
    json!({"Frequency": {"Unit": "HOURS", "Value": 6}, "Status": "ENABLED"})
}

/// The ARN a dashboard called `name` in `region` carries, written out from the ARN
/// format the model documents for `DashboardArn` rather than built by the production
/// helper.
fn dashboard_arn(region: &str, name: &str) -> String {
    format!("arn:aws:cloudtrail:{region}:000000000000:dashboard/{name}")
}

/// Create a dashboard in `region` from `request` and hand back the reply.
async fn create(emulator: &Emulator, region: &str, request: &Value) -> Reply {
    emulator.call_in("CreateDashboard", region, request).await
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

/// Delete a dashboard by name or ARN, calling from `region`.
async fn delete(emulator: &Emulator, region: &str, dashboard_id: &str) -> Reply {
    emulator
        .call_in(
            "DeleteDashboard",
            region,
            &json!({"DashboardId": dashboard_id}),
        )
        .await
}

/// Detach the resource policy of `resource_arn`.
async fn detach(emulator: &Emulator, resource_arn: &str) -> Reply {
    emulator
        .call("DeleteResourcePolicy", &json!({"ResourceArn": resource_arn}))
        .await
}

/// The termination-protection flag the registry holds for the dashboard `name` in
/// `region` — the stored record, not only the answer a read gives.
fn stored_protection(emulator: &Emulator, name: &str, region: &str) -> Option<bool> {
    let state = emulator.state();
    let store = state.lock();
    store
        .dashboards
        .get(&(name.to_string(), region.to_string()))
        .map(|record| record.termination_protection_enabled)
}

/// RULE: `TerminationProtectionEnabled` is the value the request states, and a request
/// that states nothing leaves the dashboard protected — and the create response, every
/// later read and the stored record all report that one value.
///
/// The rows differ along whether the member is present and what it says, the Region the
/// dashboard lives in, the dashboard TYPE (the reserved Highlights name names the
/// dashboard CloudTrail manages) and what else the request carries.
#[tokio::test]
async fn termination_protection_is_the_requests_value_and_on_when_the_request_states_nothing() {
    let emulator = Emulator::new();

    // (case, Region, request, the protection the dashboard must report)
    let rows: Vec<(&str, &str, Value, bool)> = vec![
        (
            "the member absent, nothing else either",
            HOME_REGION,
            json!({"Name": "defaulted-dash"}),
            state::DASHBOARD_TERMINATION_PROTECTION_DEFAULT,
        ),
        (
            "the member absent, widgets and tags present",
            HOME_REGION,
            json!({
                "Name": "defaulted_with_members",
                "Widgets": [widget(0), widget(1)],
                "TagsList": [{"Key": "team", "Value": "security"}],
            }),
            state::DASHBOARD_TERMINATION_PROTECTION_DEFAULT,
        ),
        (
            "the member absent, in another Region",
            OTHER_REGION,
            json!({"Name": "defaulted-elsewhere"}),
            state::DASHBOARD_TERMINATION_PROTECTION_DEFAULT,
        ),
        (
            "the member absent on the dashboard CloudTrail manages",
            HOME_REGION,
            json!({
                "Name": state::HIGHLIGHTS_DASHBOARD_NAME,
                "RefreshSchedule": highlights_schedule(),
            }),
            state::DASHBOARD_TERMINATION_PROTECTION_DEFAULT,
        ),
        (
            "stated true",
            HOME_REGION,
            json!({"Name": "protected-dash", "TerminationProtectionEnabled": true}),
            true,
        ),
        (
            "stated false",
            HOME_REGION,
            json!({"Name": "unprotected-dash", "TerminationProtectionEnabled": false}),
            false,
        ),
        (
            "stated false, with widgets and a schedule",
            OTHER_REGION,
            json!({
                "Name": "unprotected_with_members",
                "TerminationProtectionEnabled": false,
                "Widgets": [widget(2)],
                "RefreshSchedule": {"Frequency": {"Unit": "DAYS", "Value": 1}, "Status": "DISABLED"},
            }),
            false,
        ),
    ];

    for (case, region, request, expected) in &rows {
        let name = request["Name"].as_str().expect("the row names a dashboard");
        let created = create(&emulator, region, request).await;
        let body = created.expect_ok(case);
        assert_eq!(
            body.get("TerminationProtectionEnabled"),
            Some(&json!(expected)),
            "case {case:?}: expected TerminationProtectionEnabled {expected}, got {body:?}"
        );

        // The stored record, and the dashboard as a later caller reads it back — by
        // name and by ARN — carry the same one value.
        assert_eq!(
            stored_protection(&emulator, name, region),
            Some(*expected),
            "case {case:?}: expected the registry to hold TerminationProtectionEnabled \
             {expected}, got {:?}",
            stored_protection(&emulator, name, region)
        );
        for (reader, identifier) in [
            ("by name", name.to_string()),
            ("by ARN", dashboard_arn(region, name)),
        ] {
            let fetched = read(&emulator, region, &identifier).await;
            let fetched_body = fetched.expect_ok(case);
            assert_eq!(
                fetched_body.get("TerminationProtectionEnabled"),
                Some(&json!(expected)),
                "case {case:?} read {reader}: expected TerminationProtectionEnabled \
                 {expected}, got {fetched_body:?}"
            );
        }
    }
}

/// One delete case: a dashboard, how its termination protection came to be where it
/// is, and how the delete addresses it.
struct DeleteCase {
    case: &'static str,
    region: &'static str,
    name: &'static str,
    /// Members merged into the create request beside `Name`.
    extra: Value,
    /// An `UpdateDashboard` patch applied before the delete, if the row needs one.
    patch: Option<Value>,
    /// Whether the delete addresses the dashboard by ARN rather than by name.
    by_arn: bool,
    /// Whether the dashboard is protected when the delete arrives.
    protected: bool,
}

/// RULE: "You cannot delete a dashboard that has termination protection enabled" — the
/// delete is refused with `ValidationException` (HTTP 400) and releases nothing, and
/// clearing the flag through `UpdateDashboard` makes the same delete go through. A
/// dashboard whose protection is off is deleted straight away.
///
/// The rows differ along HOW protection came to be on (the request's default, the
/// request stating it, or a later update turning it on), the form of the identifier the
/// delete addresses the dashboard by, the Region it lives in, and the one row whose
/// protection is off.
#[tokio::test]
async fn deleting_a_protected_dashboard_is_refused_as_a_validation_error() {
    let emulator = Emulator::new();

    let rows: Vec<DeleteCase> = vec![
        DeleteCase {
            case: "protected by the default, addressed by name",
            region: HOME_REGION,
            name: "default-protected",
            extra: json!({}),
            patch: None,
            by_arn: false,
            protected: true,
        },
        DeleteCase {
            case: "protected by the request, addressed by ARN",
            region: HOME_REGION,
            name: "stated_protected",
            extra: json!({"TerminationProtectionEnabled": true, "Widgets": [widget(0)]}),
            patch: None,
            by_arn: true,
            protected: true,
        },
        DeleteCase {
            case: "protected by a later update, in another Region, addressed by ARN",
            region: OTHER_REGION,
            name: "updated-protected",
            extra: json!({"TerminationProtectionEnabled": false}),
            patch: Some(json!({"TerminationProtectionEnabled": true})),
            by_arn: true,
            protected: true,
        },
        DeleteCase {
            case: "protection off, addressed by name",
            region: OTHER_REGION,
            name: "not_protected",
            extra: json!({"TerminationProtectionEnabled": false}),
            patch: None,
            by_arn: false,
            protected: false,
        },
    ];

    for row in &rows {
        let DeleteCase {
            case,
            region,
            name,
            extra,
            patch,
            by_arn,
            protected,
        } = row;
        let mut request = json!({"Name": name});
        if let Some(members) = extra.as_object() {
            for (key, value) in members {
                request[key] = value.clone();
            }
        }
        create(&emulator, region, &request).await.expect_ok(case);
        if let Some(patch) = patch {
            update(&emulator, region, name, patch).await.expect_ok(case);
        }

        let arn = dashboard_arn(region, name);
        let identifier = if *by_arn { arn.clone() } else { name.to_string() };

        if *protected {
            let refused = delete(&emulator, region, &identifier).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (PROTECTED_STATUS, PROTECTED_CODE),
                "case {case:?}: a protected dashboard cannot be deleted, got {refused:?}"
            );
            // The refusal released nothing: the dashboard is still stored, still
            // protected.
            assert_eq!(
                stored_protection(&emulator, name, region),
                Some(true),
                "case {case:?}: the refused delete must leave the dashboard stored and \
                 protected, got {:?}",
                stored_protection(&emulator, name, region)
            );
            read(&emulator, region, &identifier)
                .await
                .expect_ok(&format!("{case}: after the refused delete"));

            // Clearing the flag is what makes the dashboard deletable.
            update(
                &emulator,
                region,
                &identifier,
                &json!({"TerminationProtectionEnabled": false}),
            )
            .await
            .expect_ok(case);
        }

        delete(&emulator, region, &identifier)
            .await
            .expect_ok(&format!("{case}: the delete of an unprotected dashboard"));
        assert_eq!(
            stored_protection(&emulator, name, region),
            None,
            "case {case:?}: the delete must release the dashboard, got {:?}",
            stored_protection(&emulator, name, region)
        );
        read(&emulator, region, &identifier).await.expect_error(
            &format!("{case}: after the delete"),
            400,
            "ResourceNotFoundException",
        );
    }
}

/// RULE: the dashboard `CreateDashboard` has returned is ready, so a resource-based
/// policy attaches to it with NO wait, is reported back and read back byte for byte,
/// and is detached again.
///
/// The rows differ along the Region the dashboard lives in and the SHAPE of the
/// document — one statement or several, a map principal or the `*` wildcard, with and
/// without the optional elements, and with whitespace a byte-equal round trip must
/// preserve.
#[tokio::test]
async fn a_policy_attaches_to_a_dashboard_the_create_just_returned() {
    let emulator = Emulator::new();

    // (case, Region, dashboard name, the document exactly as it goes on the wire)
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "one statement, a map principal",
            HOME_REGION,
            "policy-one",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowOneAccount",
                    "Effect": "Allow",
                    "Principal": {"AWS": "arn:aws:iam::123456789012:root"},
                    "Action": "cloudtrail:StartDashboardRefresh",
                }],
            })
            .to_string(),
        ),
        (
            "two statements, one of them a Deny",
            HOME_REGION,
            "policy_two",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": {"Service": "cloudtrail.amazonaws.com"},
                        "Action": ["cloudtrail:StartQuery", "cloudtrail:GetQueryResults"],
                    },
                    {
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "cloudtrail:DeleteResourcePolicy",
                    },
                ],
            })
            .to_string(),
        ),
        (
            "a single statement object rather than a list, in another Region",
            OTHER_REGION,
            "policy-elsewhere",
            json!({
                "Statement": {
                    "Effect": "Allow",
                    "Principal": {"AWS": ["123456789012", "arn:aws:iam::123456789012:root"]},
                    "NotAction": "cloudtrail:DeleteDashboard",
                },
            })
            .to_string(),
        ),
        (
            "a document carrying newlines and indentation, which a byte-equal round \
             trip preserves",
            HOME_REGION,
            "policy-formatted",
            "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n      \
             \"Effect\": \"Allow\",\n      \"Principal\": \"*\",\n      \"Action\": \
             \"cloudtrail:GetDashboard\"\n    }\n  ]\n}"
                .to_string(),
        ),
    ];

    for (case, region, name, document) in &rows {
        let created = create(&emulator, region, &json!({"Name": name})).await;
        let resource_arn = created
            .expect_ok(case)
            .get("DashboardArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: CreateDashboard reported no ARN"))
            .to_string();

        // No wait between the create and the attach: the dashboard the create returned
        // is ready to carry a policy.
        let attached = put(&emulator, &resource_arn, document).await;
        let attached_body = attached.expect_ok(case);
        assert_eq!(
            (
                attached_body.get("ResourceArn").and_then(Value::as_str),
                attached_body.get("ResourcePolicy").and_then(Value::as_str),
            ),
            (Some(resource_arn.as_str()), Some(document.as_str())),
            "case {case:?}: the attach must report the document it stored against \
             {resource_arn}, got {attached_body:?}"
        );

        // Read back byte for byte.
        let fetched = get(&emulator, &resource_arn).await;
        let fetched_body = fetched.expect_ok(case);
        assert_eq!(
            fetched_body.get("ResourcePolicy").and_then(Value::as_str),
            Some(document.as_str()),
            "case {case:?}: the policy must read back byte for byte, got {fetched_body:?}"
        );

        // And detached again, leaving the dashboard carrying none.
        detach(&emulator, &resource_arn)
            .await
            .expect_ok(&format!("{case}: the detach"));
        let after = get(&emulator, &resource_arn).await;
        after.expect_error(
            &format!("{case}: after the detach"),
            404,
            "ResourcePolicyNotFoundException",
        );
    }
}

/// RULE: a document that is not a resource-based policy is refused with
/// `ResourcePolicyNotValidException` (HTTP 400) and one fixed sentence — "the
/// resouce-based policy has syntax errors, or contains a principal that is not valid"
/// — and stores nothing, on the dashboard the create has just returned as on any
/// other resource.
///
/// The rows differ along WHICH part of the grammar the document breaks: the JSON
/// syntax, the Statement element, the Effect, the Action and the Principal.
#[tokio::test]
async fn a_document_that_is_not_a_policy_is_refused_on_a_fresh_dashboard() {
    let emulator = Emulator::new();
    let created = create(&emulator, HOME_REGION, &json!({"Name": "bad-documents"})).await;
    let resource_arn = created
        .expect_ok("create the dashboard")
        .get("DashboardArn")
        .and_then(Value::as_str)
        .expect("CreateDashboard reported an ARN")
        .to_string();

    // (case, the document exactly as it goes on the wire)
    let rows: Vec<(&str, String)> = vec![
        ("not JSON at all", "this is not a policy".to_string()),
        (
            "JSON, but not an object",
            json!(["Allow", "everything"]).to_string(),
        ),
        (
            "no Statement element",
            json!({"Version": "2012-10-17"}).to_string(),
        ),
        (
            "an empty Statement list",
            json!({"Statement": []}).to_string(),
        ),
        (
            "a statement with no Effect",
            json!({
                "Statement": [{
                    "Principal": "*",
                    "Action": "cloudtrail:GetDashboard",
                }],
            })
            .to_string(),
        ),
        (
            "a statement whose Effect is neither Allow nor Deny",
            json!({
                "Statement": [{
                    "Effect": "Maybe",
                    "Principal": "*",
                    "Action": "cloudtrail:GetDashboard",
                }],
            })
            .to_string(),
        ),
        (
            "a statement with no Action",
            json!({
                "Statement": [{"Effect": "Allow", "Principal": "*"}],
            })
            .to_string(),
        ),
        (
            "a statement with no Principal",
            json!({
                "Statement": [{"Effect": "Allow", "Action": "cloudtrail:GetDashboard"}],
            })
            .to_string(),
        ),
        (
            "a statement whose Principal names nobody",
            json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": ""},
                    "Action": "cloudtrail:GetDashboard",
                }],
            })
            .to_string(),
        ),
    ];

    for (case, document) in &rows {
        let refused = put(&emulator, &resource_arn, document).await;
        refused.expect_error(case, 400, "ResourcePolicyNotValidException");
        assert_eq!(
            refused.message(),
            NOT_A_POLICY_MESSAGE,
            "case {case:?}: expected {NOT_A_POLICY_MESSAGE:?}, got {:?}",
            refused.message()
        );
        // The refusal stored nothing: the dashboard still carries no policy.
        get(&emulator, &resource_arn).await.expect_error(
            &format!("{case}: the refused put stored nothing"),
            404,
            "ResourcePolicyNotFoundException",
        );
    }

    // The control: the same dashboard takes a document that IS a policy, so every row
    // above was refused for its document and not for the dashboard.
    let valid = json!({
        "Statement": [{
            "Effect": "Allow",
            "Principal": "*",
            "Action": "cloudtrail:GetDashboard",
        }],
    })
    .to_string();
    put(&emulator, &resource_arn, &valid)
        .await
        .expect_ok("the control: a document that is a policy");
}

/// RULE: a resource that exists and carries no resource-based policy is refused with
/// `ResourcePolicyNotFoundException` and ONE fixed sentence — CloudTrail states nothing
/// from the request in it — from `GetResourcePolicy` and `DeleteResourcePolicy` alike.
///
/// The rows span the resource KINDS a policy attaches to (a dashboard created over the
/// wire, a channel and an event data store seeded in the registries the handlers read)
/// and the Region each lives in; the two operations are walked over every row.
#[tokio::test]
async fn a_resource_carrying_no_policy_is_refused_with_one_fixed_sentence() {
    let emulator = Emulator::new();

    let fresh = create(&emulator, HOME_REGION, &json!({"Name": "no-policy-dash"})).await;
    let fresh_arn = fresh
        .expect_ok("create the dashboard")
        .get("DashboardArn")
        .and_then(Value::as_str)
        .expect("CreateDashboard reported an ARN")
        .to_string();
    let elsewhere = create(&emulator, OTHER_REGION, &json!({"Name": "no-policy-elsewhere"})).await;
    let elsewhere_arn = elsewhere
        .expect_ok("create the dashboard in another Region")
        .get("DashboardArn")
        .and_then(Value::as_str)
        .expect("CreateDashboard reported an ARN")
        .to_string();

    let rows: Vec<(&str, String)> = vec![
        ("a dashboard the create just returned", fresh_arn),
        ("a dashboard in another Region", elsewhere_arn),
        (
            "a channel",
            channel(&emulator, "no-policy-channel", HOME_REGION),
        ),
        (
            "an event data store",
            event_data_store(&emulator, "no-policy-store", OTHER_REGION),
        ),
    ];

    for (case, resource_arn) in &rows {
        for operation in ["GetResourcePolicy", "DeleteResourcePolicy"] {
            let case = format!("{case} through {operation}");
            let refused = emulator
                .call(operation, &json!({"ResourceArn": resource_arn}))
                .await;
            refused.expect_error(&case, 404, "ResourcePolicyNotFoundException");
            assert_eq!(
                refused.message(),
                NO_POLICY_MESSAGE,
                "case {case:?}: expected {NO_POLICY_MESSAGE:?}, got {:?}",
                refused.message()
            );
        }
    }
}
