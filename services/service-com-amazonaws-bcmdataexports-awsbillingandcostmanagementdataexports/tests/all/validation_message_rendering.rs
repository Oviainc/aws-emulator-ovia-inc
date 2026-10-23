// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal, over the awsJson1_1 wire.
//!
//! Two rules are under test, both about the sentence a refused caller reads.
//!
//! 1. A request that breaks a modelled input constraint is answered HTTP 400
//!    `ValidationException` whose `Message` is the AWS constraint envelope —
//!    `1 validation error detected: Value '<value>' at '<member path>' failed to satisfy
//!    constraint: <requirement>`, and `N validation errors detected: …; …` for several —
//!    where a `@pattern` requirement quotes the expression WITHOUT the `^` / `$` anchors
//!    the model spells it with. A client that lifts the advertised expression out of the
//!    message to repair its request must receive the expression the service advertises,
//!    not the model's anchored source.
//!
//! 2. A `GetTable` naming a table the catalog does not publish is answered
//!    `ValidationException` whose `Message` is BUILT FROM THE REQUEST: it names the table
//!    that was asked for and reports it as not found.
//!
//! Both are asserted over TABLES of inputs that differ along what the rules generalise
//! over: rule 1 over every constraint class this model's input shapes spell (`@pattern`,
//! both `@length` bounds on a string and on a list, both `@range` bounds, an absent
//! `@required` member), on members of four different operations, at three different path
//! depths (a top-level member, a nested one, a doubly-nested one), including a request
//! that breaks two constraints at once; rule 2 over table names that miss the catalog in
//! every way a name can (unknown, wrong case, padded, empty, outsized).
//!
//! Every constraint quoted below is cited from the model (`smithy_query(query="operation",
//! operation="…")` / `query="shape"`), and each row's expected clause is built from that
//! citation rather than from the emulator's output.

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::TABLE_CATALOG;

use crate::export_lifecycle::{cases, error_code, Client};

/// `Export$Name`'s `@pattern`, as the MODEL spells it — anchored (smithy_query shape
/// `Export`, member `Name`: `@length(1, 128)`, `@pattern`).
const EXPORT_NAME_PATTERN: &str = r"^[0-9A-Za-z\-_]+$";

/// `Arn`'s `@pattern`, as the model spells it (smithy_query shape `Arn`: `@length(20,
/// 2048)`, `@pattern`). Reached by `GetExport$ExportArn`, `UpdateExport$ExportArn`,
/// `DeleteExport$ExportArn`, `ListTagsForResource$ResourceArn`, `GetExecution$ExportArn`.
const ARN_PATTERN: &str =
    r"^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$";

/// One refused request: the operation, its body, and the clauses the envelope must carry.
struct Row {
    label: &'static str,
    operation: &'static str,
    body: Value,
    /// One clause per violation the request breaks, as the rule renders it.
    clauses: Vec<String>,
}

/// The clause for a value that is present but outside its constraint.
fn clause(value: &str, path: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// A `@pattern` requirement, with the anchors the rule strips removed here the same way:
/// by naming the UNANCHORED expression explicitly, so the expectation is not computed by
/// the code under test.
fn pattern_requirement(unanchored: &str) -> String {
    format!("Member must satisfy regular expression pattern: {unanchored}")
}

/// A well-formed `Export` with `name` and `query_statement` substituted, so a row varies
/// exactly one member of an otherwise valid definition.
fn export_with(name: &str, query_statement: &str) -> Value {
    let mut export = cases().remove(0).create_request()["Export"].clone();
    let object = export.as_object_mut().expect("Export is a JSON object");
    object.insert("Name".to_string(), json!(name));
    object.insert(
        "DataQuery".to_string(),
        json!({ "QueryStatement": query_statement }),
    );
    export
}

/// The table of refused requests.
fn rows() -> Vec<Row> {
    let valid_query = "SELECT identity_line_item_id FROM COST_AND_USAGE_REPORT";
    // 201 tags: one past `ResourceTagList`'s `@length(0, 200)` (smithy_query operation
    // `CreateExport`, member `ResourceTags`; `TagResource`, member `ResourceTags`).
    let too_many_tags: Vec<Value> = (0..=200)
        .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
        .collect();
    let max_length_200 = "Member must have length less than or equal to 200".to_string();
    let name_pattern = pattern_requirement(r"[0-9A-Za-z\-_]+");
    let arn_pattern =
        pattern_requirement(r"arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+");

    vec![
        // @pattern, on a NESTED member (`Export.Name`) — the recorded case.
        Row {
            label: "a name outside Export.Name's pattern",
            operation: "CreateExport",
            body: json!({ "Export": export_with("bad name! (not allowed)", valid_query) }),
            clauses: vec![clause(
                "bad name! (not allowed)",
                "export.name",
                &name_pattern,
            )],
        },
        // The same constraint, a different offending value: the rule is about the
        // CONSTRAINT's rendering, not about one rejected name.
        Row {
            label: "a name with a dot in it",
            operation: "CreateExport",
            body: json!({ "Export": export_with("export.name.with.dots", valid_query) }),
            clauses: vec![clause(
                "export.name.with.dots",
                "export.name",
                &name_pattern,
            )],
        },
        // @length minimum, on the same nested member. An empty name breaks the minimum AND
        // the pattern, so this row also pins how SEVERAL clauses about ONE member read.
        Row {
            label: "an empty name",
            operation: "CreateExport",
            body: json!({ "Export": export_with("", valid_query) }),
            clauses: vec![
                clause(
                    "",
                    "export.name",
                    "Member must have length greater than or equal to 1",
                ),
                clause("", "export.name", &name_pattern),
            ],
        },
        // @length maximum, on a DOUBLY nested member: `Export.DataQuery.QueryStatement`
        // is `@length(1, 36000)` (smithy_query shape `DataQuery`, member
        // `QueryStatement`), so the empty statement breaks its minimum.
        Row {
            label: "an empty query statement",
            operation: "CreateExport",
            body: json!({ "Export": export_with("valid_name", "") }),
            clauses: vec![clause(
                "",
                "export.dataQuery.queryStatement",
                "Member must have length greater than or equal to 1",
            )],
        },
        // @length on a LIST member, which is what the rendered list bound reports.
        Row {
            label: "201 resource tags on a create",
            operation: "CreateExport",
            body: json!({
                "Export": export_with("tagged_export", valid_query),
                "ResourceTags": too_many_tags,
            }),
            clauses: vec![clause("201 items", "resourceTags", &max_length_200)],
        },
        // The same list bound on the other operation that takes the tag list, so the
        // renderer is shown to be the service's and not one operation's.
        Row {
            label: "201 resource tags on TagResource",
            operation: "TagResource",
            body: json!({
                "ResourceArn": ::engine::emulator_arn(
                    "bcm-data-exports",
                    "us-east-1",
                    "export/tagged_export",
                ),
                "ResourceTags": too_many_tags,
            }),
            clauses: vec![clause("201 items", "resourceTags", &max_length_200)],
        },
        // @pattern on a TOP-LEVEL member, on a read.
        Row {
            label: "an ExportArn that is not an ARN",
            operation: "GetExport",
            body: json!({ "ExportArn": "not-an-arn-but-long-enough-to-pass" }),
            clauses: vec![clause(
                "not-an-arn-but-long-enough-to-pass",
                "exportArn",
                &arn_pattern,
            )],
        },
        // The same pattern, reached through another operation's member name.
        Row {
            label: "a ResourceArn that is not an ARN",
            operation: "ListTagsForResource",
            body: json!({ "ResourceArn": "arn:aws:iam::000000000000:role/not-this-service" }),
            clauses: vec![clause(
                "arn:aws:iam::000000000000:role/not-this-service",
                "resourceArn",
                &arn_pattern,
            )],
        },
        // @range maximum: `MaxResults` is `@range(1, 300)` (smithy_query operation
        // `ListExports`, member `MaxResults`).
        Row {
            label: "MaxResults one past the maximum",
            operation: "ListExports",
            body: json!({ "MaxResults": 301 }),
            clauses: vec![clause(
                "301",
                "maxResults",
                "Member must have value less than or equal to 300",
            )],
        },
        // @range minimum.
        Row {
            label: "MaxResults one below the minimum",
            operation: "ListExports",
            body: json!({ "MaxResults": 0 }),
            clauses: vec![clause(
                "0",
                "maxResults",
                "Member must have value greater than or equal to 1",
            )],
        },
        // An absent @required member has no value to quote.
        Row {
            label: "a GetExecution without its ExecutionId",
            operation: "GetExecution",
            body: json!({
                "ExportArn": ::engine::emulator_arn(
                    "bcm-data-exports",
                    "us-east-1",
                    "export/absent",
                ),
            }),
            clauses: vec![
                "Value null at 'executionId' failed to satisfy constraint: Member must not be null"
                    .to_string(),
            ],
        },
        // Violations across TWO members of one request: the envelope counts them.
        Row {
            label: "an empty name beside an empty query statement",
            operation: "CreateExport",
            body: json!({ "Export": export_with("", "") }),
            clauses: vec![
                clause(
                    "",
                    "export.name",
                    "Member must have length greater than or equal to 1",
                ),
                clause("", "export.name", &name_pattern),
                clause(
                    "",
                    "export.dataQuery.queryStatement",
                    "Member must have length greater than or equal to 1",
                ),
            ],
        },
    ]
}

#[tokio::test]
async fn a_boundary_refusal_states_the_aws_envelope_with_an_unanchored_pattern() {
    let client = Client::new();

    for row in rows() {
        let case = row.label;
        let (status, body) = client.call(row.operation, &row.body).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        // A boundary refusal is rendered by the framework's pre-dispatch envelope, which
        // spells the member lowercase, while a handler-returned modelled error spells it
        // as the model does (`Message`). Both are read here, so the rule under test is the
        // SENTENCE and not which envelope carried it.
        let message = body["Message"]
            .as_str()
            .or_else(|| body["message"].as_str())
            .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry a Message: {body}"))
            .to_string();

        let expected = match row.clauses.len() {
            1 => format!("1 validation error detected: {}", row.clauses[0]),
            n => format!("{n} validation errors detected: "),
        };
        if row.clauses.len() == 1 {
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        } else {
            assert!(
                message.starts_with(&expected),
                "case {case:?}: expected the envelope to start {expected:?}, got {message:?}"
            );
            for clause in &row.clauses {
                assert!(
                    message.contains(clause),
                    "case {case:?}: expected the envelope to carry {clause:?}, got {message:?}"
                );
            }
        }

        // The anchored source of a pattern never reaches the wire.
        for anchored in [EXPORT_NAME_PATTERN, ARN_PATTERN] {
            assert!(
                !message.contains(anchored),
                "case {case:?}: the message must not quote the anchored pattern {anchored:?}, got {message:?}"
            );
        }
        assert!(
            message.len() <= 1024,
            "case {case:?}: the Message must satisfy the modelled @length(0, 1024), got {} characters",
            message.len()
        );
    }
}

/// A value that satisfies every constraint is NOT refused by this renderer: the rows above
/// fail because of the member they vary, not because the renderer refuses everything.
#[tokio::test]
async fn a_request_inside_every_constraint_is_served() {
    let client = Client::new();

    let created = client
        .ok(
            "CreateExport",
            &json!({
                "Export": export_with(
                    "Within-All_Constraints0",
                    "SELECT identity_line_item_id FROM COST_AND_USAGE_REPORT",
                ),
                "ResourceTags": [{ "Key": "k", "Value": "v" }],
            }),
        )
        .await;
    assert!(
        created["ExportArn"].is_string(),
        "a definition inside every constraint must be created, got {created}"
    );
    let listed = client.ok("ListExports", &json!({ "MaxResults": 300 })).await;
    assert_eq!(
        listed["Exports"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "MaxResults at the published maximum must be accepted, got {listed}"
    );
}

/// Table names that miss the catalog in every way a name can.
fn unknown_table_names() -> Vec<String> {
    let mut names = vec![
        // The recorded case.
        "NOT_A_REAL_TABLE_XYZ".to_string(),
        // A published name in the wrong case, and one with padding: neither is published.
        "cost_and_usage_report".to_string(),
        "COST_AND_USAGE_REPORT ".to_string(),
        " ".to_string(),
        String::new(),
        // A name that is published by no table but looks like one.
        "COST_AND_USAGE_REPORT_V3".to_string(),
    ];
    // A name built from a published one by appending to it, for every catalog table, so
    // the rule is exercised against a name the issue never cited for each table.
    for table in TABLE_CATALOG {
        names.push(format!("{}_NOT_PUBLISHED", table.name));
    }
    names
}

#[tokio::test]
async fn an_unknown_table_is_reported_as_not_found_by_name() {
    let client = Client::new();

    for name in unknown_table_names() {
        let (status, body) = client
            .call("GetTable", &json!({ "TableName": name }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {name:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        let message = body["Message"].as_str().unwrap_or_default();
        assert_eq!(
            message,
            format!("Table {name} not found"),
            "case {name:?}: expected the request-derived not-found sentence, got {message:?}"
        );
    }

    // Every name the catalog DOES publish is served, so the sentence above is the answer
    // to a name outside the catalog and not to every name.
    for table in TABLE_CATALOG {
        let body = client
            .ok("GetTable", &json!({ "TableName": table.name }))
            .await;
        assert_eq!(
            body["TableName"].as_str(),
            Some(table.name),
            "case {:?}: a published table must be described, got {body}",
            table.name
        );
    }
}
