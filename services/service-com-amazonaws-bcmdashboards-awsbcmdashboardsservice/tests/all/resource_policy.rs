// SPDX-License-Identifier: CC0-1.0

//! `GetResourcePolicy` over the wire: the resource-based policy document persisted on
//! the dashboard the request names.
//!
//! The rule under test is that `GetResourcePolicy` reports the policy document stored on
//! the dashboard its `resourceArn` names — the empty document while no policy is
//! attached, since `policyDocument`'s modelled length starts at 0 — echoes that ARN,
//! answers `ResourceNotFoundException` (404) when this service holds no dashboard under
//! the ARN, and (being `@readonly`) changes nothing a later caller can observe.
//!
//! This service's model declares no operation that ATTACHES a policy (its 15 operations
//! carry no `PutResourcePolicy`), so the only way to show the answer is projected from
//! persisted state rather than hardcoded is to write the document into the shared
//! engine's store — exactly where a future attach operation would put it — and read it
//! back over the wire. The documents below are a TABLE spanning the class
//! `policyDocument` admits: no policy at all, the empty document, a one-statement
//! policy, a multi-statement policy, one at the 1024-character maximum, and one carrying
//! whitespace and non-ASCII characters.
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::ComAmazonawsBcmdashboardsAwsbcmdashboardsserviceState as DashboardsState;
use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::EmulatedService;

use super::dashboard_lifecycle::{call, error_code, graph_config, ok_call};

/// A router serving this one service, plus the engine behind it, so a test can write a
/// policy document into the very store the handlers read.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Persist `document` as the resource-based policy of the dashboard `arn` names (or
/// `None` to leave it with no policy attached), writing straight into the shared store.
fn attach_policy(engine: &engine::InMemoryEngine, arn: &str, document: Option<&str>) {
    let state = engine.state::<DashboardsState>();
    let mut state = state.lock();
    let dashboard = state
        .dashboards
        .get_mut(arn)
        .unwrap_or_else(|| panic!("the store must hold the dashboard {arn} just created"));
    dashboard.resource_policy = document.map(str::to_string);
}

/// Create a dashboard and return the ARN it was stored under.
async fn create_dashboard(router: &MultiServiceRouter, request: &Value) -> String {
    let created = ok_call(router, "CreateDashboard", request).await;
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDashboard must answer an arn: {created}"))
        .to_string()
}

/// The `policyDocument` of a `GetResourcePolicy` answer.
fn reported_document(body: &Value) -> String {
    body.get("policyDocument")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("GetResourcePolicy must answer a policyDocument: {body}"))
        .to_string()
}

/// The `resourceArn` of a `GetResourcePolicy` answer.
fn reported_arn(body: &Value) -> String {
    body.get("resourceArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("GetResourcePolicy must answer a resourceArn: {body}"))
        .to_string()
}

/// How many dashboards this router holds, used to show a read stored nothing.
async fn dashboard_count(router: &MultiServiceRouter) -> usize {
    let page = ok_call(router, "ListDashboards", &json!({"maxResults": 100})).await;
    page["dashboards"]
        .as_array()
        .map_or(0, |dashboards| dashboards.len())
}

/// One row of the policy table: what is persisted, and what a read must report.
struct PolicyCase {
    label: &'static str,
    /// `None` means no policy is attached to the dashboard at all.
    stored: Option<String>,
    expected: String,
}

/// A one-statement dashboard-sharing policy, the shape a resource-based policy takes.
fn sharing_policy(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "ShareDashboard",
            "Effect": "Allow",
            "Principal": {"AWS": principal},
            "Action": ["bcm-dashboards:GetDashboard"],
            "Resource": "*"
        }]
    })
    .to_string()
}

/// The documents this suite spans. The bounds come from the model: `policyDocument`
/// targets `GenericString` with length 0..1024 and pattern `^[\S\s]*$` — any characters
/// at all — source: the `policyDocument` member reported by
/// `smithy_query(query="operation", operation="GetResourcePolicy")`.
fn policy_cases() -> Vec<PolicyCase> {
    let max_length = {
        // A real policy padded with `Sid` characters to exactly the 1024-character
        // maximum the member admits.
        let head = r#"{"Version":"2012-10-17","Statement":[{"Sid":""#;
        let tail = r#"","Effect":"Allow","Principal":"*","Action":"*","Resource":"*"}]}"#;
        let padding = 1024 - head.len() - tail.len();
        format!("{head}{}{tail}", "a".repeat(padding))
    };
    assert_eq!(
        max_length.len(),
        1024,
        "the maximum-length row must be exactly at the modelled 1024-character bound"
    );

    vec![
        PolicyCase {
            label: "no policy attached at all",
            stored: None,
            expected: String::new(),
        },
        PolicyCase {
            label: "an attached but empty document (the 0-character minimum)",
            stored: Some(String::new()),
            expected: String::new(),
        },
        PolicyCase {
            label: "an empty JSON object",
            stored: Some("{}".to_string()),
            expected: "{}".to_string(),
        },
        PolicyCase {
            label: "a one-statement sharing policy",
            stored: Some(sharing_policy("arn:aws:iam::000000000000:root")),
            expected: sharing_policy("arn:aws:iam::000000000000:root"),
        },
        PolicyCase {
            label: "a two-statement policy with an organization condition",
            stored: Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {"Effect": "Allow", "Principal": "*", "Action": "bcm-dashboards:GetDashboard",
                         "Condition": {"StringEquals": {"aws:PrincipalOrgID": "o-abcd1234"}}},
                        {"Effect": "Deny", "Principal": "*", "Action": "bcm-dashboards:DeleteDashboard"}
                    ]
                })
                .to_string(),
            ),
            expected: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Principal": "*", "Action": "bcm-dashboards:GetDashboard",
                     "Condition": {"StringEquals": {"aws:PrincipalOrgID": "o-abcd1234"}}},
                    {"Effect": "Deny", "Principal": "*", "Action": "bcm-dashboards:DeleteDashboard"}
                ]
            })
            .to_string(),
        },
        PolicyCase {
            label: "a document carrying newlines, tabs and non-ASCII characters",
            stored: Some("{\n\t\"Sid\": \"Zahlungsabläufe / 50% \\\"prod\\\"\"\n}".to_string()),
            expected: "{\n\t\"Sid\": \"Zahlungsabläufe / 50% \\\"prod\\\"\"\n}".to_string(),
        },
        PolicyCase {
            label: "a document at the 1024-character maximum",
            stored: Some(max_length.clone()),
            expected: max_length,
        },
    ]
}

/// For every document the member admits, `GetResourcePolicy` reports exactly what is
/// persisted on the dashboard — and reports it again unchanged on a second read.
#[tokio::test]
async fn reported_document_is_the_document_persisted_on_the_dashboard() {
    for case in policy_cases() {
        let (router, engine) = router_with_engine();
        let arn = create_dashboard(
            &router,
            &json!({
                "name": "Shared Dashboard",
                "widgets": [{"title": "Spend", "configs": [graph_config("BlendedCost", "STACK", "MONTHLY")]}]
            }),
        )
        .await;
        attach_policy(&engine, &arn, case.stored.as_deref());

        let answer = ok_call(&router, "GetResourcePolicy", &json!({"resourceArn": arn})).await;
        let reported = reported_document(&answer);
        assert_eq!(
            reported, case.expected,
            "case {}: expected the persisted document {:?}, got {:?}",
            case.label, case.expected, reported
        );
        let echoed = reported_arn(&answer);
        assert_eq!(
            echoed, arn,
            "case {}: expected the dashboard arn {arn}, got {echoed}",
            case.label
        );

        // A read must not change what a later caller sees: the same call answers the
        // same document, and the store still holds exactly the one dashboard.
        let again = ok_call(&router, "GetResourcePolicy", &json!({"resourceArn": arn})).await;
        let reported_again = reported_document(&again);
        assert_eq!(
            reported_again, reported,
            "case {}: a second read must report the same document {reported:?}, got {reported_again:?}",
            case.label
        );
        let count = dashboard_count(&router).await;
        assert_eq!(
            count, 1,
            "case {}: reading a policy must store nothing, got {count} dashboards",
            case.label
        );
    }
}

/// A dashboard created through the API carries no policy, whatever shape the create
/// request took, so its policy reads back as the empty document — and the dashboard
/// itself is still readable afterwards.
#[tokio::test]
async fn a_dashboard_created_through_the_api_reports_the_empty_document() {
    // Rows differ in the members the create request supplies and in widget count, which
    // is what a dashboard create varies over; none of them attaches a policy.
    let creates: Vec<(&str, Value)> = vec![
        (
            "name only, no widgets",
            json!({"name": "Bare Dashboard", "widgets": []}),
        ),
        (
            "name and description",
            json!({"name": "Described Dashboard", "description": "Monthly spend", "widgets": []}),
        ),
        (
            "one widget",
            json!({
                "name": "One Widget",
                "widgets": [{"title": "Spend", "configs": [graph_config("UnblendedCost", "STACK", "DAILY")]}]
            }),
        ),
        (
            "three widgets and tags",
            json!({
                "name": "Three Widgets",
                "widgets": [
                    {"title": "One", "configs": [graph_config("Cost", "STACK", "MONTHLY")]},
                    {"title": "Two", "width": 2, "configs": [graph_config("UsageQuantity", "LINE", "DAILY")]},
                    {"title": "Three", "height": 10, "configs": [graph_config("Hour", "STACK", "HOURLY")]}
                ],
                "resourceTags": [{"key": "team", "value": "finops"}]
            }),
        ),
    ];

    for (label, request) in &creates {
        let (router, _engine) = router_with_engine();
        let arn = create_dashboard(&router, request).await;

        let answer = ok_call(&router, "GetResourcePolicy", &json!({"resourceArn": arn})).await;
        let reported = reported_document(&answer);
        assert!(
            reported.is_empty(),
            "case {label}: a dashboard with no policy attached must report the empty document, got {reported:?}"
        );

        // Round trip: the dashboard the policy was read for is untouched.
        let read_back = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        let read_arn = read_back["arn"].as_str().unwrap_or("<no arn>");
        assert_eq!(
            read_arn, arn,
            "case {label}: GetDashboard must still answer {arn}, got {read_arn}"
        );
    }
}

/// Policies are per dashboard: each ARN answers its own persisted document, never
/// another dashboard's and never the most recently attached one.
#[tokio::test]
async fn documents_are_reported_per_dashboard() {
    let (router, engine) = router_with_engine();

    let mut stored: Vec<(&str, String, String)> = Vec::new();
    for case in policy_cases() {
        let arn = create_dashboard(
            &router,
            &json!({"name": "Shared Router Dashboard", "widgets": []}),
        )
        .await;
        attach_policy(&engine, &arn, case.stored.as_deref());
        stored.push((case.label, arn, case.expected));
    }

    for (label, arn, expected) in &stored {
        let answer = ok_call(&router, "GetResourcePolicy", &json!({"resourceArn": arn})).await;
        let reported = reported_document(&answer);
        assert_eq!(
            &reported, expected,
            "case {label}: expected that dashboard's own document {expected:?}, got {reported:?}"
        );
        let echoed = reported_arn(&answer);
        assert_eq!(
            &echoed, arn,
            "case {label}: expected the dashboard arn {arn}, got {echoed}"
        );
    }
}

/// A well-formed `DashboardArn` this service holds no dashboard under is a not-found —
/// a missing dashboard, unlike a missing policy, is an error — and the refused read
/// stores nothing.
#[tokio::test]
async fn an_arn_naming_no_dashboard_is_not_found() {
    let (router, engine) = router_with_engine();

    // A dashboard that was created, given a policy, and then deleted: the policy went
    // with the record.
    let deleted = create_dashboard(&router, &json!({"name": "Doomed Dashboard", "widgets": []})).await;
    attach_policy(&engine, &deleted, Some(&sharing_policy("*")));
    ok_call(&router, "DeleteDashboard", &json!({"arn": deleted})).await;

    let before = dashboard_count(&router).await;

    // Each row satisfies `DashboardArn`'s pattern
    // `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$` yet names
    // no stored dashboard — including the `*` resource id that pattern admits.
    let cases: Vec<(&str, String)> = vec![
        (
            "a dashboard id never created",
            "arn:aws:bcm-dashboards::000000000000:dashboard/does-not-exist".to_string(),
        ),
        (
            "the wildcard dashboard id",
            "arn:aws:bcm-dashboards::000000000000:dashboard/*".to_string(),
        ),
        (
            "a partition other than aws",
            "arn:aws-cn:bcm-dashboards::000000000000:dashboard/does-not-exist".to_string(),
        ),
        (
            "an account id this emulator did not mint under",
            "arn:aws:bcm-dashboards::999999999999:dashboard/00000000000000000000000000000001"
                .to_string(),
        ),
        ("a deleted dashboard", deleted.clone()),
    ];

    for (label, arn) in &cases {
        let (status, body) = call(&router, "GetResourcePolicy", &json!({"resourceArn": arn})).await;
        assert_eq!(
            status, 404,
            "case {label}: {arn} names no dashboard, expected 404, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {label}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );
    }

    let after = dashboard_count(&router).await;
    assert_eq!(
        after, before,
        "a refused read must store nothing: {before} dashboards before, {after} after"
    );
}

/// An ARN that is not a `DashboardArn` at all is refused as invalid input (400), not as
/// a not-found: the modelled constraints are checked before the handler runs.
#[tokio::test]
async fn an_arn_outside_the_modelled_pattern_is_refused() {
    let (router, _engine) = router_with_engine();

    // Each row breaks `DashboardArn`'s pattern or its 20..2048 length in a different way.
    let cases: Vec<(&str, String)> = vec![
        ("not an arn at all", "not-an-arn".to_string()),
        (
            "shorter than the 20-character minimum",
            "arn:aws:bcm-dash".to_string(),
        ),
        (
            "another service",
            "arn:aws:bcm-dataexports::000000000000:dashboard/abc".to_string(),
        ),
        (
            "a scheduled report, which is not a dashboard",
            "arn:aws:bcm-dashboards::000000000000:scheduled-report/abc".to_string(),
        ),
        (
            "an account id that is not 12 digits",
            "arn:aws:bcm-dashboards::1234:dashboard/abc".to_string(),
        ),
        (
            "an uppercase resource id",
            "arn:aws:bcm-dashboards::000000000000:dashboard/ABC".to_string(),
        ),
        (
            "a region segment, which this service's arns do not carry",
            "arn:aws:bcm-dashboards:us-east-1:000000000000:dashboard/abc".to_string(),
        ),
        (
            "longer than the 2048-character maximum",
            format!(
                "arn:aws:bcm-dashboards::000000000000:dashboard/{}",
                "a".repeat(2048)
            ),
        ),
    ];

    for (label, arn) in &cases {
        let (status, body) = call(&router, "GetResourcePolicy", &json!({"resourceArn": arn})).await;
        assert_eq!(
            status, 400,
            "case {label}: {arn} is not a DashboardArn, expected 400, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {label}: expected ValidationException, got {}",
            error_code(&body)
        );
    }
}
