// SPDX-License-Identifier: CC0-1.0

//! The TEXT a refused request of this service carries, for the two refusals whose message
//! is the only thing that says what went wrong.
//!
//! `refusal_envelope.rs` pins which refusals carry no message at all. This suite pins the
//! wording of the two that do:
//!
//! 1. **A boundary constraint refusal.** Rule: a value outside a member's modelled
//!    constraint is refused with the counted envelope over one clause per offending
//!    member — `N validation error(s) detected: Value '<value>' at '<member path>' failed
//!    to satisfy constraint: <requirement>` — the quoted value is the one the CALLER
//!    SUBMITTED, however long it is, and a `@pattern` requirement quotes the expression a
//!    value has to satisfy WITHOUT the `^` / `$` the model wraps it in to make that match
//!    total. The table below spans every `@pattern`-constrained member
//!    this model declares and the operations that carry them, with the expected
//!    expression spelled per row from
//!    `smithy_query(shape="ReportName,S3Bucket,S3Prefix,BillingViewArn,LastDelivery,\
//!    GenericString")`; a non-pattern constraint is a row too, so the rest of the envelope
//!    is pinned unchanged.
//! 2. **The update path's not-found refusal.** Rule: updating a report the account does
//!    not hold is refused with the operation's own `ValidationException`, whose message
//!    identifies the report the update could not find — the name the request named, the
//!    type of report that was looked for, and the account the lookup ran against — because
//!    that message is the only discriminator between the conditions this one error code
//!    multiplexes. Rows differ in the spelling and length of the name and in how the
//!    account came not to hold it.

use serde_json::{json, Value};

use super::report_definition_lifecycle::{error_code, minimal_definition, Cur};

/// The longest name the model allows: `ReportName` is length 0..256
/// (`smithy_query(shape="ReportName")`).
const REPORT_NAME_MAX: usize = 256;

/// The `message` member of an error body, under either spelling: the model names the
/// member `Message`, and the JSON protocols render an error message member lowercase.
fn error_message(body: &Value) -> String {
    ["Message", "message"]
        .into_iter()
        .find_map(|key| body.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The clause the envelope states for one offending member.
fn clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// The requirement a `@pattern` violation states: the expression, as the service quotes
/// it.
fn pattern_requirement(expression: &str) -> String {
    format!("Member must satisfy regular expression pattern: {expression}")
}

/// One update of a report the account does not hold: the calls that build the account
/// state, and the name the update names.
struct Missing {
    case: &'static str,
    setup: Vec<(&'static str, Value)>,
    name: String,
}

/// One constraint refusal: the call that must be refused, and every clause its message
/// must state.
struct Constrained {
    case: &'static str,
    operation: &'static str,
    request: Value,
    clauses: Vec<String>,
}

/// A report definition with `member` replaced by `value`.
fn definition_with(member: &str, value: Value) -> Value {
    let mut definition = minimal_definition("message-probe");
    definition[member] = value;
    definition
}

fn put_body(definition: Value) -> Value {
    json!({ "ReportDefinition": definition })
}

/// The refusals under test, one row per `@pattern`-constrained member of this model plus
/// the rows that pin the envelope around them.
///
/// Each expected expression is the model's `@pattern` with the outer anchor pair dropped
/// and nothing else changed; every one is cited in the module header.
fn constrained_table() -> Vec<Constrained> {
    // The expressions, by the shape that declares them.
    let report_name = r"[0-9A-Za-z!\-_.*\'()]+";
    let s3_bucket = r"[A-Za-z0-9_\.\-]+";
    let s3_prefix = r"[0-9A-Za-z!\-_.*\'()/]*";
    let billing_view_arn =
        r"(arn:aws(-cn)?:billing::[0-9]{12}:billingview/)?[a-zA-Z0-9_\+=\.\-@].{1,30}";
    let last_delivery = r"[0-9]{8}[T][0-9]{6}([Z]|[+-][0-9]{4})";
    let generic_string = r"[A-Za-z0-9_\.\-=]*";

    vec![
        Constrained {
            case: "the definition's report name holds spaces",
            operation: "PutReportDefinition",
            request: put_body(definition_with("ReportName", json!("bad name with spaces"))),
            clauses: vec![clause(
                "bad name with spaces",
                "reportDefinition.reportName",
                &pattern_requirement(report_name),
            )],
        },
        Constrained {
            case: "the definition's report name holds a slash",
            operation: "PutReportDefinition",
            request: put_body(definition_with("ReportName", json!("reports/daily"))),
            clauses: vec![clause(
                "reports/daily",
                "reportDefinition.reportName",
                &pattern_requirement(report_name),
            )],
        },
        Constrained {
            case: "the bucket holds a slash",
            operation: "PutReportDefinition",
            request: put_body(definition_with("S3Bucket", json!("my-bucket/cur"))),
            clauses: vec![clause(
                "my-bucket/cur",
                "reportDefinition.s3Bucket",
                &pattern_requirement(s3_bucket),
            )],
        },
        Constrained {
            case: "the prefix holds a space",
            operation: "PutReportDefinition",
            request: put_body(definition_with("S3Prefix", json!("cur reports/"))),
            clauses: vec![clause(
                "cur reports/",
                "reportDefinition.s3Prefix",
                &pattern_requirement(s3_prefix),
            )],
        },
        Constrained {
            case: "the billing view names an 11-digit account",
            operation: "PutReportDefinition",
            request: put_body(definition_with(
                "BillingViewArn",
                json!("arn:aws:billing::12345678901:billingview/123456789012"),
            )),
            clauses: vec![clause(
                "arn:aws:billing::12345678901:billingview/123456789012",
                "reportDefinition.billingViewArn",
                &pattern_requirement(billing_view_arn),
            )],
        },
        Constrained {
            case: "a nested member: the last delivery is an ISO-8601 instant",
            operation: "PutReportDefinition",
            request: put_body(definition_with(
                "ReportStatus",
                json!({ "lastDelivery": "2024-01-01T00:00:00Z" }),
            )),
            clauses: vec![clause(
                "2024-01-01T00:00:00Z",
                "reportDefinition.reportStatus.lastDelivery",
                &pattern_requirement(last_delivery),
            )],
        },
        Constrained {
            case: "two offending members in one request",
            operation: "PutReportDefinition",
            request: put_body({
                let mut definition = definition_with("S3Bucket", json!("my bucket"));
                definition["S3Prefix"] = json!("cur reports/");
                definition
            }),
            clauses: vec![
                clause(
                    "my bucket",
                    "reportDefinition.s3Bucket",
                    &pattern_requirement(s3_bucket),
                ),
                clause(
                    "cur reports/",
                    "reportDefinition.s3Prefix",
                    &pattern_requirement(s3_prefix),
                ),
            ],
        },
        Constrained {
            case: "the page token holds a character outside its pattern",
            operation: "DescribeReportDefinitions",
            request: json!({ "NextToken": "not a token!" }),
            clauses: vec![clause(
                "not a token!",
                "nextToken",
                &pattern_requirement(generic_string),
            )],
        },
        Constrained {
            case: "the update names a report whose name holds spaces",
            operation: "ModifyReportDefinition",
            request: json!({
                "ReportName": "bad name with spaces",
                "ReportDefinition": minimal_definition("message-probe"),
            }),
            clauses: vec![clause(
                "bad name with spaces",
                "reportName",
                &pattern_requirement(report_name),
            )],
        },
        Constrained {
            case: "the tagged report's name holds a space",
            operation: "TagResource",
            request: json!({
                "ReportName": "bad name",
                "Tags": [{ "Key": "env", "Value": "staging" }],
            }),
            clauses: vec![clause(
                "bad name",
                "reportName",
                &pattern_requirement(report_name),
            )],
        },
        Constrained {
            case: "the listed report's name holds a space",
            operation: "ListTagsForResource",
            request: json!({ "ReportName": "bad name" }),
            clauses: vec![clause(
                "bad name",
                "reportName",
                &pattern_requirement(report_name),
            )],
        },
        Constrained {
            case: "the untagged report's name holds a space",
            operation: "UntagResource",
            request: json!({ "ReportName": "bad name", "TagKeys": ["env"] }),
            clauses: vec![clause(
                "bad name",
                "reportName",
                &pattern_requirement(report_name),
            )],
        },
        Constrained {
            case: "the deleted report's name holds a space",
            operation: "DeleteReportDefinition",
            request: json!({ "ReportName": "bad name" }),
            clauses: vec![clause(
                "bad name",
                "reportName",
                &pattern_requirement(report_name),
            )],
        },
        // A constraint that is not a `@pattern`, so the rest of the envelope is pinned
        // independently of the expression rendering: `MaxResults` carries range 5..5
        // (`smithy_query(operation="DescribeReportDefinitions")`), so each side of it is
        // out.
        Constrained {
            case: "the page size is one above its range",
            operation: "DescribeReportDefinitions",
            request: json!({ "MaxResults": 6 }),
            clauses: vec![clause(
                "6",
                "maxResults",
                "Member must have value less than or equal to 5",
            )],
        },
        Constrained {
            case: "the page size is one below its range",
            operation: "DescribeReportDefinitions",
            request: json!({ "MaxResults": 4 }),
            clauses: vec![clause(
                "4",
                "maxResults",
                "Member must have value greater than or equal to 5",
            )],
        },
        // A length bound, inside a list element: `Tag$Key` is length 1..128
        // (`smithy_query(shape="Tag")`), so an empty key is one character short of it.
        Constrained {
            case: "a tag key is one character short of its length bound",
            operation: "TagResource",
            request: json!({
                "ReportName": "message-probe",
                "Tags": [{ "Key": "", "Value": "staging" }],
            }),
            clauses: vec![clause(
                "",
                "tags.1.member.key",
                "Member must have length greater than or equal to 1",
            )],
        },
        // An enum catalog, for the same reason.
        Constrained {
            case: "the time unit is outside its catalog",
            operation: "PutReportDefinition",
            request: put_body(definition_with("TimeUnit", json!("WEEKLY"))),
            clauses: vec![clause(
                "WEEKLY",
                "reportDefinition.timeUnit",
                "Member must satisfy enum value set: [HOURLY, DAILY, MONTHLY]",
            )],
        },
        // The clause quotes the value the CALLER SUBMITTED, however long it is: a name one
        // character past `ReportName`'s 256-character maximum is quoted whole, not as a
        // prefix of itself.
        Constrained {
            case: "the report name is one character past its length bound",
            operation: "PutReportDefinition",
            request: put_body(definition_with(
                "ReportName",
                json!("r".repeat(REPORT_NAME_MAX + 1)),
            )),
            clauses: vec![clause(
                &"r".repeat(REPORT_NAME_MAX + 1),
                "reportDefinition.reportName",
                "Member must have length less than or equal to 256",
            )],
        },
        // The same over-long value breaking TWO of its member's constraints: both clauses
        // quote it whole.
        Constrained {
            case: "an over-long report name that also holds a space",
            operation: "PutReportDefinition",
            request: put_body(definition_with(
                "ReportName",
                json!(format!("{} x", "r".repeat(REPORT_NAME_MAX))),
            )),
            clauses: vec![
                clause(
                    &format!("{} x", "r".repeat(REPORT_NAME_MAX)),
                    "reportDefinition.reportName",
                    "Member must have length less than or equal to 256",
                ),
                clause(
                    &format!("{} x", "r".repeat(REPORT_NAME_MAX)),
                    "reportDefinition.reportName",
                    &pattern_requirement(report_name),
                ),
            ],
        },
    ]
}

#[tokio::test]
async fn a_constraint_refusal_states_the_counted_envelope_over_every_offending_member() {
    for row in constrained_table() {
        let Constrained {
            case,
            operation,
            request,
            clauses,
        } = row;
        let cur = Cur::new();
        let (status, response) = cur.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {operation}, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            error_code(&response)
        );

        let actual = error_message(&response);
        let opening = match clauses.len() {
            1 => "1 validation error detected: ".to_string(),
            n => format!("{n} validation errors detected: "),
        };
        assert!(
            actual.starts_with(&opening),
            "case {case:?}: expected the message to open with {opening:?}, got {actual:?}"
        );
        for expected in &clauses {
            assert!(
                actual.contains(expected.as_str()),
                "case {case:?}: expected the clause {expected:?}, got {actual:?}"
            );
        }
        assert_eq!(
            actual.len(),
            opening.len() + clauses.iter().map(String::len).sum::<usize>() + 2 * (clauses.len() - 1),
            "case {case:?}: expected exactly the clauses {clauses:?} joined into {opening:?}, \
             got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_quoted_pattern_carries_neither_anchor() {
    // The same rule stated the other way round, so a renderer that kept an anchor fails
    // here even if the clause it built were otherwise shaped right: a quoted expression
    // begins with no `^` and ends with no `$`.
    for row in constrained_table() {
        let Constrained {
            case,
            operation,
            request,
            ..
        } = row;
        let cur = Cur::new();
        let (status, response) = cur.call(operation, &request).await;
        let actual = error_message(&response);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {operation}, got {status} {response:?}"
        );
        assert!(
            !actual.is_empty(),
            "case {case:?}: expected a worded refusal, got {response:?}"
        );
        let marker = "Member must satisfy regular expression pattern: ";
        for quoted in actual.split(marker).skip(1) {
            let expression = quoted.split_once("; ").map_or(quoted, |(head, _)| head);
            assert!(
                !expression.starts_with('^') && !expression.ends_with('$'),
                "case {case:?}: the quoted expression must carry neither anchor, got \
                 {expression:?} in {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_clause_quotes_the_value_whole() {
    // The rule's other half, stated so a shortened echo fails here whatever else the
    // clause got right: a refusal quotes what the caller sent, so no message carries a
    // byte-count summary standing in for a value it would not quote.
    for row in constrained_table() {
        let Constrained {
            case,
            operation,
            request,
            ..
        } = row;
        let cur = Cur::new();
        let (status, response) = cur.call(operation, &request).await;
        let actual = error_message(&response);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from {operation}, got {status} {response:?}"
        );
        assert!(
            !actual.is_empty(),
            "case {case:?}: expected a worded refusal, got {response:?}"
        );
        assert!(
            !actual.contains(" bytes]"),
            "case {case:?}: the clause must quote the submitted value, not a summary of \
             it, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn updating_a_report_the_account_does_not_hold_names_report_type_and_account() {
    // Rows differ in the spelling and length of the name and in why the account does not
    // hold it: never created, created and deleted, and created under another name.
    let cases = vec![
        Missing {
            case: "a name the account never held",
            setup: vec![],
            name: "never-created".to_string(),
        },
        Missing {
            case: "a report that was created and then deleted",
            setup: vec![
                (
                    "PutReportDefinition",
                    put_body(minimal_definition("deleted-report")),
                ),
                (
                    "DeleteReportDefinition",
                    json!({ "ReportName": "deleted-report" }),
                ),
            ],
            name: "deleted-report".to_string(),
        },
        Missing {
            case: "another report exists under a different name",
            setup: vec![(
                "PutReportDefinition",
                put_body(minimal_definition("surviving-report")),
            )],
            name: "missing-report".to_string(),
        },
        Missing {
            case: "a name of the maximum modelled length",
            setup: vec![],
            name: "n".repeat(REPORT_NAME_MAX),
        },
        Missing {
            case: "a name using the pattern's punctuation",
            setup: vec![],
            name: "r3port!-_.*'()".to_string(),
        },
    ];

    for row in cases {
        let Missing { case, setup, name } = row;
        let missing = name.as_str();
        let cur = Cur::new();
        for (operation, body) in &setup {
            let (status, response) = cur.call(operation, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: setup call {operation} got {status} {response:?}"
            );
        }

        let (status, response) = cur
            .call(
                "ModifyReportDefinition",
                &json!({
                    "ReportName": missing,
                    "ReportDefinition": minimal_definition(missing),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            error_code(&response)
        );
        let expected = format!(
            "The report name [{missing}] and report type [CUR] to be update doesn't exist in \
             account [{}]",
            engine::emulator_account_id()
        );
        let actual = error_message(&response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );

        // The refused update left the store as it was: whatever the setup created is
        // still there, and the missing name was not conjured into existence.
        let listed = cur.describe_all().await;
        let names: Vec<&str> = listed
            .iter()
            .filter_map(|definition| definition["ReportName"].as_str())
            .collect();
        assert!(
            !names.contains(&missing),
            "case {case:?}: a refused update must store nothing, got {names:?}"
        );
        assert_eq!(
            names.len(),
            setup
                .iter()
                .filter(|(operation, _)| *operation == "PutReportDefinition")
                .count()
                - setup
                    .iter()
                    .filter(|(operation, _)| *operation == "DeleteReportDefinition")
                    .count(),
            "case {case:?}: expected the store unchanged by the refusal, got {names:?}"
        );
    }
}
