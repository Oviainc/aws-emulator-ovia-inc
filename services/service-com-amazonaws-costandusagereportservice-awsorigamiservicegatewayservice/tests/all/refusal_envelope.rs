// SPDX-License-Identifier: CC0-1.0

//! What a refusal of this service carries on the wire.
//!
//! The rule under test: this service's modelled exceptions declare a single OPTIONAL
//! `Message` (target `ErrorMessage`, documented as "A message to show the detail of the
//! exception"), and an error shape's own documentation is the model's description of the
//! error CLASS, not text the service puts on the wire — so a refusal that reports a
//! resource-state condition is serialized with its error code and HTTP status alone, no
//! `Message` member at all, and nothing from the request echoed back into it.
//!
//! The table below spans what that rule generalises over: the operation that refuses
//! (`PutReportDefinition` and all three tag-plane operations), the error shape
//! (`DuplicateReportNameException`, `ReportLimitReachedException`,
//! `ResourceNotFoundException`), the condition (a name already held, the account-wide
//! report cap, a name never created, a name whose report was deleted), and the spelling
//! and length of the report name the request carries — the recorded instance is one row.
//!
//! The counterpart rule is asserted too: an input-constraint refusal, the one kind of
//! refusal that reports which member of the request is wrong, still carries its
//! diagnostic.

use serde_json::{json, Value};

use service_com_amazonaws_costandusagereportservice_awsorigamiservicegatewayservice::state::{
    MAX_REPORTS, MAX_TAGS_PER_REPORT,
};

use super::report_definition_lifecycle::{error_code, minimal_definition, Cur};

/// The longest name the model allows: `ReportName` is length 0..256
/// (`smithy_query(shape="ReportName")`).
const REPORT_NAME_MAX: usize = 256;

/// The `Message` member of an error body, under either spelling.
///
/// The model names the member `Message`; the JSON protocols render an error message
/// member as lowercase `message`. Either one present means the service sent a message,
/// so both are looked up.
fn error_message(body: &Value) -> Option<&Value> {
    ["Message", "message"]
        .into_iter()
        .find_map(|key| body.get(key))
}

/// One refusal: the calls that build the account state, the call that must be refused,
/// and the error code the model names for that condition.
struct Refusal {
    case: &'static str,
    setup: Vec<(&'static str, Value)>,
    refused: (&'static str, Value),
    code: &'static str,
}

/// A name using every punctuation character the `ReportName` pattern allows
/// (`^[0-9A-Za-z!\-_.*'()]+$`, per `smithy_query(shape="ReportName")`), so a row proves
/// the rule does not depend on the shape of the name.
fn punctuated_name() -> String {
    "r3port!-_.*'()".to_string()
}

fn put(definition: &Value) -> (&'static str, Value) {
    ("PutReportDefinition", json!({ "ReportDefinition": definition }))
}

fn refusal_table() -> Vec<Refusal> {
    let long_name = "n".repeat(REPORT_NAME_MAX);
    let punctuated = punctuated_name();

    let mut table = vec![
        Refusal {
            case: "duplicate name, the same definition put twice",
            setup: vec![put(&minimal_definition("plain-report"))],
            refused: put(&minimal_definition("plain-report")),
            code: "DuplicateReportNameException",
        },
        Refusal {
            case: "duplicate name, a different definition under the held name, report tagged",
            setup: vec![
                put(&minimal_definition("tagged-report")),
                (
                    "TagResource",
                    json!({
                        "ReportName": "tagged-report",
                        "Tags": [{ "Key": "env", "Value": "staging" }],
                    }),
                ),
            ],
            refused: {
                let mut definition = minimal_definition("tagged-report");
                definition["TimeUnit"] = json!("MONTHLY");
                put(&definition)
            },
            code: "DuplicateReportNameException",
        },
        Refusal {
            case: "duplicate name, a name of the maximum modelled length",
            setup: vec![put(&minimal_definition(&long_name))],
            refused: put(&minimal_definition(&long_name)),
            code: "DuplicateReportNameException",
        },
        Refusal {
            case: "duplicate name, a name using the pattern's punctuation",
            setup: vec![put(&minimal_definition(&punctuated))],
            refused: put(&minimal_definition(&punctuated)),
            code: "DuplicateReportNameException",
        },
        Refusal {
            case: "TagResource on a name the account never held",
            setup: vec![],
            refused: (
                "TagResource",
                json!({
                    "ReportName": "never-created",
                    "Tags": [{ "Key": "env", "Value": "staging" }],
                }),
            ),
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "TagResource on a deleted report",
            setup: vec![
                put(&minimal_definition("deleted-report")),
                (
                    "DeleteReportDefinition",
                    json!({ "ReportName": "deleted-report" }),
                ),
            ],
            refused: (
                "TagResource",
                json!({
                    "ReportName": "deleted-report",
                    "Tags": [
                        { "Key": "env", "Value": "staging" },
                        { "Key": "owner", "Value": "finance" },
                    ],
                }),
            ),
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "ListTagsForResource on a name the account never held",
            setup: vec![],
            refused: (
                "ListTagsForResource",
                json!({ "ReportName": "never-created" }),
            ),
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "ListTagsForResource on a deleted report, beside a surviving one",
            setup: vec![
                put(&minimal_definition("report-a")),
                put(&minimal_definition("report-b")),
                ("DeleteReportDefinition", json!({ "ReportName": "report-b" })),
            ],
            refused: ("ListTagsForResource", json!({ "ReportName": "report-b" })),
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "UntagResource on a deleted report",
            setup: vec![
                put(&minimal_definition(&punctuated)),
                (
                    "DeleteReportDefinition",
                    json!({ "ReportName": punctuated.as_str() }),
                ),
            ],
            refused: (
                "UntagResource",
                json!({ "ReportName": punctuated.as_str(), "TagKeys": ["env"] }),
            ),
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "UntagResource on a name the account never held, several keys",
            setup: vec![],
            refused: (
                "UntagResource",
                json!({
                    "ReportName": "never-created",
                    "TagKeys": ["env", "owner", "cost-centre"],
                }),
            ),
            code: "ResourceNotFoundException",
        },
    ];

    // The account-wide cap: filling it takes MAX_REPORTS successful puts, so this row
    // builds its own setup list rather than spelling five literals.
    let mut fill: Vec<(&'static str, Value)> = Vec::new();
    for index in 0..MAX_REPORTS {
        fill.push(put(&minimal_definition(&format!("capped-{index}"))));
    }
    table.push(Refusal {
        case: "the account already holds the modelled report limit",
        setup: fill,
        refused: put(&minimal_definition("one-too-many")),
        code: "ReportLimitReachedException",
    });

    table
}

#[tokio::test]
async fn a_resource_state_refusal_carries_its_code_and_no_message() {
    for refusal in refusal_table() {
        let Refusal {
            case,
            setup,
            refused,
            code,
        } = refusal;
        let cur = Cur::new();
        for (operation, body) in &setup {
            let (status, response) = cur.call(operation, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: setup call {operation} got {status} {response:?}"
            );
        }

        let (operation, body) = &refused;
        let (status, response) = cur.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {operation}, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            code,
            "case {case:?}: expected error code {code}, got {:?}",
            error_code(&response)
        );
        assert!(
            error_message(&response).is_none(),
            "case {case:?}: expected no Message member, got {:?}",
            error_message(&response)
        );
    }
}

#[tokio::test]
async fn no_refusal_echoes_the_requested_report_name() {
    // The names below are what a refusal could leak: each is the only place it appears,
    // so finding it anywhere in the error body proves the request was echoed.
    let names = ["leak-probe-duplicate", "leak-probe-missing"];

    let cur = Cur::new();
    let (status, body) = cur.put(&minimal_definition(names[0]), None).await;
    assert_eq!(status, 200, "case seed: got {body:?}");

    let probes: Vec<(&str, &str, Value)> = vec![
        (
            "duplicate put",
            "PutReportDefinition",
            json!({ "ReportDefinition": minimal_definition(names[0]) }),
        ),
        (
            "tag a missing report",
            "TagResource",
            json!({
                "ReportName": names[1],
                "Tags": [{ "Key": "env", "Value": "staging" }],
            }),
        ),
        (
            "list the tags of a missing report",
            "ListTagsForResource",
            json!({ "ReportName": names[1] }),
        ),
        (
            "untag a missing report",
            "UntagResource",
            json!({ "ReportName": names[1], "TagKeys": ["env"] }),
        ),
    ];

    for (case, operation, request) in probes {
        let (status, response) = cur.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response:?}"
        );
        let rendered = response.to_string();
        for name in names {
            assert!(
                !rendered.contains(name),
                "case {case:?}: the error body must not echo the report name {name:?}, got {rendered}"
            );
        }
    }
}

#[tokio::test]
async fn an_input_constraint_refusal_still_reports_which_member_is_wrong() {
    // The one refusal that is not a resource-state one: the request itself is outside a
    // constraint, and such a refusal names the member the caller has to fix. Rows span
    // the two paths that produce it — the boundary validator (an enum catalog) and the
    // handler's own per-report tag cap.
    let cur = Cur::new();
    let (status, body) = cur.put(&minimal_definition("constrained"), None).await;
    assert_eq!(status, 200, "case seed: got {body:?}");

    let mut bad_enum = minimal_definition("bad-enum");
    bad_enum["TimeUnit"] = json!("WEEKLY");

    let over_cap: Vec<Value> = (0..=MAX_TAGS_PER_REPORT)
        .map(|index| json!({ "Key": format!("key-{index}"), "Value": "v" }))
        .collect();

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a definition member outside its modelled enum catalog",
            "PutReportDefinition",
            json!({ "ReportDefinition": bad_enum }),
        ),
        (
            "more tags than one report may hold",
            "TagResource",
            json!({ "ReportName": "constrained", "Tags": over_cap }),
        ),
    ];

    for (case, operation, request) in cases {
        let (status, response) = cur.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
        let message = error_message(&response)
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {case:?}: expected a diagnostic naming the offending member, got {response:?}"
        );
    }
}
