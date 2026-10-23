// SPDX-License-Identifier: CC0-1.0

//! How this service words a request the framework refused before any handler ran.
//!
//! The rule under test: a refusal is the AWS validation envelope — `N validation
//! error(s) detected:` followed by one clause per failing value, each naming the member
//! PATH and the constraint it failed — and a `@pattern` constraint is quoted as the
//! regular expression WITHOUT the `^` / `$` anchors that bind it to the whole value.
//! The rule is about the rendering of a constraint, not about any one member, so it is
//! asserted over a TABLE of refusals that differ along what it generalises over: which
//! operation, which member (a nested one and a top-level one), which pattern, and the
//! offending value.
//!
//! The clauses of the OTHER constraint classes are asserted too — an absent required
//! member and a length bound — because the rule changes how a pattern is rendered and
//! nothing else: a refusal that stopped naming its member, or a length bound that lost
//! its wording, would be the same defect from the other side.

use serde_json::{json, Value};

use crate::export_lifecycle::{cases, error_code, Client};

/// The patterns the model declares for the members under test (smithy_query shape
/// `Export` member `Name`, and shape `Arn`), as the model spells them — anchored.
const EXPORT_NAME_PATTERN_ANCHORED: &str = r"^[0-9A-Za-z\-_]+$";

/// The same two patterns as a validation message must quote them: the expression with
/// its anchors stripped. Written out rather than computed, so the expectation is the
/// text and not this test's copy of the implementation.
const EXPORT_NAME_PATTERN: &str = r"[0-9A-Za-z\-_]+";

/// `Arn`'s pattern as a validation message must quote it.
const ARN_PATTERN: &str =
    r"arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+";

/// An ARN-shaped string that is long enough to satisfy `Arn`'s `@length 20..2048` and
/// still outside its pattern, so the refusal is the pattern's and not the length's.
const NOT_AN_ARN: &str = "not-an-arn-but-long-enough";

/// One row: a request whose value is outside its member's pattern.
struct Row {
    label: &'static str,
    operation: &'static str,
    body: Value,
    /// The offending value, as the clause echoes it.
    value: String,
    /// The member path the clause names.
    path: &'static str,
    /// The pattern the clause must quote.
    pattern: &'static str,
}

/// The table of pattern refusals: both patterned members of this service, at every
/// operation that carries one, over values outside them in different ways.
fn rows() -> Vec<Row> {
    let valid_export = cases().remove(0).create_request()["Export"].clone();
    let export_with_name = |name: &str| {
        let mut export = valid_export.clone();
        export
            .as_object_mut()
            .expect("the Export structure is a JSON object")
            .insert("Name".to_string(), json!(name));
        export
    };

    let mut rows = Vec::new();
    // Export.Name, over values outside `^[0-9A-Za-z\-_]+$` in different ways: a space
    // (the recorded row), punctuation the class does not admit, a non-ASCII letter, and
    // a value made only of a disallowed character.
    for name in ["bad name!", "name.with.dots", "nämé", "%%%"] {
        rows.push(Row {
            label: "CreateExport with a name outside the pattern",
            operation: "CreateExport",
            body: json!({ "Export": export_with_name(name) }),
            value: name.to_string(),
            path: "export.name",
            pattern: EXPORT_NAME_PATTERN,
        });
    }
    // The same member on the other operation that carries an Export.
    rows.push(Row {
        label: "UpdateExport with a name outside the pattern",
        operation: "UpdateExport",
        body: json!({
            "ExportArn": engine::emulator_arn("bcm-data-exports", "us-east-1", "export/absent"),
            "Export": export_with_name("bad name!"),
        }),
        value: "bad name!".to_string(),
        path: "export.name",
        pattern: EXPORT_NAME_PATTERN,
    });
    // The Arn pattern, at every operation that takes an ARN.
    for (label, operation, body, path) in [
        (
            "GetExport with a malformed ARN",
            "GetExport",
            json!({ "ExportArn": NOT_AN_ARN }),
            "exportArn",
        ),
        (
            "DeleteExport with a malformed ARN",
            "DeleteExport",
            json!({ "ExportArn": NOT_AN_ARN }),
            "exportArn",
        ),
        (
            "UpdateExport with a malformed ARN",
            "UpdateExport",
            json!({ "ExportArn": NOT_AN_ARN, "Export": valid_export.clone() }),
            "exportArn",
        ),
        (
            "ListTagsForResource with a malformed ARN",
            "ListTagsForResource",
            json!({ "ResourceArn": NOT_AN_ARN }),
            "resourceArn",
        ),
        (
            "TagResource with a malformed ARN",
            "TagResource",
            json!({
                "ResourceArn": NOT_AN_ARN,
                "ResourceTags": [{ "Key": "env", "Value": "test" }],
            }),
            "resourceArn",
        ),
        (
            "UntagResource with a malformed ARN",
            "UntagResource",
            json!({ "ResourceArn": NOT_AN_ARN, "ResourceTagKeys": ["env"] }),
            "resourceArn",
        ),
        (
            "ListExecutions with a malformed ARN",
            "ListExecutions",
            json!({ "ExportArn": NOT_AN_ARN }),
            "exportArn",
        ),
    ] {
        rows.push(Row {
            label,
            operation,
            body,
            value: NOT_AN_ARN.to_string(),
            path,
            pattern: ARN_PATTERN,
        });
    }
    rows
}

/// The `message` a refused request carried, with its status and code checked first.
async fn refusal_message(client: &Client, label: &str, operation: &str, body: &Value) -> String {
    let (status, response) = client.call(operation, body).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ValidationException"),
        "case {label:?}: expected a 400 ValidationException, got {status} with {response}"
    );
    response
        .get("message")
        .or_else(|| response.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {label:?}: the refusal must carry a message, got {response}"))
        .to_string()
}

#[tokio::test]
async fn a_pattern_constraint_is_quoted_without_its_anchors() {
    let client = Client::new();

    for row in rows() {
        let case = format!("{} ({})", row.label, row.value);
        let message = refusal_message(&client, &case, row.operation, &row.body).await;
        let expected = format!(
            "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: Member must satisfy regular expression pattern: {}",
            row.value, row.path, row.pattern
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(EXPORT_NAME_PATTERN_ANCHORED) && !message.contains("+$"),
            "case {case:?}: the message must not quote the anchored pattern, got {message:?}"
        );
    }
}

#[tokio::test]
async fn several_failing_values_are_stated_as_one_envelope() {
    let client = Client::new();

    // Two members outside their constraints at once: Name outside its pattern and
    // Description past `@length 0..1024` (smithy_query shape `Export`, member
    // `Description`).
    let mut export = cases().remove(0).create_request()["Export"].clone();
    let object = export
        .as_object_mut()
        .expect("the Export structure is a JSON object");
    object.insert("Name".to_string(), json!("bad name!"));
    object.insert("Description".to_string(), json!("d".repeat(1025)));

    let case = "two failing values";
    let message = refusal_message(&client, case, "CreateExport", &json!({ "Export": export })).await;
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case:?}: expected the plural envelope, got {message:?}"
    );
    assert!(
        message.contains(&format!(
            "Value 'bad name!' at 'export.name' failed to satisfy constraint: Member must satisfy regular expression pattern: {EXPORT_NAME_PATTERN}"
        )),
        "case {case:?}: expected the unanchored pattern clause, got {message:?}"
    );
    assert!(
        message.contains(
            "at 'export.description' failed to satisfy constraint: Member must have length less than or equal to 1024"
        ),
        "case {case:?}: expected the length clause, got {message:?}"
    );
    assert!(
        message.contains("; "),
        "case {case:?}: the clauses must be joined with '; ', got {message:?}"
    );
}

/// One row: a refusal of a class that is NOT a pattern, and the clause it must state.
const OTHER_CONSTRAINT_CLAUSES: &[(&str, &str)] = &[
    (
        "absent required member",
        "Value null at 'export' failed to satisfy constraint: Member must not be null",
    ),
    (
        "a name past its length bound",
        "failed to satisfy constraint: Member must have length less than or equal to 128",
    ),
    (
        "an ARN below its length bound",
        "failed to satisfy constraint: Member must have length greater than or equal to 20",
    ),
];

#[tokio::test]
async fn the_other_constraint_classes_keep_their_wording() {
    let client = Client::new();
    let valid_export = cases().remove(0).create_request()["Export"].clone();
    let long_name = {
        let mut export = valid_export.clone();
        export
            .as_object_mut()
            .expect("the Export structure is a JSON object")
            // `Export.Name` is `@length 1..128`, so one character past the ceiling.
            .insert("Name".to_string(), json!("N".repeat(129)));
        export
    };

    let requests = [
        ("CreateExport", json!({})),
        ("CreateExport", json!({ "Export": long_name })),
        ("GetExport", json!({ "ExportArn": "arn:aws:short" })),
    ];
    for ((case, expected), (operation, body)) in OTHER_CONSTRAINT_CLAUSES.iter().zip(requests) {
        let message = refusal_message(&client, case, operation, &body).await;
        // One envelope, singular or plural: a value can fail a length bound and a
        // pattern at once, and both clauses then go in the one sentence.
        assert!(
            message.starts_with("1 validation error detected: ")
                || message.contains(" validation errors detected: "),
            "case {case:?}: expected the AWS validation envelope, got {message:?}"
        );
        assert!(
            message.contains(expected),
            "case {case:?}: expected a clause containing {expected:?}, got {message:?}"
        );
    }
}
