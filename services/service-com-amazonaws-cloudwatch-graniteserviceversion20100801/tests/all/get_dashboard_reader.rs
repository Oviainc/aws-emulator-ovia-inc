// SPDX-License-Identifier: CC0-1.0

//! `GetDashboard` as a READER of the dashboard store.
//!
//! "Displays the details of the dashboard that you specify." (`GetDashboard`) — so the call
//! reports the document a write stored, under the global ARN of the name it was stored with,
//! and nothing about the store changes because somebody read it.
//!
//! Two rules are pinned here, both one level more general than any single request:
//!
//! * Every member that NAMES a dashboard is checked against the documented
//!   length-and-character rule for a dashboard name BEFORE the name is looked up, and a value
//!   outside that rule is a bad parameter value (HTTP 400 `InvalidParameterValueException`,
//!   `awsQueryError` code `InvalidParameterValue`) naming the member it arrived in — not a
//!   dashboard that happens not to exist. A name inside the rule that no dashboard is held
//!   under is the not-found refusal (HTTP 404 `DashboardNotFoundError`, `awsQueryError` code
//!   `ResourceNotFound`).
//! * A read reports state and does not change it: every answer a later caller gets is the one
//!   it would have got had the read never happened.
//!
//! The table rows differ along what those rules generalise over — the name's length at and
//! past both published bounds, which characters it holds, whether a dashboard is held under
//! it, and which operation the name arrived in — so the dashboard a recorded request happened
//! to read is one row among many rather than the specification.

use crate::harness::{build_router, call, error_code, error_message, ok};
use serde_json::{json, Value};

/// The published bound on a dashboard name: "The maximum length is 255, and valid characters
/// are A-Z, a-z, 0-9, \"-\", and \"_\"." (`smithy_query(query="doc",shape="PutDashboardInput",
/// member="DashboardName")`, the same prose `GetDashboardInput$DashboardName` and
/// `DeleteDashboardsInput$DashboardNames` carry).
const MAX_NAME_LEN: usize = 255;

/// The account every dashboard ARN below is minted under: the emulator's canonical one.
const ACCOUNT: &str = "000000000000";

/// The ARN a dashboard of this name carries. The Region segment is EMPTY because "All
/// dashboards in your account are global, not region-specific." (`PutDashboard`).
fn expected_arn(name: &str) -> String {
    format!("arn:aws:cloudwatch::{ACCOUNT}:dashboard/{name}")
}

/// A string member of a response body.
fn text(value: &Value, member: &str) -> Option<String> {
    value
        .get(member)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// What every dashboard-naming member must answer for a row's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameVerdict {
    /// The name satisfies the documented rule, so the call gets as far as the store.
    Allowed,
    /// Longer than the documented maximum.
    TooLong,
    /// No characters at all, so it names no dashboard.
    TooShort,
    /// Holds a character the documented set does not list.
    BadCharacters,
}

impl NameVerdict {
    /// The sentence CloudWatch answers for a value this verdict refuses, naming the member
    /// `field` the value arrived in, or `None` when the value is a dashboard name.
    fn message(self, field: &str) -> Option<String> {
        match self {
            Self::Allowed => None,
            Self::TooLong => Some(format!(
                "The value for field {field} is too long. It must have length less than or \
                 equal to {MAX_NAME_LEN}.\n"
            )),
            Self::TooShort => Some(format!(
                "The value for field {field} is too short. It must have length greater than or \
                 equal to 1.\n"
            )),
            Self::BadCharacters => Some(format!(
                "The value for field {field} contains invalid characters. It can only contain \
                 alphanumerics, dash (-) and underscore (_).\n"
            )),
        }
    }
}

/// The name table both rules below are walked over: rows span each published bound and one
/// past it, the characters inside and outside the documented set, and a name whose length is
/// counted in characters rather than bytes.
fn name_cases() -> Vec<(&'static str, String, NameVerdict)> {
    vec![
        (
            "one character, the shortest name",
            "d".to_string(),
            NameVerdict::Allowed,
        ),
        (
            "every documented character class",
            "aZ9-_".to_string(),
            NameVerdict::Allowed,
        ),
        (
            "255 characters, the documented maximum",
            "n".repeat(MAX_NAME_LEN),
            NameVerdict::Allowed,
        ),
        (
            "256 characters, one past the maximum",
            "n".repeat(MAX_NAME_LEN + 1),
            NameVerdict::TooLong,
        ),
        (
            "no characters at all",
            String::new(),
            NameVerdict::TooShort,
        ),
        (
            "a name holding a space",
            "my dashboard".to_string(),
            NameVerdict::BadCharacters,
        ),
        (
            "a name holding a period, which only the list prefix allows",
            "my.dashboard".to_string(),
            NameVerdict::BadCharacters,
        ),
        (
            "a name holding a plus",
            "a+b".to_string(),
            NameVerdict::BadCharacters,
        ),
        (
            "a name holding a tab",
            "a\tb".to_string(),
            NameVerdict::BadCharacters,
        ),
        (
            "255 non-ASCII characters, inside the length bound",
            "ü".repeat(MAX_NAME_LEN),
            NameVerdict::BadCharacters,
        ),
        (
            "256 non-ASCII characters, so the length is refused first",
            "ü".repeat(MAX_NAME_LEN + 1),
            NameVerdict::TooLong,
        ),
    ]
}

/// One member that carries a dashboard name: the operation it belongs to, the member the
/// refusal names, and how a request carrying `name` in it is built.
type NamingMember = (&'static str, &'static str, fn(&str) -> Value);

/// A dashboard document the widget schema accepts, for the rows that need one stored.
fn stored_body(marker: &str) -> String {
    json!({"widgets": [{
        "type": "text",
        "x": 0,
        "y": 0,
        "width": 6,
        "height": 3,
        "properties": {"markdown": format!("# {marker}")},
    }]})
    .to_string()
}

#[tokio::test]
async fn get_dashboard_checks_the_name_rule_before_it_looks_a_dashboard_up() {
    for (case, name, verdict) in name_cases() {
        let router = build_router();
        // One dashboard the row never names, so "the read left the store alone" is asserted
        // against something rather than against emptiness.
        let bystander = "bystander";
        ok(
            &router,
            "PutDashboard",
            &json!({"DashboardName": bystander, "DashboardBody": stored_body(bystander)}),
        )
        .await;

        let (status, body) = call(&router, "GetDashboard", &json!({"DashboardName": name})).await;
        match verdict.message("DashboardName") {
            Some(expected) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "InvalidParameterValueException"),
                    "case {case:?}: a name outside the documented rule is a bad parameter \
                     value, got {status} {body}"
                );
                assert_eq!(
                    error_message(&body),
                    expected,
                    "case {case:?}: expected CloudWatch's bad-field sentence, got {body}"
                );
            }
            None => {
                // A name the rule allows reaches the store, which holds nothing under it yet.
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (404, "DashboardNotFoundError"),
                    "case {case:?}: an allowed name nothing is stored under is a not-found, \
                     got {status} {body}"
                );
                assert_eq!(
                    error_message(&body),
                    format!("Dashboard {name} does not exist"),
                    "case {case:?}: expected the refusal to name the dashboard asked for, \
                     got {body}"
                );

                // And once a dashboard IS held under it, the same read reports it.
                let document = stored_body(&format!("row {case}"));
                ok(
                    &router,
                    "PutDashboard",
                    &json!({"DashboardName": name, "DashboardBody": document}),
                )
                .await;
                let got = ok(&router, "GetDashboard", &json!({"DashboardName": name})).await;
                assert_eq!(
                    text(&got, "DashboardBody").as_deref(),
                    Some(document.as_str()),
                    "case {case:?}: expected the stored document verbatim, got {got}"
                );
                assert_eq!(
                    text(&got, "DashboardName").as_deref(),
                    Some(name.as_str()),
                    "case {case:?}: expected the name asked for, got {got}"
                );
                assert_eq!(
                    text(&got, "DashboardArn").as_deref(),
                    Some(expected_arn(&name).as_str()),
                    "case {case:?}: expected the dashboard's global ARN, got {got}"
                );
            }
        }

        // Whatever the row asked for, the dashboard the row did not name is untouched.
        let bystander_read = ok(
            &router,
            "GetDashboard",
            &json!({"DashboardName": bystander}),
        )
        .await;
        assert_eq!(
            text(&bystander_read, "DashboardBody").as_deref(),
            Some(stored_body(bystander).as_str()),
            "case {case:?}: a read of another name must leave this dashboard alone, \
             got {bystander_read}"
        );
    }
}

#[tokio::test]
async fn every_member_naming_a_dashboard_refuses_the_same_names() {
    // The rule belongs to the dashboard NAME, not to one operation, so each row is walked
    // through every member that carries one — the reader, the writer, and the member that
    // carries a LIST of names, whose refusal names that member instead.
    let members: Vec<NamingMember> = vec![
        ("GetDashboard", "DashboardName", |name| {
            json!({"DashboardName": name})
        }),
        ("PutDashboard", "DashboardName", |name| {
            json!({"DashboardName": name, "DashboardBody": "{}"})
        }),
        ("DeleteDashboards", "DashboardNames", |name| {
            json!({"DashboardNames": [name]})
        }),
    ];

    for (case, name, verdict) in name_cases() {
        for (operation, field, request) in &members {
            let router = build_router();
            let (status, body) = call(&router, operation, &request(&name)).await;
            match verdict.message(field) {
                Some(expected) => {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (400, "InvalidParameterValueException"),
                        "case {case:?} through {operation}: expected a 400 \
                         InvalidParameterValueException, got {status} {body}"
                    );
                    assert_eq!(
                        error_message(&body),
                        expected,
                        "case {case:?} through {operation}: the refusal must name the member \
                         the value arrived in, got {body}"
                    );
                }
                None => assert_ne!(
                    error_code(&body).as_str(),
                    "InvalidParameterValueException",
                    "case {case:?} through {operation}: a name the documented rule allows must \
                     not be refused as a bad parameter value, got {status} {body}"
                ),
            }
        }
    }
}

#[tokio::test]
async fn reading_a_dashboard_changes_nothing_a_later_caller_sees() {
    let router = build_router();
    // Rows differ in the document stored, so the snapshot compared below covers an empty
    // document, one carrying a widget, and one carrying several.
    let held: Vec<(&str, String)> = vec![
        ("empty-doc", "{}".to_string()),
        ("one-widget", stored_body("one")),
        (
            "two-widgets",
            json!({"widgets": [
                {"type": "text", "x": 0, "y": 0, "width": 6, "height": 3,
                 "properties": {"markdown": "# first"}},
                {"type": "text", "x": 6, "y": 0, "width": 6, "height": 3,
                 "properties": {"markdown": "# second"}},
            ]})
            .to_string(),
        ),
    ];
    for (name, body) in &held {
        ok(
            &router,
            "PutDashboard",
            &json!({"DashboardName": name, "DashboardBody": body}),
        )
        .await;
    }

    // The whole reported store, including each entry's `Size` and `LastModified`, before any
    // read: "The size of the dashboard, in bytes." (`DashboardEntry$Size`) and the time it was
    // last written are both derived from stored state, so a read that changed state would move
    // one of them.
    let before = ok(&router, "ListDashboards", &json!({})).await;

    // Every way of reading: each held dashboard, twice over; a name the rule allows that
    // nothing is held under; and a name outside the rule entirely.
    for _ in 0..2 {
        for (name, body) in &held {
            let got = ok(&router, "GetDashboard", &json!({"DashboardName": name})).await;
            assert_eq!(
                text(&got, "DashboardBody").as_deref(),
                Some(body.as_str()),
                "case {name:?}: every read reports the same document, got {got}"
            );
        }
        for name in ["absent", "EMPTY-DOC", "empty-do", "my dashboard"] {
            let (status, _) = call(&router, "GetDashboard", &json!({"DashboardName": name})).await;
            assert_ne!(
                status, 200,
                "case {name:?}: no dashboard is held under this name, got {status}"
            );
        }
    }

    let after = ok(&router, "ListDashboards", &json!({})).await;
    assert_eq!(
        after, before,
        "case \"after every read\": a read must leave the reported store exactly as it was, \
         got {after}"
    );
}
