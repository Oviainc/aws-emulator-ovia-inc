// SPDX-License-Identifier: CC0-1.0

//! What a Config query over this account's recorded resources answers, end to end over
//! the wire.
//!
//! The rule pinned here: `SelectResourceConfig` READS the `Expression` as the SQL
//! `SELECT` command the Config query language is, runs it over the configuration items
//! Config has recorded, and answers the properties the query selected — while an
//! expression that is NOT such a command is refused 400 `InvalidExpressionException`,
//! never answered with an empty result set. The two are different answers and a client
//! has to be able to tell them apart, which is why every table below asserts a query
//! that matched nothing beside the refusals.
//!
//! Pinned with it: `QueryInfo` names the fields the query selected, `Limit` and
//! `NextToken` page the results, and a token outside the opaque form this operation
//! issues is `InvalidNextTokenException`.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds. The recorded
//! resources are seeded in the store the way a handler reaches it (`service_state`, as
//! `aggregate_resource_config` seeds the reads it pins): the resource types a Config
//! query selects are the modelled `ResourceType` enum, while `PutResourceConfig` records
//! CUSTOM resource types, so a configuration item of an Amazon Web Services resource
//! type cannot be put over the wire by any implemented operation.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, DiscoveredResourceEntry,
};

use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

/// Resource types the rows use. Entries of `com.amazonaws.configservice#ResourceType`,
/// from `smithy_query(query="shape", shape="ResourceType")`.
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const DYNAMODB_TABLE: &str = "AWS::DynamoDB::Table";
/// See [`EC2_VOLUME`].
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`EC2_VOLUME`].
const LAMBDA_FUNCTION: &str = "AWS::Lambda::Function";
/// See [`EC2_VOLUME`]: a type nothing in the seeded inventory is of.
const RDS_INSTANCE: &str = "AWS::RDS::DBInstance";

/// The instant a seeded resource's configuration item was captured.
fn captured_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

/// One resource the store holds: its type, its id, the configuration document recorded
/// for it (if any), its tags, and whether its deletion is recorded.
struct Recorded {
    resource_type: &'static str,
    resource_id: &'static str,
    configuration: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    deleted: bool,
}

/// The inventory every table runs its queries over.
fn inventory() -> Vec<Recorded> {
    vec![
        Recorded {
            resource_type: EC2_VOLUME,
            resource_id: "vol-0001",
            configuration: Some(r#"{"state":"available","size":100}"#),
            tags: &[("owner", "platform"), ("env", "prod")],
            deleted: false,
        },
        Recorded {
            resource_type: EC2_VOLUME,
            resource_id: "vol-0002",
            configuration: Some(r#"{"state":"in-use","size":500}"#),
            tags: &[("owner", "data"), ("env", "prod")],
            deleted: false,
        },
        Recorded {
            resource_type: DYNAMODB_TABLE,
            resource_id: "orders",
            configuration: Some(r#"{"state":"ACTIVE","size":20}"#),
            tags: &[("owner", "data"), ("env", "dev")],
            deleted: false,
        },
        Recorded {
            resource_type: S3_BUCKET,
            resource_id: "config-bucket",
            configuration: Some(r#"{"state":"available"}"#),
            tags: &[],
            deleted: false,
        },
        Recorded {
            resource_type: LAMBDA_FUNCTION,
            resource_id: "evaluate",
            configuration: None,
            tags: &[],
            deleted: false,
        },
        Recorded {
            resource_type: EC2_VOLUME,
            resource_id: "vol-0009",
            configuration: Some(r#"{"state":"deleted"}"#),
            tags: &[],
            deleted: true,
        },
    ]
}

/// An emulator with a configuration recorder, holding [`inventory`] in its
/// discovered-resource inventory.
async fn emulator_holding_inventory() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
        .await;
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    {
        let mut store = state.lock();
        for resource in inventory() {
            store.discovered_resources.insert(
                (
                    resource.resource_type.to_string(),
                    resource.resource_id.to_string(),
                ),
                DiscoveredResourceEntry {
                    resource_type: resource.resource_type.to_string(),
                    resource_id: resource.resource_id.to_string(),
                    resource_name: Some(resource.resource_id.to_string()),
                    deleted_at: resource.deleted.then(captured_at),
                    schema_version_id: resource
                        .configuration
                        .map(|_| "00000000-0000-0000-0000-000000000001".to_string()),
                    configuration: resource.configuration.map(str::to_string),
                    tags: resource
                        .tags
                        .iter()
                        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                        .collect(),
                    first_recorded_at: resource.configuration.map(|_| captured_at()),
                    last_recorded_at: resource.configuration.map(|_| captured_at()),
                    ..DiscoveredResourceEntry::default()
                },
            );
        }
    }
    emulator
}

/// The `SelectResourceConfig` body for `expression`.
fn select(expression: &str) -> Value {
    json!({ "Expression": expression })
}

/// `Limit`'s published maximum — "The maximum number of query results returned on each
/// page", `Limit` range 0..100 from
/// `smithy_query(query="operation", operation="SelectResourceConfig")`.
const MAX_LIMIT: i32 = 100;

/// Every aggregate function the Config query language has, from the Query Components
/// section of the Config Developer Guide, which lists AVG, COUNT, MAX, MIN and SUM.
///
/// Each entry is the field as a query selects it, paired with what the function computes
/// over [`inventory`]: the sizes the recorded configuration documents carry are 100
/// (`vol-0001`), 500 (`vol-0002`) and 20 (`orders`), while the two resources whose
/// configuration carries no size contribute nothing to a function over that property —
/// and `COUNT(*)` counts the matched ITEMS rather than the values of one property, so it
/// counts all five resources whose deletion is not recorded.
const AGGREGATES: [(&str, &str, f64); 6] = [
    ("the average of the sizes", "AVG(configuration.size)", 620.0 / 3.0),
    ("the values of a property", "COUNT(configuration.size)", 3.0),
    ("the matched items", "COUNT(*)", 5.0),
    ("the largest size", "MAX(configuration.size)", 500.0),
    ("the smallest size", "MIN(configuration.size)", 20.0),
    ("the total of the sizes", "SUM(configuration.size)", 620.0),
];

/// The results of a select response, each parsed back out of the JSON document it is.
fn results(case: &str, body: &Value) -> Vec<Value> {
    body["Results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Results, got {body}"))
        .iter()
        .map(|result| {
            let text = result
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a result is a JSON document, got {body}"));
            serde_json::from_str::<Value>(text)
                .unwrap_or_else(|_| panic!("case {case:?}: a result parses as JSON, got {text:?}"))
        })
        .collect()
}

#[tokio::test]
async fn an_expression_that_is_not_a_config_query_is_refused_rather_than_answered_empty() {
    // Rows differ along what the rule generalises over: HOW the expression fails to be a
    // Config query — a missing projection, a misspelled keyword, a truncated condition,
    // a clause the documented query form has no place for (FROM, a set operation, a
    // subquery), more than one statement, a statement that is not a query, a function
    // that is not one a Config query selects, and an aggregate beside a property the
    // query does not group by.
    let cases: [(&str, String); 13] = [
        (
            "the recorded malformed expression",
            "SELECT WHERE".to_string(),
        ),
        ("a misspelled SELECT", "SELEC resourceId".to_string()),
        (
            "a WHERE with no condition",
            "SELECT resourceId WHERE".to_string(),
        ),
        (
            "a condition with no value",
            "SELECT resourceId WHERE resourceType =".to_string(),
        ),
        (
            "a FROM clause",
            "SELECT resourceId FROM aws_config".to_string(),
        ),
        (
            "a set operation",
            "SELECT resourceId UNION SELECT resourceType".to_string(),
        ),
        (
            "a subquery in the condition",
            "SELECT resourceId WHERE resourceId IN (SELECT resourceId)".to_string(),
        ),
        (
            "two statements",
            "SELECT resourceId; SELECT resourceType".to_string(),
        ),
        (
            "a statement that is not a query",
            "DELETE FROM resources".to_string(),
        ),
        (
            "a function a Config query does not select",
            "SELECT UPPER(resourceId)".to_string(),
        ),
        (
            "an aggregate beside an ungrouped property",
            "SELECT resourceType, COUNT(*)".to_string(),
        ),
        (
            "a bare word",
            "resourceId".to_string(),
        ),
        (
            "a condition nesting far past what a Config query writes",
            format!(
                "SELECT resourceId WHERE {}",
                (0..40)
                    .map(|_| "resourceType = 'AWS::EC2::Volume'")
                    .collect::<Vec<&str>>()
                    .join(" AND ")
            ),
        ),
    ];

    let emulator = emulator_holding_inventory().await;
    for (case, expression) in cases {
        let (status, code) = emulator
            .refused("SelectResourceConfig", &select(&expression))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidExpressionException"),
            "case {case:?}: {expression:?} is not a Config query and must be refused 400 \
             InvalidExpressionException, got {status} {code}"
        );
    }

    // A query that IS one and matches nothing is the other answer: an empty result set,
    // which is what the refusals above must not be confused with.
    let expression = format!("SELECT resourceId WHERE resourceType = '{RDS_INSTANCE}'");
    let body = emulator.ok("SelectResourceConfig", &select(&expression)).await;
    assert_eq!(
        results("a query that matched nothing", &body),
        Vec::<Value>::new(),
        "a well formed query that matched nothing answers an empty result set, got {body}"
    );
}

#[tokio::test]
async fn a_config_query_answers_the_properties_it_selected_of_the_resources_it_matched() {
    // Rows differ along what the rule generalises over: how many properties the query
    // selects, whether a property is nested, which operator the condition uses (=, IN,
    // LIKE, a numeric ordering, IS NULL, AND/OR), whether the query groups and
    // aggregates, whether it orders, and whether it matches many resources, one or none.
    let cases: [(&str, String, Value); 14] = [
        (
            "one property of one resource type",
            format!("SELECT resourceId WHERE resourceType = '{EC2_VOLUME}'"),
            json!([{ "resourceId": "vol-0001" }, { "resourceId": "vol-0002" }]),
        ),
        (
            "two properties, matched by a pattern",
            "SELECT resourceId, resourceType WHERE resourceId LIKE 'vol-%'".to_string(),
            json!([
                { "resourceId": "vol-0001", "resourceType": EC2_VOLUME },
                { "resourceId": "vol-0002", "resourceType": EC2_VOLUME },
            ]),
        ),
        (
            "a pattern matching a single character",
            "SELECT resourceId WHERE resourceId LIKE 'vol-000_'".to_string(),
            json!([{ "resourceId": "vol-0001" }, { "resourceId": "vol-0002" }]),
        ),
        (
            "several resource types by IN",
            format!(
                "SELECT resourceId WHERE resourceType IN ('{S3_BUCKET}', '{DYNAMODB_TABLE}')"
            ),
            json!([{ "resourceId": "orders" }, { "resourceId": "config-bucket" }]),
        ),
        (
            "a nested property, selected and filtered on",
            "SELECT resourceId, configuration.state WHERE configuration.state = 'in-use'"
                .to_string(),
            json!([{ "resourceId": "vol-0002", "configuration": { "state": "in-use" } }]),
        ),
        (
            "a numeric ordering on a nested property",
            "SELECT resourceId WHERE configuration.size > 100".to_string(),
            json!([{ "resourceId": "vol-0002" }]),
        ),
        (
            "a numeric range",
            "SELECT resourceId WHERE configuration.size BETWEEN 20 AND 100".to_string(),
            json!([{ "resourceId": "orders" }, { "resourceId": "vol-0001" }]),
        ),
        (
            "two conditions joined by AND",
            format!(
                "SELECT resourceId WHERE resourceType = '{EC2_VOLUME}' AND tags.owner = 'data'"
            ),
            json!([{ "resourceId": "vol-0002" }]),
        ),
        (
            "two conditions joined by OR",
            format!(
                "SELECT resourceId WHERE resourceType = '{S3_BUCKET}' OR resourceType = \
                 '{LAMBDA_FUNCTION}'"
            ),
            json!([{ "resourceId": "evaluate" }, { "resourceId": "config-bucket" }]),
        ),
        (
            "a tag of the resource",
            "SELECT resourceId, tags.env WHERE tags.env = 'prod'".to_string(),
            json!([
                { "resourceId": "vol-0001", "tags": { "env": "prod" } },
                { "resourceId": "vol-0002", "tags": { "env": "prod" } },
            ]),
        ),
        (
            "a property the resource carries no value for",
            "SELECT resourceId WHERE configuration IS NULL".to_string(),
            json!([{ "resourceId": "evaluate" }]),
        ),
        (
            "a negated pattern",
            format!(
                "SELECT resourceId WHERE resourceType = '{EC2_VOLUME}' AND resourceId NOT LIKE \
                 '%0001'"
            ),
            json!([{ "resourceId": "vol-0002" }]),
        ),
        (
            "an ordering the query asks for",
            format!(
                "SELECT resourceId WHERE resourceType = '{EC2_VOLUME}' ORDER BY resourceId DESC"
            ),
            json!([{ "resourceId": "vol-0002" }, { "resourceId": "vol-0001" }]),
        ),
        (
            "a count per group",
            "SELECT resourceType, COUNT(*) GROUP BY resourceType".to_string(),
            json!([
                { "resourceType": DYNAMODB_TABLE, "COUNT(*)": 1 },
                { "resourceType": EC2_VOLUME, "COUNT(*)": 2 },
                { "resourceType": LAMBDA_FUNCTION, "COUNT(*)": 1 },
                { "resourceType": S3_BUCKET, "COUNT(*)": 1 },
            ]),
        ),
    ];

    let emulator = emulator_holding_inventory().await;
    for (case, expression, expected) in cases {
        let body = emulator.ok("SelectResourceConfig", &select(&expression)).await;
        assert_eq!(
            Value::Array(results(case, &body)),
            expected,
            "case {case:?}: expected {expression:?} to answer {expected}, got {body}"
        );

        // The query is a report: running it again must answer the same thing.
        let again = emulator.ok("SelectResourceConfig", &select(&expression)).await;
        assert_eq!(
            again, body,
            "case {case:?}: a second run of the same query must answer the same, got {again}"
        );
    }
}

#[tokio::test]
async fn the_query_info_names_the_fields_the_query_selected() {
    // Rows differ along what is selected: one property, several, a nested one, an
    // aggregate beside the property it is grouped by, and every member of the item.
    let cases: [(&str, String, Vec<&str>); 5] = [
        (
            "one property",
            "SELECT resourceId".to_string(),
            vec!["resourceId"],
        ),
        (
            "several properties",
            "SELECT resourceId, resourceType, awsRegion".to_string(),
            vec!["resourceId", "resourceType", "awsRegion"],
        ),
        (
            "a nested property",
            "SELECT configuration.state".to_string(),
            vec!["configuration.state"],
        ),
        (
            "an aggregate beside its group",
            "SELECT resourceType, COUNT(*) GROUP BY resourceType".to_string(),
            vec!["resourceType", "COUNT(*)"],
        ),
        ("every member", "SELECT *".to_string(), vec!["*"]),
    ];

    let emulator = emulator_holding_inventory().await;
    for (case, expression, expected) in cases {
        let body = emulator.ok("SelectResourceConfig", &select(&expression)).await;
        let fields: Vec<String> = body["QueryInfo"]["SelectFields"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected QueryInfo.SelectFields, got {body}"))
            .iter()
            .map(|field| field["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            fields,
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the selected fields {expected:?}, got {body}"
        );
    }

    // `SELECT *` answers every member the configuration item carries, for every
    // resource the inventory holds that has not been deleted.
    let body = emulator.ok("SelectResourceConfig", &select("SELECT *")).await;
    let rows = results("every member", &body);
    assert_eq!(
        rows.len(),
        5,
        "every recorded resource that is not deleted is answered, got {body}"
    );
    assert!(
        rows.iter().all(|row| row["resourceId"].is_string()
            && row["resourceType"].is_string()
            && row["accountId"].is_string()),
        "each row carries the members of the configuration item, got {rows:?}"
    );
}

#[tokio::test]
async fn a_query_pages_its_results_at_the_limit_the_request_asks_for() {
    // Rows differ along the page size asked for, against a query matching four
    // resources: one per page, two, more than the query matched, and the modelled
    // default of 0 (the maximum).
    let expression = "SELECT resourceId WHERE resourceId LIKE '%'";
    let cases: [(&str, i32, usize); 4] = [
        ("one result per page", 1, 1),
        ("two results per page", 2, 2),
        ("more than the query matched", 50, 5),
        ("the modelled default", 0, 5),
    ];

    let emulator = emulator_holding_inventory().await;
    for (case, limit, expected_page) in cases {
        let mut seen: Vec<Value> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = select(expression);
            request["Limit"] = json!(limit);
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let body = emulator.ok("SelectResourceConfig", &request).await;
            let page = results(case, &body);
            assert!(
                page.len() <= expected_page,
                "case {case:?}: a page carries at most {expected_page} results, got {page:?}"
            );
            seen.extend(page);
            next_token = body["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen.len(),
            5,
            "case {case:?}: paging answers every matched resource once, got {seen:?}"
        );
    }

    // A token that is not in the opaque form this operation issues cannot be read as a
    // page: it is refused rather than taken for a resume point. (A token the operation
    // DID issue is exercised by the paging loop above, which walks the whole result set
    // through the tokens it answered.)
    for (case, token) in [
        ("not base64 at all", "**not base64**"),
        (
            "a string outside the token codec's own base64 form",
            "cGFnZS1vbmU",
        ),
    ] {
        let mut request = select(expression);
        request["NextToken"] = json!(token);
        let (status, code) = emulator.refused("SelectResourceConfig", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: expected 400 InvalidNextTokenException, got {status} {code}"
        );
    }

    // A limit outside the modelled 0..100 range is refused: the request boundary
    // enforces the modelled range before the handler runs, and the handler answers its
    // own `InvalidLimitException` for a limit that reaches it, so either code is the
    // refusal this bound produces.
    for (case, limit) in [("one past the maximum", MAX_LIMIT + 1), ("a negative limit", -1)] {
        let mut request = select(expression);
        request["Limit"] = json!(limit);
        let (status, code) = emulator.refused("SelectResourceConfig", &request).await;
        assert!(
            status == 400 && (code == "InvalidLimitException" || code == "ValidationException"),
            "case {case:?}: a limit of {limit} is outside 0..100 and must be refused 400, got \
             {status} {code}"
        );
    }

    // The published maximum itself is INSIDE the range, so a request asking for it is
    // served, with a page holding every matched resource.
    let mut request = select(expression);
    request["Limit"] = json!(MAX_LIMIT);
    let body = emulator.ok("SelectResourceConfig", &request).await;
    assert_eq!(
        results("the published maximum", &body).len(),
        5,
        "case {:?}: a limit of {MAX_LIMIT} is the published maximum and carries every matched \
         resource in one page, got {body}",
        "the published maximum"
    );
    assert!(
        body["NextToken"].is_null(),
        "case {:?}: a page carrying every matched resource answers no next token, got {body}",
        "the published maximum"
    );
}

#[tokio::test]
async fn an_aggregate_is_computed_over_the_configuration_items_the_query_matched() {
    // Rows differ along WHICH function of the language is selected ([`AGGREGATES`],
    // every entry of the documented catalog — the issue's units cite COUNT alone), over a
    // numeric property of the recorded configuration document. A query that aggregates
    // without grouping has one group holding every matched item, so each row is one
    // result.
    let emulator = emulator_holding_inventory().await;
    for (case, field, expected) in AGGREGATES {
        let body = emulator
            .ok("SelectResourceConfig", &select(&format!("SELECT {field}")))
            .await;
        let rows = results(case, &body);
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: an ungrouped aggregate answers exactly one result, got {body}"
        );
        let actual = rows[0][field].as_f64();
        assert!(
            actual.is_some_and(|value| (value - expected).abs() < 1e-9),
            "case {case:?}: expected {field} over the recorded inventory to be {expected}, got \
             {:?}",
            rows[0]
        );

        // The field is reported under the function applied to the property, in
        // `QueryInfo` as well as in the result document.
        let fields: Vec<String> = body["QueryInfo"]["SelectFields"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected QueryInfo.SelectFields, got {body}"))
            .iter()
            .map(|selected| selected["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            fields,
            vec![field.to_string()],
            "case {case:?}: expected the selected field {field:?}, got {body}"
        );
    }

    // Grouped, the function computes over the items of ITS OWN group only, and a group
    // whose items carry no value for the property carries no aggregate at all — which is
    // a different answer from a group whose aggregate is zero.
    let body = emulator
        .ok(
            "SelectResourceConfig",
            &select("SELECT resourceType, MAX(configuration.size) GROUP BY resourceType"),
        )
        .await;
    assert_eq!(
        Value::Array(results("an aggregate per group", &body)),
        json!([
            { "resourceType": DYNAMODB_TABLE, "MAX(configuration.size)": 20 },
            { "resourceType": EC2_VOLUME, "MAX(configuration.size)": 500 },
            { "resourceType": LAMBDA_FUNCTION },
            { "resourceType": S3_BUCKET },
        ]),
        "case {:?}: expected the largest size of each resource type's own items, got {body}",
        "an aggregate per group"
    );

    // An aggregate over resources nothing matched is still one row: COUNT of no items is
    // zero, not an absent result.
    let body = emulator
        .ok(
            "SelectResourceConfig",
            &select(&format!(
                "SELECT COUNT(*) WHERE resourceType = '{RDS_INSTANCE}'"
            )),
        )
        .await;
    assert_eq!(
        Value::Array(results("a group of no items", &body)),
        json!([{ "COUNT(*)": 0 }]),
        "case {:?}: expected the count of nothing to be zero, got {body}",
        "a group of no items"
    );
}

#[tokio::test]
async fn a_limit_the_expression_carries_bounds_the_rows_the_query_answers() {
    // Rows differ along the bound the expression itself names, against a query matching
    // the five resources whose deletion is not recorded: no bound, fewer rows than
    // matched, exactly as many, more than matched, and no rows at all.
    let cases: [(&str, Option<usize>, usize); 5] = [
        ("no bound in the expression", None, 5),
        ("fewer rows than matched", Some(2), 2),
        ("exactly the rows matched", Some(5), 5),
        ("more rows than matched", Some(50), 5),
        ("no rows at all", Some(0), 0),
    ];

    let emulator = emulator_holding_inventory().await;
    for (case, limit, expected) in cases {
        let expression = match limit {
            None => "SELECT resourceId".to_string(),
            Some(rows) => format!("SELECT resourceId LIMIT {rows}"),
        };
        // The request asks for the largest page the model allows, so the only bound left
        // on the answer is the one the expression carries.
        let mut request = select(&expression);
        request["Limit"] = json!(MAX_LIMIT);
        let body = emulator.ok("SelectResourceConfig", &request).await;
        let rows = results(case, &body);
        assert_eq!(
            rows.len(),
            expected,
            "case {case:?}: expected {expression:?} to answer {expected} result(s), got {body}"
        );
        assert!(
            body["NextToken"].is_null(),
            "case {case:?}: every row the expression allows fits in this page, so no next token \
             is answered, got {body}"
        );
    }

    // The expression's bound holds ACROSS pages too: a request asking for one result per
    // page walks exactly the rows the expression allowed, never the whole match. The
    // walk is CAPPED rather than driven by the tokens alone: a page carries at least one
    // row whenever one remains, so a walk that takes more steps than the largest page
    // the model allows is a token that stopped advancing, which must fail the test
    // rather than spin it.
    let mut seen: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..=MAX_LIMIT {
        let mut request = select("SELECT resourceId LIMIT 3");
        request["Limit"] = json!(1);
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let body = emulator.ok("SelectResourceConfig", &request).await;
        seen.extend(results("paged under the expression's bound", &body));
        next_token = body["NextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }
    assert!(
        next_token.is_none(),
        "case {:?}: the walk must end at a page answering no next token, got {next_token:?}",
        "paged under the expression's bound"
    );
    assert_eq!(
        seen.len(),
        3,
        "case {:?}: paging answers the three rows the expression allowed, got {seen:?}",
        "paged under the expression's bound"
    );
}
