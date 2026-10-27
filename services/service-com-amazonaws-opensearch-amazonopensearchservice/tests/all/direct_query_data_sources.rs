// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OpenSearch Service direct query data sources of an
//! ACCOUNT: `AddDirectQueryDataSource` and `GetDirectQueryDataSource`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the tagging helpers from
//! [`super::resource_tags`]; this suite adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * an add persists the data source under the name that addresses it and answers
//!   with the identifier it minted, and a read of that name reports the stored record
//!   member for member — over a table of adds differing in the union member they
//!   carry, in the name's length and character classes, in whether a description, an
//!   access policy, OpenSearch collections or tags were supplied, and in how many
//!   collections they named;
//! * an access policy document "must be in valid JSON format", so one that is not JSON
//!   is refused and one that is is reported as the document it denotes: the tokens the
//!   caller wrote, with the insignificant whitespace between them dropped and every
//!   value — member order, the digits of a number, the content of a string — unchanged,
//!   over a table of documents whose integers are past what a double holds exactly,
//!   whose decimals go past a double's precision, whose timestamps carry sub-second
//!   digits, and which carry non-ASCII text, escaped quotes, whitespace inside strings
//!   and nothing at all;
//! * the name is unique within the account, so a second add of one it already holds is
//!   refused and the first record stands;
//! * a name the account holds no data source under is refused as the value it is: the
//!   modelled `ValidationException`, in the words the service states that condition in;
//! * `OpenSearchArns` is required for exactly the two union members the model says it
//!   is required for, and optional for the third;
//! * a value that is not an ARN at all is refused as a malformed request and nothing is
//!   persisted;
//! * the store is bounded: adds are accepted up to the quota and the one past it is
//!   refused with the modelled `LimitExceededException`, leaving the stored set at the
//!   quota;
//! * the tags an add carries are the resource's own, readable and extendable through
//!   the tagging operations under the ARN the add minted.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    DIRECT_QUERY_DATA_SOURCE_QUOTA, TAG_QUOTA,
};

use super::domain_lifecycle::{error_code, string_member, Answer, Emulator, PREFIX, REGION};
use super::resource_tags::{add_tags, as_set, error_message, stored_tags, tag, wire_tags};

/// `AddDirectQueryDataSource`: `POST /2021-01-01/opensearch/directQueryDataSource`.
async fn add(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/directQueryDataSource"),
            Some(body),
        )
        .await
}

/// `GetDirectQueryDataSource`:
/// `GET /2021-01-01/opensearch/directQueryDataSource/{DataSourceName}`.
async fn get(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/directQueryDataSource/{name}"),
            None,
        )
        .await
}

/// The ARN a data source of this account is addressed by, composed here from the
/// partition, this service's namespace, the Region the request was signed for, the
/// account this emulator serves and the name the add carried — so the assertion does
/// not ask the service to confirm its own answer.
fn expected_arn(name: &str) -> String {
    format!(
        "arn:aws:es:{REGION}:{}:datasource/{name}",
        engine::emulator_account_id()
    )
}

/// An IAM role a data source's union member names, inside
/// `DirectQueryDataSourceRoleArn`'s `@length(min: 32, max: 200)` and its pattern
/// `^arn:aws[a-zA-Z-]*:iam::\d{12}:role(\/service-role)?\/[A-Za-z0-9+=,.@\-_]{1,64}$`
/// (`smithy_query(query="shape",shape="CloudWatchDirectQueryDataSource")`).
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/OpenSearchDirectQueryRole";

/// The same shape of role ARN through the `service-role` path the pattern also allows.
const SERVICE_ROLE_ARN: &str =
    "arn:aws:iam::123456789012:role/service-role/OpenSearchDirectQuery_1";

/// An Amazon Managed Prometheus workspace, inside `AMPWorkspaceArn`'s pattern
/// `^arn:aws[a-zA-Z-]*:aps:[a-z0-9-]+:[0-9]{12}:workspace\/ws-[a-z0-9-]*$`
/// (`smithy_query(query="shape",shape="PrometheusDirectQueryDataSource")`).
const WORKSPACE_ARN: &str =
    "arn:aws:aps:us-east-1:123456789012:workspace/ws-0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9";

/// One OpenSearch collection a data source may be associated with: an `ARN`, which is
/// `@length(min: 20, max: 2048)` with pattern `.*`.
const COLLECTION_ARN: &str = "arn:aws:es:us-east-1:123456789012:domain/search-logs";

/// A second collection, so a row can name more than one.
const SECOND_COLLECTION_ARN: &str = "arn:aws:es:us-east-1:123456789012:domain/search-events";

/// One add to make, and therefore one record to read back.
struct Row {
    /// What the case is called when an assertion fails.
    case: &'static str,
    /// The `DataSourceName` the add carries, which is also the name the read uses.
    name: String,
    /// The `DataSourceType` union, as the wire carries it.
    data_source_type: Value,
    /// The `Description` to send, absent where `None`.
    description: Option<String>,
    /// The `OpenSearchArns` to send, absent where `None`.
    open_search_arns: Option<Vec<String>>,
    /// The `DataSourceAccessPolicy` to send, absent where `None`.
    policy: Option<String>,
    /// The document a read reports for that policy: the same document with no
    /// insignificant whitespace, and none at all where the add named no document.
    stored_policy: Option<String>,
    /// The `TagList` to send, empty where the add names no tags.
    tags: Vec<(String, String)>,
}

impl Row {
    /// The request body this row sends: every member it names and no member it does
    /// not, so an absent member is absent on the wire rather than sent as null.
    fn body(&self) -> Value {
        let mut body = json!({
            "DataSourceName": self.name,
            "DataSourceType": self.data_source_type,
        });
        if let Some(description) = &self.description {
            body["Description"] = json!(description);
        }
        if let Some(arns) = &self.open_search_arns {
            body["OpenSearchArns"] = json!(arns);
        }
        if let Some(policy) = &self.policy {
            body["DataSourceAccessPolicy"] = json!(policy);
        }
        if !self.tags.is_empty() {
            body["TagList"] = wire_tags(&self.tags);
        }
        body
    }
}

/// The CloudWatch Logs union member, which names an IAM role only.
fn cloud_watch(role_arn: &str) -> Value {
    json!({ "CloudWatchLog": { "RoleArn": role_arn } })
}

/// The Security Lake union member, the same shape as [`cloud_watch`]'s.
fn security_lake(role_arn: &str) -> Value {
    json!({ "SecurityLake": { "RoleArn": role_arn } })
}

/// The Prometheus union member, which names an IAM role and a workspace.
fn prometheus(role_arn: &str, workspace_arn: &str) -> Value {
    json!({ "Prometheus": { "RoleArn": role_arn, "WorkspaceArn": workspace_arn } })
}

/// An access policy document carrying an integer past the 2^53 a double holds exactly
/// and a timestamp with nanosecond digits, written with no whitespace between its
/// tokens, so what a read reports for it is the document itself.
const PRECISE_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::123456789012:root"},"#,
    r#""Action":"es:ESHttpGet","Resource":"*","Condition":{"#,
    r#""NumericEquals":{"es:QueryBudget":9007199254740993},"#,
    r#""NumericLessThan":{"es:QueryCost":0.100000000000000005},"#,
    r#""DateGreaterThan":{"aws:CurrentTime":"2026-10-24T05:54:21.123456789Z"}}}]}"#
);

/// The same document written the way a client formats one: whitespace between the
/// tokens, which denotes nothing.
const SPACED_POLICY: &str = concat!(
    "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [\n    {\n",
    "      \"Effect\": \"Allow\",\n",
    "      \"Action\": [ \"es:ESHttpGet\" ],\n",
    "      \"Resource\": \"*\",\n",
    "      \"Condition\": { \"NumericEquals\": { \"es:QueryBudget\": 9007199254740993 } }\n",
    "    }\n  ]\n}"
);

/// [`SPACED_POLICY`] as the service holds it: the same tokens in the same order, with
/// the whitespace between them dropped and the digits of its integer untouched.
const SPACED_POLICY_AS_STORED: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Action":["es:ESHttpGet"],"Resource":"*","#,
    r#""Condition":{"NumericEquals":{"es:QueryBudget":9007199254740993}}}]}"#
);

/// The add table: one row per way an accepted add differs from another — which member
/// of the union it carries, the name at each published length bound and spanning the
/// character classes its pattern allows, a description absent / empty / at its longest,
/// no collections / one / two, an access policy present or not, and tags present or
/// not.
///
/// `DirectQueryDataSourceName` is `@length(min: 3, max: 80)` with pattern
/// `^[a-z][a-z0-9_]+$`, `DirectQueryDataSourceDescription` is `@length(min: 0, max:
/// 1000)`, `PolicyDocument` is `@length(min: 0, max: 102400)` and `OpenSearchArns` is
/// optional — all from this service's Smithy model
/// (`smithy_query(query="operation",operation="AddDirectQueryDataSource")`), so every
/// row is inside the modelled constraints and the service itself is what accepts it.
fn add_rows() -> Vec<Row> {
    vec![
        // The shortest name the constraint allows (3), the CloudWatch Logs member, one
        // collection, and nothing optional at all.
        Row {
            case: "the shortest name, no description, one collection",
            name: "cwl".to_string(),
            data_source_type: cloud_watch(ROLE_ARN),
            description: None,
            open_search_arns: Some(vec![COLLECTION_ARN.to_string()]),
            policy: None,
            stored_policy: None,
            tags: Vec::new(),
        },
        // The longest name the constraint allows (80), Security Lake, two collections,
        // the longest description (1000) and an access policy.
        Row {
            case: "the longest name, two collections, the longest description, a policy",
            name: format!("security_lake_{}", "x".repeat(66)),
            data_source_type: security_lake(ROLE_ARN),
            description: Some("d".repeat(1000)),
            open_search_arns: Some(vec![
                COLLECTION_ARN.to_string(),
                SECOND_COLLECTION_ARN.to_string(),
            ]),
            policy: Some(PRECISE_POLICY.to_string()),
            stored_policy: Some(PRECISE_POLICY.to_string()),
            tags: Vec::new(),
        },
        // Prometheus, the one member that asks for no collection at all, with a
        // workspace beside its role.
        Row {
            case: "prometheus naming no collection",
            name: "prometheus_workspace".to_string(),
            data_source_type: prometheus(ROLE_ARN, WORKSPACE_ARN),
            description: Some("Metrics read through the shared workspace.".to_string()),
            open_search_arns: None,
            policy: None,
            stored_policy: None,
            tags: Vec::new(),
        },
        // A name spanning the digits and underscores its pattern allows, a role ARN
        // through the `service-role` path, a policy document written with whitespace
        // between its tokens, and tags.
        Row {
            case: "digits and underscores in the name, a service-role role, tags",
            name: "a0_source_9_log".to_string(),
            data_source_type: cloud_watch(SERVICE_ROLE_ARN),
            description: Some("Logs 2026-10-24 @ 50% sampling: ok/ok!".to_string()),
            open_search_arns: Some(vec![SECOND_COLLECTION_ARN.to_string()]),
            policy: Some(SPACED_POLICY.to_string()),
            stored_policy: Some(SPACED_POLICY_AS_STORED.to_string()),
            tags: vec![tag("project", "trinity"), tag("env", "prod")],
        },
        // The empty description the constraint allows (0), and the blank policy document
        // `PolicyDocument`'s own minimum allows, which names no policy at all.
        Row {
            case: "an empty description and a blank policy document",
            name: "security_lake_empty".to_string(),
            data_source_type: security_lake(SERVICE_ROLE_ARN),
            description: Some(String::new()),
            open_search_arns: Some(vec![COLLECTION_ARN.to_string()]),
            policy: Some(String::new()),
            stored_policy: None,
            tags: vec![tag("k", "")],
        },
    ]
}

/// A member of a read answer that the row supplied, or `None` where the answer carries
/// none.
fn member(answer: &Answer, name: &str) -> Option<String> {
    answer.body[name].as_str().map(str::to_owned)
}

/// The `OpenSearchArns` a read answer reports, or `None` where it carries none.
fn reported_arns(answer: &Answer) -> Option<Vec<String>> {
    answer.body["OpenSearchArns"].as_array().map(|arns| {
        arns.iter()
            .map(|arn| arn.as_str().unwrap_or_default().to_owned())
            .collect()
    })
}

/// Add `row` and assert the add was served, handing back the `DataSourceArn` it
/// answered with.
async fn added(emulator: &Emulator, row: &Row) -> String {
    let answer = add(emulator, row.body()).await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: expected 200 from AddDirectQueryDataSource, got {} with {:?}",
        row.case, answer.status, answer.body
    );
    string_member(&answer.body, "DataSourceArn", row.case)
}

/// RULE: an add persists the data source under the name that addresses it and answers
/// with the identifier it minted; a read of that name reports the stored record member
/// for member.
#[tokio::test]
async fn an_added_data_source_is_persisted_and_read_back_member_for_member() {
    let emulator = Emulator::new();
    for row in add_rows() {
        let minted = added(&emulator, &row).await;
        assert_eq!(
            minted,
            expected_arn(&row.name),
            "case {:?}: expected the minted ARN to name this account's data source, got {minted:?}",
            row.case
        );

        let read = get(&emulator, &row.name).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetDirectQueryDataSource, got {} with {:?}",
            row.case, read.status, read.body
        );
        assert_eq!(
            member(&read, "DataSourceName"),
            Some(row.name.clone()),
            "case {:?}: expected the stored name, got {:?}",
            row.case,
            member(&read, "DataSourceName")
        );
        assert_eq!(
            member(&read, "DataSourceArn"),
            Some(minted.clone()),
            "case {:?}: expected the read to report the minted ARN, got {:?}",
            row.case,
            member(&read, "DataSourceArn")
        );
        assert_eq!(
            read.body["DataSourceType"], row.data_source_type,
            "case {:?}: expected the stored union member, got {:?}",
            row.case, read.body["DataSourceType"]
        );
        assert_eq!(
            member(&read, "Description"),
            row.description,
            "case {:?}: expected the stored description, got {:?}",
            row.case,
            member(&read, "Description")
        );
        assert_eq!(
            member(&read, "DataSourceAccessPolicy"),
            row.stored_policy,
            "case {:?}: expected the stored access policy, got {:?}",
            row.case,
            member(&read, "DataSourceAccessPolicy")
        );
        assert_eq!(
            reported_arns(&read),
            row.open_search_arns,
            "case {:?}: expected the stored collections, got {:?}",
            row.case,
            reported_arns(&read)
        );

        // A read changes nothing about the record, so a second one answers identically.
        let again = get(&emulator, &row.name).await;
        assert_eq!(
            again.body, read.body,
            "case {:?}: expected a second read to report the same record, got {:?}",
            row.case, again.body
        );

        // The tags the add carried are the resource's own, under the ARN it minted.
        assert_eq!(
            stored_tags(&emulator, &minted, row.case).await,
            as_set(&row.tags),
            "case {:?}: expected exactly the tags the add carried",
            row.case
        );
    }
}

/// RULE: the values of an access policy document are the caller's own, so what a read
/// reports is the document it denotes: every token and every value as they were
/// written, in the order they were written, with only the insignificant whitespace
/// BETWEEN the tokens dropped.
#[tokio::test]
async fn an_access_policy_document_is_reported_as_the_document_it_denotes() {
    let emulator = Emulator::new();
    // (case, the document as the add sends it, the document a read reports for it)
    let rows: Vec<(&str, String, Option<String>)> = vec![
        (
            "an integer past what a double holds exactly, and nanosecond digits",
            PRECISE_POLICY.to_string(),
            Some(PRECISE_POLICY.to_string()),
        ),
        (
            "the same document written with whitespace between its tokens",
            SPACED_POLICY.to_string(),
            Some(SPACED_POLICY_AS_STORED.to_string()),
        ),
        (
            "integers at the edges of i64",
            r#"{"Budget":9223372036854775807,"Floor":-9223372036854775808}"#.to_string(),
            Some(r#"{"Budget":9223372036854775807,"Floor":-9223372036854775808}"#.to_string()),
        ),
        (
            "decimals that go past a double's precision, and an exponent",
            r#"{ "Rate": 0.1234567890123456789, "Scale": 1e308 }"#.to_string(),
            Some(r#"{"Rate":0.1234567890123456789,"Scale":1e308}"#.to_string()),
        ),
        (
            "timestamps with a sub-second part and an offset",
            r#"{ "From": "2026-10-24T05:54:21.000000001+00:00", "To": "2026-10-24T05:54:21Z" }"#
                .to_string(),
            Some(
                r#"{"From":"2026-10-24T05:54:21.000000001+00:00","To":"2026-10-24T05:54:21Z"}"#
                    .to_string(),
            ),
        ),
        (
            "member order a re-render through a sorted map would not keep",
            "{\n  \"Statement\" : [ ],\n  \"Version\" : \"2012-10-17\"\n}".to_string(),
            Some(r#"{"Statement":[],"Version":"2012-10-17"}"#.to_string()),
        ),
        (
            "non-ASCII text",
            r#"{ "Sid": "données-de-journal-日本語" }"#.to_string(),
            Some(r#"{"Sid":"données-de-journal-日本語"}"#.to_string()),
        ),
        (
            "whitespace, an escaped quote and braces INSIDE a string",
            "{ \"Sid\": \"a quote \\\" and a brace { } kept as written\" }".to_string(),
            Some("{\"Sid\":\"a quote \\\" and a brace { } kept as written\"}".to_string()),
        ),
        (
            // A string literal that CLOSES after an escaped quote, with further members
            // behind it: the whitespace between those later tokens is between tokens, so
            // it goes, and an escaped quote is legal in any statement id or condition
            // value.
            "members behind a string that holds an escaped quote",
            "{ \"a\" : \"x\\\"y\" , \"b\" : 1 }".to_string(),
            Some("{\"a\":\"x\\\"y\",\"b\":1}".to_string()),
        ),
        (
            "a document at a thousand characters",
            format!(r#"{{"Sid":"{}"}}"#, "s".repeat(1000)),
            Some(format!(r#"{{"Sid":"{}"}}"#, "s".repeat(1000))),
        ),
        (
            "the blank document the constraint allows, which names no policy",
            String::new(),
            None,
        ),
        (
            "a document of whitespace only, which names no policy either",
            "   \n".to_string(),
            None,
        ),
    ];
    for (index, (case, policy, stored)) in rows.iter().enumerate() {
        let name = format!("policy_round_trip_{index}");
        let answer = add(
            &emulator,
            json!({
                "DataSourceName": name,
                "DataSourceType": cloud_watch(ROLE_ARN),
                "OpenSearchArns": [COLLECTION_ARN],
                "DataSourceAccessPolicy": policy,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from AddDirectQueryDataSource, got {} with {:?}",
            answer.status, answer.body
        );

        let read = get(&emulator, &name).await;
        assert_eq!(
            member(&read, "DataSourceAccessPolicy"),
            *stored,
            "case {case:?}: expected the document the service holds, got {:?}",
            member(&read, "DataSourceAccessPolicy")
        );
    }
}

/// RULE: an access policy document "must be in valid JSON format", so a text that is
/// not JSON is a malformed request and nothing is persisted.
#[tokio::test]
async fn a_policy_document_that_is_not_json_is_refused() {
    let emulator = Emulator::new();
    // (case, the text sent as the document)
    let rows: &[(&str, &str)] = &[
        ("text that is not JSON at all", "allow everything"),
        (
            "an object that is never closed",
            r#"{"Version":"2012-10-17""#,
        ),
        ("an array that is never closed", r#"[{"Effect":"Allow"}"#),
        (
            "a trailing comma no JSON parser accepts",
            r#"{"Effect":"Allow",}"#,
        ),
        ("single quotes around a member name", "{'Effect':'Allow'}"),
        ("two documents in one text", r#"{"a":1}{"b":2}"#),
    ];
    for (index, (case, policy)) in rows.iter().enumerate() {
        let name = format!("malformed_policy_{index}");
        let answer = add(
            &emulator,
            json!({
                "DataSourceName": name,
                "DataSourceType": cloud_watch(ROLE_ARN),
                "OpenSearchArns": [COLLECTION_ARN],
                "DataSourceAccessPolicy": policy,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a document that is not JSON, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected the modelled refusal, got {:?}",
            error_code(&answer)
        );
        let read = get(&emulator, &name).await;
        assert_eq!(
            read.status, 400,
            "case {case:?}: expected the refused add to persist nothing, got {} with {:?}",
            read.status, read.body
        );
    }
}

/// RULE: the name identifies the data source within the account, so a second add of a
/// name it already holds is refused and the record the first add made stands.
#[tokio::test]
async fn a_second_add_of_a_held_name_is_refused_and_the_first_record_stands() {
    let emulator = Emulator::new();
    for row in add_rows() {
        added(&emulator, &row).await;
        let first = get(&emulator, &row.name).await;

        // The second add asks for a DIFFERENT record under the same name: another union
        // member, another description, another collection.
        let again = add(
            &emulator,
            json!({
                "DataSourceName": row.name,
                "DataSourceType": prometheus(ROLE_ARN, WORKSPACE_ARN),
                "Description": "a second record under one name",
                "OpenSearchArns": [SECOND_COLLECTION_ARN],
            }),
        )
        .await;
        assert_eq!(
            again.status, 400,
            "case {:?}: expected 400 for a name the account holds, got {} with {:?}",
            row.case, again.status, again.body
        );
        assert_eq!(
            error_code(&again),
            "ValidationException",
            "case {:?}: expected the modelled refusal, got {:?}",
            row.case,
            error_code(&again)
        );

        let after = get(&emulator, &row.name).await;
        assert_eq!(
            after.body, first.body,
            "case {:?}: expected the refused add to leave the record as it was, got {:?}",
            row.case, after.body
        );
    }
}

/// RULE: a name the account holds no data source under names nothing to report, which
/// is refused as the value it is: the modelled `ValidationException`.
#[tokio::test]
async fn a_name_that_names_no_data_source_is_refused() {
    let emulator = Emulator::new();
    let held = add_rows().remove(0);
    added(&emulator, &held).await;

    // (case, the name to read)
    let rows: &[(&str, &str)] = &[
        ("a name nothing was ever added under", "never_added_source"),
        ("another such name", "zzz_absent"),
        (
            "a name one character from a held one",
            // The held row's name is `cwl`; this differs from it only in a trailing
            // character, so it resolves on the whole name rather than a prefix of it.
            "cwlx",
        ),
        (
            "the longest name the constraint allows",
            "absent_at_the_bound_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        ),
    ];
    for (case, name) in rows {
        let read = get(&emulator, name).await;
        assert_eq!(
            read.status, 400,
            "case {case:?}: expected 400 for a name that names nothing, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            error_code(&read),
            "ValidationException",
            "case {case:?}: expected the modelled refusal, got {:?}",
            error_code(&read)
        );
        // The refusal's wording is the service's own sentence for the condition, so
        // every name that resolves to nothing is refused in the same words.
        assert_eq!(
            error_message(&read),
            "Data source not found",
            "case {case:?}: expected the refusal the service words for this condition, got {:?}",
            error_message(&read)
        );
    }
}

/// RULE: the OpenSearch collections a data source is associated with are required for
/// exactly the two union members the model says they are required for, and optional for
/// the third.
#[tokio::test]
async fn opensearch_collections_are_required_for_the_members_that_ask_for_them() {
    let emulator = Emulator::new();
    // (case, the union member, the `OpenSearchArns` to send, whether it is served)
    let rows: Vec<(&str, Value, Option<Vec<&str>>, bool)> = vec![
        (
            "cloudwatch logs naming no collection",
            cloud_watch(ROLE_ARN),
            None,
            false,
        ),
        (
            "cloudwatch logs naming an empty list of collections",
            cloud_watch(ROLE_ARN),
            Some(Vec::new()),
            false,
        ),
        (
            "cloudwatch logs naming one collection",
            cloud_watch(ROLE_ARN),
            Some(vec![COLLECTION_ARN]),
            true,
        ),
        (
            "security lake naming no collection",
            security_lake(ROLE_ARN),
            None,
            false,
        ),
        (
            "security lake naming two collections",
            security_lake(ROLE_ARN),
            Some(vec![COLLECTION_ARN, SECOND_COLLECTION_ARN]),
            true,
        ),
        (
            "prometheus naming no collection",
            prometheus(ROLE_ARN, WORKSPACE_ARN),
            None,
            true,
        ),
        (
            "prometheus naming an empty list of collections",
            prometheus(ROLE_ARN, WORKSPACE_ARN),
            Some(Vec::new()),
            true,
        ),
        (
            "prometheus naming one collection",
            prometheus(ROLE_ARN, WORKSPACE_ARN),
            Some(vec![COLLECTION_ARN]),
            true,
        ),
    ];
    for (index, (case, data_source_type, arns, served)) in rows.iter().enumerate() {
        let name = format!("collection_rule_{index}");
        let mut body = json!({
            "DataSourceName": name,
            "DataSourceType": data_source_type,
        });
        if let Some(arns) = arns {
            body["OpenSearchArns"] = json!(arns);
        }
        let answer = add(&emulator, body).await;
        if *served {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected the modelled refusal, got {:?}",
            error_code(&answer)
        );
        // A refused add persists nothing, so the name it carried still names nothing.
        let read = get(&emulator, &name).await;
        assert_eq!(
            read.status, 400,
            "case {case:?}: expected the refused add to persist nothing, got {} with {:?}",
            read.status, read.body
        );
    }
}

/// RULE: a member that carries an ARN and is handed a value that is not one is a
/// malformed request, and a refused add persists nothing.
#[tokio::test]
async fn a_collection_that_is_not_an_arn_is_refused_as_a_malformed_request() {
    let emulator = Emulator::new();
    // Each value is at least the 20 characters `ARN`'s own `@length` demands, so what
    // refuses it is the syntax check rather than the boundary's length constraint.
    // (case, the value sent as a collection)
    let rows: &[(&str, &str)] = &[
        ("a value with no ARN structure at all", "not-an-arn-at-all-but-long"),
        ("an ARN missing its resource", "arn:aws:es:us-east-1:123456789012:"),
        ("an ARN missing its service", "arn:aws::us-east-1:123456789012:domain/search-logs"),
        ("a value that only starts like an ARN", "arnarnarnarnarnarnarn"),
        (
            "an ARN with too few fields",
            "arn:aws:es:us-east-1:123456789012",
        ),
    ];
    for (index, (case, value)) in rows.iter().enumerate() {
        let name = format!("malformed_collection_{index}");
        let answer = add(
            &emulator,
            json!({
                "DataSourceName": name,
                "DataSourceType": cloud_watch(ROLE_ARN),
                "OpenSearchArns": [value],
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a value that is no ARN, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected the modelled refusal, got {:?}",
            error_code(&answer)
        );
        let read = get(&emulator, &name).await;
        assert_eq!(
            read.status, 400,
            "case {case:?}: expected the refused add to persist nothing, got {} with {:?}",
            read.status, read.body
        );
    }
}

/// RULE: the store is bounded — adds are accepted up to the quota and the one past it
/// is refused with the modelled `LimitExceededException`, leaving the stored set at the
/// quota.
#[tokio::test]
async fn the_account_holds_at_most_the_quota_of_data_sources() {
    let emulator = Emulator::new();
    for index in 0..DIRECT_QUERY_DATA_SOURCE_QUOTA {
        let answer = add(
            &emulator,
            json!({
                "DataSourceName": format!("quota_source_{index}"),
                "DataSourceType": prometheus(ROLE_ARN, WORKSPACE_ARN),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case \"add {index} of the quota\": expected 200, got {} with {:?}",
            answer.status, answer.body
        );
    }

    let past = add(
        &emulator,
        json!({
            "DataSourceName": "one_past_the_quota",
            "DataSourceType": prometheus(ROLE_ARN, WORKSPACE_ARN),
        }),
    )
    .await;
    assert_eq!(
        past.status, 409,
        "case \"one past the quota\": expected 409, got {} with {:?}",
        past.status, past.body
    );
    assert_eq!(
        error_code(&past),
        "LimitExceededException",
        "case \"one past the quota\": expected the modelled refusal, got {:?}",
        error_code(&past)
    );
    let held = emulator.store().lock().direct_query_data_sources.len();
    assert_eq!(
        held, DIRECT_QUERY_DATA_SOURCE_QUOTA,
        "case \"one past the quota\": expected the stored set to stay at the quota, got {held}"
    );
    let read = get(&emulator, "one_past_the_quota").await;
    assert_eq!(
        read.status, 400,
        "case \"one past the quota\": expected the refused add to persist nothing, got {} with {:?}",
        read.status, read.body
    );
}

/// RULE: the tags an add carries belong to the data source it created, so the tagging
/// operations read and extend them under the ARN the add minted, and the same per-
/// resource bound holds on them.
#[tokio::test]
async fn the_tags_an_add_carries_belong_to_the_data_source_it_created() {
    let emulator = Emulator::new();
    let created = vec![tag("project", "trinity"), tag("env", "prod")];
    let minted = added(
        &emulator,
        &Row {
            case: "a data source added with tags",
            name: "tagged_source".to_string(),
            data_source_type: cloud_watch(ROLE_ARN),
            description: None,
            open_search_arns: Some(vec![COLLECTION_ARN.to_string()]),
            policy: None,
            stored_policy: None,
            tags: created.clone(),
        },
    )
    .await;

    assert_eq!(
        stored_tags(&emulator, &minted, "after the add").await,
        as_set(&created),
        "case \"after the add\": expected exactly the tags the add carried"
    );

    // A later attach adds to that same set rather than to a second one.
    let attached = add_tags(&emulator, &minted, wire_tags(&[tag("team", "search")])).await;
    assert_eq!(
        attached.status, 200,
        "case \"a later AddTags\": expected 200, got {} with {:?}",
        attached.status, attached.body
    );
    let mut merged = created.clone();
    merged.push(tag("team", "search"));
    assert_eq!(
        stored_tags(&emulator, &minted, "a later AddTags").await,
        as_set(&merged),
        "case \"a later AddTags\": expected the add's tags and the attached one"
    );

    // An add naming more tags than a resource may carry is refused rather than storing
    // a set no `AddTags` could have built.
    let over: Vec<(String, String)> = (0..=TAG_QUOTA)
        .map(|index| tag(&format!("key{index}"), "value"))
        .collect();
    let refused = add(
        &emulator,
        json!({
            "DataSourceName": "over_the_tag_quota",
            "DataSourceType": cloud_watch(ROLE_ARN),
            "OpenSearchArns": [COLLECTION_ARN],
            "TagList": wire_tags(&over),
        }),
    )
    .await;
    assert_eq!(
        refused.status, 409,
        "case \"one tag past the quota\": expected 409, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "case \"one tag past the quota\": expected the modelled refusal, got {:?}",
        error_code(&refused)
    );
    let read = get(&emulator, "over_the_tag_quota").await;
    assert_eq!(
        read.status, 400,
        "case \"one tag past the quota\": expected the refused add to persist nothing, got {} \
         with {:?}",
        read.status, read.body
    );
}
