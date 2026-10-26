// SPDX-License-Identifier: CC0-1.0

//! What a Config query run BY one configuration aggregator answers, end to end over the
//! wire.
//!
//! The rules pinned here:
//!
//! 1. `SelectAggregateResourceConfig` runs the `Expression` over the resources the named
//!    aggregator takes in — the source accounts and Regions it aggregates, narrowed by
//!    its own resource-type filter — and answers the properties the query selected;
//! 2. the three ways the request can fail to BE such a query are told apart: an
//!    aggregator this account does not hold is `NoSuchConfigurationAggregatorException`,
//!    an expression that is not a Config query is `InvalidExpressionException`, and a
//!    `NextToken` the operation never issued is `InvalidNextTokenException` — none of
//!    them is answered as an empty result set, which is what a query that matched
//!    nothing answers;
//! 3. `MaxResults` and `Limit` both page the results, and a value outside the modelled
//!    range is refused.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds. The aggregated
//! resources are seeded in the store the way a handler reaches it (`service_state`, as
//! `select_resource_config` and `config_delete_configuration_aggregator` seed theirs):
//! the resource types an aggregate query selects are the modelled `ResourceType` enum,
//! while `PutResourceConfig` records CUSTOM resource types.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, DiscoveredResourceEntry,
};

use crate::config_setup_and_rule_loop::Emulator;

/// Entries of `com.amazonaws.configservice#ResourceType`, from
/// `smithy_query(query="shape", shape="ResourceType")`.
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`EC2_VOLUME`].
const DYNAMODB_TABLE: &str = "AWS::DynamoDB::Table";

/// The only published `AggregatorFilterType` value, from
/// `smithy_query(query="shape", shape="AggregatorFilterType")`: `INCLUDE`.
const FILTER_TYPE_INCLUDE: &str = "INCLUDE";

/// The aggregator over this account's own Config data.
const AGGREGATOR: &str = "org-wide";

/// A twelve-digit account id that is not this emulator's, the form `AccountId`'s
/// `^\d{12}$` pattern allows.
const ANOTHER_ACCOUNT: &str = "210987654321";

/// This emulator's account id — the one account whose Config data the store holds, and
/// so the only source any aggregator can take data in from.
fn account() -> String {
    emulator_server::emulator_account_id().to_string()
}

/// One resource the inventory holds, as this suite's rows name it.
struct Recorded {
    resource_type: &'static str,
    resource_id: &'static str,
    configuration: &'static str,
}

/// The inventory every query runs over.
fn inventory() -> Vec<Recorded> {
    vec![
        Recorded {
            resource_type: EC2_VOLUME,
            resource_id: "vol-0001",
            configuration: r#"{"state":"available","size":100}"#,
        },
        Recorded {
            resource_type: EC2_VOLUME,
            resource_id: "vol-0002",
            configuration: r#"{"state":"in-use","size":500}"#,
        },
        Recorded {
            resource_type: S3_BUCKET,
            resource_id: "reports",
            configuration: r#"{"state":"available"}"#,
        },
        Recorded {
            resource_type: DYNAMODB_TABLE,
            resource_id: "orders",
            configuration: r#"{"state":"ACTIVE"}"#,
        },
    ]
}

/// An emulator holding [`inventory`] and the aggregator `sources` under [`AGGREGATOR`].
async fn emulator_aggregating(sources: Value) -> Emulator {
    let emulator = Emulator::new();
    let mut request = sources;
    request["ConfigurationAggregatorName"] = json!(AGGREGATOR);
    emulator
        .ok("PutConfigurationAggregator", &request)
        .await;

    let captured_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
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
                    deleted_at: None,
                    schema_version_id: Some("00000000-0000-0000-0000-000000000001".to_string()),
                    configuration: Some(resource.configuration.to_string()),
                    first_recorded_at: Some(captured_at),
                    last_recorded_at: Some(captured_at),
                    ..DiscoveredResourceEntry::default()
                },
            );
        }
    }
    emulator
}

/// An aggregator over this account, every Region.
fn this_account_everywhere() -> Value {
    json!({
        "AccountAggregationSources": [{ "AccountIds": [account()], "AllAwsRegions": true }],
    })
}

/// The `SelectAggregateResourceConfig` body running `expression` under `aggregator`.
fn select(aggregator: &str, expression: &str) -> Value {
    json!({
        "ConfigurationAggregatorName": aggregator,
        "Expression": expression,
    })
}

/// The results of a response, each parsed back out of the JSON document it is.
fn results(case: &str, body: &Value) -> Vec<Value> {
    body["Results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Results, got {body}"))
        .iter()
        .map(|result| {
            let text = result.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: a result is a JSON document, got {body}")
            });
            serde_json::from_str::<Value>(text)
                .unwrap_or_else(|_| panic!("case {case:?}: a result parses as JSON, got {text:?}"))
        })
        .collect()
}

#[tokio::test]
async fn an_aggregate_query_answers_the_properties_it_selected_of_the_resources_aggregated() {
    // Rows differ along what the rule generalises over: how many properties the query
    // selects, which operator its condition uses, whether it selects a nested property,
    // whether it groups and aggregates, whether it orders, and whether it matches many
    // resources, one or none.
    let cases: [(&str, String, Value); 7] = [
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
            "the account every aggregated resource belongs to",
            format!("SELECT accountId WHERE resourceType = '{S3_BUCKET}'"),
            json!([{ "accountId": account() }]),
        ),
        (
            "a nested property, selected and filtered on",
            "SELECT resourceId, configuration.state WHERE configuration.state = 'in-use'"
                .to_string(),
            json!([{ "resourceId": "vol-0002", "configuration": { "state": "in-use" } }]),
        ),
        (
            "a count per group",
            "SELECT resourceType, COUNT(*) GROUP BY resourceType".to_string(),
            json!([
                { "resourceType": DYNAMODB_TABLE, "COUNT(*)": 1 },
                { "resourceType": EC2_VOLUME, "COUNT(*)": 2 },
                { "resourceType": S3_BUCKET, "COUNT(*)": 1 },
            ]),
        ),
        (
            "an ordering the query asks for",
            format!(
                "SELECT resourceId WHERE resourceType = '{EC2_VOLUME}' ORDER BY resourceId DESC"
            ),
            json!([{ "resourceId": "vol-0002" }, { "resourceId": "vol-0001" }]),
        ),
        (
            "a query that matched nothing",
            "SELECT resourceId WHERE resourceId = 'vol-9999'".to_string(),
            json!([]),
        ),
    ];

    let emulator = emulator_aggregating(this_account_everywhere()).await;
    for (case, expression, expected) in cases {
        let body = emulator
            .ok("SelectAggregateResourceConfig", &select(AGGREGATOR, &expression))
            .await;
        assert_eq!(
            Value::Array(results(case, &body)),
            expected,
            "case {case:?}: expected {expression:?} to answer {expected}, got {body}"
        );

        // The query is a report: running it again must answer the same thing.
        let again = emulator
            .ok("SelectAggregateResourceConfig", &select(AGGREGATOR, &expression))
            .await;
        assert_eq!(
            again, body,
            "case {case:?}: a second run of the same query must answer the same, got {again}"
        );
    }

    // "Details about the query": the fields it selected, in the order it selected them.
    let body = emulator
        .ok(
            "SelectAggregateResourceConfig",
            &select(AGGREGATOR, "SELECT resourceId, resourceType, awsRegion"),
        )
        .await;
    let fields: Vec<String> = body["QueryInfo"]["SelectFields"]
        .as_array()
        .unwrap_or_else(|| panic!("expected QueryInfo.SelectFields, got {body}"))
        .iter()
        .map(|field| field["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        fields,
        vec![
            "resourceId".to_string(),
            "resourceType".to_string(),
            "awsRegion".to_string(),
        ],
        "expected the selected fields to be reported in the order selected, got {body}"
    );
}

#[tokio::test]
async fn a_query_searches_only_what_its_own_aggregator_takes_in() {
    // Rows differ along the SCOPE the aggregator aggregates: this account everywhere,
    // this account in the Region the request is signed for, this account with a
    // resource-type filter, another account, and this account in another Region only.
    let cases: [(&str, Value, Vec<&str>); 5] = [
        (
            "this account, every Region",
            this_account_everywhere(),
            vec!["orders", "reports", "vol-0001", "vol-0002"],
        ),
        (
            "this account, the Region the request is signed for",
            json!({
                "AccountAggregationSources": [{
                    "AccountIds": [account()],
                    "AwsRegions": ["us-east-1"],
                }],
            }),
            vec!["orders", "reports", "vol-0001", "vol-0002"],
        ),
        (
            "this account, filtered to one resource type",
            json!({
                "AccountAggregationSources": [{
                    "AccountIds": [account()],
                    "AllAwsRegions": true,
                }],
                "AggregatorFilters": {
                    "ResourceType": { "Type": FILTER_TYPE_INCLUDE, "Value": [EC2_VOLUME] },
                },
            }),
            vec!["vol-0001", "vol-0002"],
        ),
        (
            "another account only",
            json!({
                "AccountAggregationSources": [{
                    "AccountIds": [ANOTHER_ACCOUNT],
                    "AllAwsRegions": true,
                }],
            }),
            vec![],
        ),
        (
            "this account in another Region only",
            json!({
                "AccountAggregationSources": [{
                    "AccountIds": [account()],
                    "AwsRegions": ["eu-central-1"],
                }],
            }),
            vec![],
        ),
    ];

    for (case, sources, expected) in cases {
        let emulator = emulator_aggregating(sources).await;
        let body = emulator
            .ok(
                "SelectAggregateResourceConfig",
                &select(AGGREGATOR, "SELECT resourceId ORDER BY resourceId"),
            )
            .await;
        let answered: Vec<String> = results(case, &body)
            .iter()
            .map(|row| row["resourceId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            answered,
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the aggregator to answer {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn an_aggregator_this_account_does_not_hold_is_refused_rather_than_answered_empty() {
    let emulator = emulator_aggregating(this_account_everywhere()).await;

    // Rows differ along HOW the name fails to be one of this account's aggregators: a
    // name nothing was ever created under, a name differing only in case, a name that is
    // a prefix of the real one, and the name of an aggregator that has been deleted.
    emulator
        .ok(
            "PutConfigurationAggregator",
            &json!({
                "ConfigurationAggregatorName": "to-be-deleted",
                "AccountAggregationSources": [{ "AccountIds": [account()], "AllAwsRegions": true }],
            }),
        )
        .await;
    emulator
        .ok(
            "DeleteConfigurationAggregator",
            &json!({ "ConfigurationAggregatorName": "to-be-deleted" }),
        )
        .await;

    for (case, name) in [
        ("a name nothing was created under", "never-created"),
        ("a name differing only in case", "ORG-WIDE"),
        ("a prefix of the real name", "org"),
        ("a deleted aggregator", "to-be-deleted"),
    ] {
        let (status, parsed) = emulator
            .call(
                "SelectAggregateResourceConfig",
                &select(name, "SELECT resourceId"),
            )
            .await;
        let code = parsed["__type"]
            .as_str()
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
            .unwrap_or("<none>");
        assert_eq!(
            (status, code),
            (400, "NoSuchConfigurationAggregatorException"),
            "case {case:?}: expected 400 NoSuchConfigurationAggregatorException for {name:?}, \
             got {status} {parsed}"
        );
    }

    // The aggregator that DOES exist answers the same query, so the refusals above are
    // about the name and not about the query.
    let body = emulator
        .ok(
            "SelectAggregateResourceConfig",
            &select(AGGREGATOR, "SELECT resourceId"),
        )
        .await;
    assert_eq!(
        results("the aggregator this account holds", &body).len(),
        inventory().len(),
        "the aggregator this account holds answers every resource it aggregates, got {body}"
    );
}

#[tokio::test]
async fn an_expression_that_is_not_a_config_query_is_refused() {
    let emulator = emulator_aggregating(this_account_everywhere()).await;

    // Rows differ along HOW the expression fails to be a Config query: a missing
    // projection, a misspelled keyword, a truncated condition, a clause the documented
    // query form has no place for, more than one statement, and a statement that is not
    // a query at all.
    for (case, expression) in [
        ("the recorded malformed expression", "SELECT WHERE"),
        ("a misspelled SELECT", "SELEC resourceId"),
        ("a condition with no value", "SELECT resourceId WHERE resourceType ="),
        ("a FROM clause", "SELECT resourceId FROM aggregate_config"),
        ("a subquery in the condition", "SELECT resourceId WHERE resourceId IN (SELECT resourceId)"),
        ("two statements", "SELECT resourceId; SELECT resourceType"),
        ("a statement that is not a query", "DELETE FROM resources"),
    ] {
        let (status, code) = emulator
            .refused(
                "SelectAggregateResourceConfig",
                &select(AGGREGATOR, expression),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidExpressionException"),
            "case {case:?}: {expression:?} is not a Config query and must be refused 400 \
             InvalidExpressionException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_request_that_asks_for_no_page_size_gets_the_one_the_operation_documents() {
    // The operation documents two default page sizes — 25 for a non-aggregation query
    // and 500 for an aggregation query — so a request that carries neither `MaxResults`
    // nor `Limit` is paged at the one its query calls for. The inventory is grown past
    // the smaller default so the boundary is observable.
    let emulator = emulator_aggregating(this_account_everywhere()).await;
    let captured_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    {
        let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
        let mut store = state.lock();
        for index in 0..30 {
            let resource_id = format!("vol-{index:04}");
            store.discovered_resources.insert(
                (EC2_VOLUME.to_string(), resource_id.clone()),
                DiscoveredResourceEntry {
                    resource_type: EC2_VOLUME.to_string(),
                    resource_id: resource_id.clone(),
                    resource_name: Some(resource_id),
                    schema_version_id: Some("00000000-0000-0000-0000-000000000001".to_string()),
                    configuration: Some(r#"{"state":"available"}"#.to_string()),
                    first_recorded_at: Some(captured_at),
                    last_recorded_at: Some(captured_at),
                    ..DiscoveredResourceEntry::default()
                },
            );
        }
    }

    // A non-aggregation query: 25 rows, and a token for the rest.
    let body = emulator
        .ok(
            "SelectAggregateResourceConfig",
            &select(AGGREGATOR, "SELECT resourceId ORDER BY resourceId"),
        )
        .await;
    assert_eq!(
        results("a non-aggregation query", &body).len(),
        25,
        "a non-aggregation query that asks for no page size is paged at 25, got {body}"
    );
    assert!(
        body["NextToken"].is_string(),
        "the rest of the results are behind a token, got {body}"
    );

    // An aggregation query: one row per resource type, all of them in one page, because
    // the default page size for an aggregation query is far above what this store holds.
    let body = emulator
        .ok(
            "SelectAggregateResourceConfig",
            &select(
                AGGREGATOR,
                "SELECT resourceType, COUNT(*) GROUP BY resourceType",
            ),
        )
        .await;
    assert_eq!(
        results("an aggregation query", &body).len(),
        3,
        "an aggregation query answers one row per group in one page, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "an aggregation query that fits in one page answers no token, got {body}"
    );
}

#[tokio::test]
async fn an_aggregate_query_pages_at_the_size_the_request_asks_for() {
    let emulator = emulator_aggregating(this_account_everywhere()).await;
    let expression = "SELECT resourceId ORDER BY resourceId";

    // Rows differ along WHICH member asks for the page size and what it asks for: the
    // member the operation documents (`MaxResults`), the member it "also allows"
    // (`Limit`), both at once, more than the query matched, and neither (the page size
    // the operation documents for a non-aggregation query, which is above what this
    // inventory holds).
    let cases: [(&str, Option<i32>, Option<i32>, usize); 6] = [
        ("one result per page by MaxResults", Some(1), None, 1),
        ("two results per page by MaxResults", Some(2), None, 2),
        ("one result per page by Limit", None, Some(1), 1),
        ("MaxResults wins over Limit", Some(1), Some(4), 1),
        ("more than the query matched", Some(50), None, 4),
        ("neither member", None, None, 4),
    ];

    for (case, max_results, limit, expected_page) in cases {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = select(AGGREGATOR, expression);
            if let Some(max_results) = max_results {
                request["MaxResults"] = json!(max_results);
            }
            if let Some(limit) = limit {
                request["Limit"] = json!(limit);
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let body = emulator
                .ok("SelectAggregateResourceConfig", &request)
                .await;
            let page = results(case, &body);
            assert!(
                page.len() <= expected_page,
                "case {case:?}: a page carries at most {expected_page} results, got {page:?}"
            );
            seen.extend(
                page.iter()
                    .map(|row| row["resourceId"].as_str().unwrap_or_default().to_string()),
            );
            next_token = body["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen,
            vec![
                "orders".to_string(),
                "reports".to_string(),
                "vol-0001".to_string(),
                "vol-0002".to_string(),
            ],
            "case {case:?}: paging answers every aggregated resource once, in order"
        );
    }

    // A token that is not in the opaque form this operation issues cannot be read as a
    // page: it is refused rather than taken for a resume point.
    for (case, token) in [
        ("not base64 at all", "**not base64**"),
        ("a string outside the token codec's own base64 form", "cGFnZS1vbmU"),
    ] {
        let mut request = select(AGGREGATOR, expression);
        request["NextToken"] = json!(token);
        let (status, code) = emulator
            .refused("SelectAggregateResourceConfig", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: expected 400 InvalidNextTokenException, got {status} {code}"
        );
    }

    // A page size outside the modelled 0..100 range is refused: the request boundary
    // enforces the modelled range before the handler runs, and the handler answers its
    // own `InvalidLimitException` for a value that reaches it, so either code is the
    // refusal this bound produces — for either member that carries it.
    for (case, member, value) in [
        ("MaxResults one past the maximum", "MaxResults", 101),
        ("a negative MaxResults", "MaxResults", -1),
        ("Limit one past the maximum", "Limit", 101),
        ("a negative Limit", "Limit", -1),
    ] {
        let mut request = select(AGGREGATOR, expression);
        request[member] = json!(value);
        let (status, code) = emulator
            .refused("SelectAggregateResourceConfig", &request)
            .await;
        assert!(
            status == 400 && (code == "InvalidLimitException" || code == "ValidationException"),
            "case {case:?}: {member} of {value} is outside 0..100 and must be refused 400, got \
             {status} {code}"
        );
    }
}
