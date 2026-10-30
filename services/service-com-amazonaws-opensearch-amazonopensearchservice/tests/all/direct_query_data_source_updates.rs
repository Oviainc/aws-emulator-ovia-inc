// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateDirectQueryDataSource`: changing the configuration of a
//! direct query data source this ACCOUNT already holds.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`]
//! and the tagging helpers from [`super::resource_tags`]; this suite adds no second copy
//! of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * an update of a data source the account holds REPLACES the members the request
//!   speaks about — the union member it is of, the description it carries and the
//!   OpenSearch collections it is associated with — and a read of the name reports the
//!   updated record member for member, over a table of updates differing in which union
//!   member they move the data source to, whether they add, replace, shorten, empty or
//!   drop the description, and how many collections they name;
//! * the identifier a data source was minted with names the same data source
//!   afterwards, so it is what the update answers with and what every later read
//!   reports, and the tags the data source carries are no business of an update;
//! * the access policy "remains unchanged" where the update names none, is replaced by
//!   the document it does name — the tokens the caller wrote, with only the
//!   insignificant whitespace between them dropped, so an integer past what a double
//!   holds exactly and a decimal past a double's precision survive the round trip — and
//!   is cleared by a blank document;
//! * a document that is not JSON is refused and the stored record stands;
//! * a string member's modelled `@pattern` is satisfied only by a value matching it over
//!   its WHOLE length, so a document carrying a line terminator — which
//!   `PolicyDocument`'s `.*` does not admit — is refused, on the update and on the add
//!   that creates the data source alike, and nothing about the record changes;
//! * a name the account holds no data source under is refused as the value it is, in the
//!   words the service states that condition in, and an update creates nothing;
//! * `OpenSearchArns` is required for exactly the two union members the model says it is
//!   required for and optional for the third, and a value that is not an ARN at all is a
//!   malformed request — in both cases the stored record stands;
//! * an update is idempotent: applying one twice leaves the same record.

use serde_json::{json, Value};

use super::domain_lifecycle::{error_code, string_member, Answer, Emulator, PREFIX, REGION};
use super::resource_tags::{as_set, error_message, stored_tags, tag, wire_tags};

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

/// `UpdateDirectQueryDataSource`:
/// `PUT /2021-01-01/opensearch/directQueryDataSource/{DataSourceName}`.
async fn update(emulator: &Emulator, name: &str, body: Value) -> Answer {
    emulator
        .call(
            "PUT",
            &format!("{PREFIX}/opensearch/directQueryDataSource/{name}"),
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
/// account this emulator serves and the name the add carried — so the assertion does not
/// ask the service to confirm its own answer.
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

/// A second role, so an update can move a data source onto another one.
const OTHER_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/OpenSearchDirectQueryRole_2";

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

/// A third, so an update can replace the set rather than only grow or shrink it.
const THIRD_COLLECTION_ARN: &str = "arn:aws:es:us-east-1:123456789012:domain/search-metrics";

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

/// An access policy document carrying an integer past the 2^53 a double holds exactly, a
/// decimal past a double's precision and a timestamp with nanosecond digits, written with
/// no whitespace between its tokens, so what a read reports for it is the document
/// itself.
const PRECISE_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::123456789012:root"},"#,
    r#""Action":"es:ESHttpGet","Resource":"*","Condition":{"#,
    r#""NumericEquals":{"es:QueryBudget":9007199254740993},"#,
    r#""NumericLessThan":{"es:QueryCost":0.100000000000000005},"#,
    r#""DateGreaterThan":{"aws:CurrentTime":"2026-10-24T05:54:21.123456789Z"}}}]}"#
);

/// A second document, written the way a client formats one — whitespace between the
/// tokens, which denotes nothing — so an update can replace one document with another.
///
/// The whitespace is spaces and tabs: a LINE TERMINATOR is outside this member's
/// modelled `@pattern` taken over the whole value, which
/// [`an_access_policy_document_outside_its_whole_value_pattern_is_refused`] pins.
const SPACED_POLICY: &str = concat!(
    "{  \"Version\": \"2012-10-17\",\t\"Statement\": [  {  ",
    "\"Effect\": \"Allow\",  ",
    "\"Action\": [ \"es:ESHttpPost\" ],  ",
    "\"Resource\": \"*\",  ",
    "\"Condition\": { \"NumericEquals\": { \"es:QueryBudget\": 9223372036854775807 } }  ",
    "}  ]  }"
);

/// [`SPACED_POLICY`] as the service holds it: the same tokens in the same order, with the
/// whitespace between them dropped and the digits of its integer untouched.
const SPACED_POLICY_AS_STORED: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Action":["es:ESHttpPost"],"Resource":"*","#,
    r#""Condition":{"NumericEquals":{"es:QueryBudget":9223372036854775807}}}]}"#
);

/// A member of an answer that carries a string, or `None` where the answer carries none.
fn member(answer: &Answer, name: &str) -> Option<String> {
    answer.body[name].as_str().map(str::to_owned)
}

/// The `OpenSearchArns` an answer reports, or `None` where it carries none.
fn reported_arns(answer: &Answer) -> Option<Vec<String>> {
    answer.body["OpenSearchArns"].as_array().map(|arns| {
        arns.iter()
            .map(|arn| arn.as_str().unwrap_or_default().to_owned())
            .collect()
    })
}

/// Add the data source `body` describes and assert the add was served, handing back the
/// `DataSourceArn` it minted.
async fn added(emulator: &Emulator, body: Value, case: &str) -> String {
    let answer = add(emulator, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from AddDirectQueryDataSource, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "DataSourceArn", case)
}

/// Update the data source `name` names and assert the update was served, handing back the
/// `DataSourceArn` it answered with.
async fn updated(emulator: &Emulator, name: &str, body: Value, case: &str) -> String {
    let answer = update(emulator, name, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from UpdateDirectQueryDataSource, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "DataSourceArn", case)
}

/// Read the data source `name` names and assert the read was served.
async fn read(emulator: &Emulator, name: &str, case: &str) -> Answer {
    let answer = get(emulator, name).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from GetDirectQueryDataSource, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// One data source to add, one update to apply to it, and the record the update leaves.
struct Row {
    /// What the case is called when an assertion fails.
    case: &'static str,
    /// The `DataSourceName` the add carries, which is also the name the update and the
    /// read address.
    name: &'static str,
    /// The body of the add that creates the data source.
    initial: Value,
    /// The body of the update applied to it.
    update: Value,
    /// The `DataSourceType` a read reports afterwards.
    expected_type: Value,
    /// The `Description` a read reports afterwards, absent where it reports none.
    expected_description: Option<String>,
    /// The `OpenSearchArns` a read reports afterwards, absent where it reports none.
    expected_arns: Option<Vec<&'static str>>,
    /// The `DataSourceAccessPolicy` a read reports afterwards, absent where it reports
    /// none.
    expected_policy: Option<String>,
}

/// The update table: one row per way an accepted update differs from another — which
/// union member it moves the data source to (every one of the three, including onto
/// itself with another role), whether it adds, replaces, shortens, empties or drops the
/// description, how many collections it names (one, two, three, none for the member that
/// asks for none), and whether it speaks about the access policy.
///
/// `DirectQueryDataSourceName` is `@length(min: 3, max: 80)` with pattern
/// `^[a-z][a-z0-9_]+$`, `DirectQueryDataSourceDescription` is `@length(min: 0, max:
/// 1000)`, `PolicyDocument` is `@length(min: 0, max: 102400)` and `OpenSearchArns` is
/// optional — all from this service's Smithy model
/// (`smithy_query(query="operation",operation="UpdateDirectQueryDataSource")`), so every
/// row is inside the modelled constraints and the service itself is what accepts it.
fn update_rows() -> Vec<Row> {
    vec![
        // CloudWatch Logs onto another role, a description where there was none, and one
        // collection replaced by another.
        Row {
            case: "another role, a description added, one collection for another",
            name: "cwl_source",
            initial: json!({
                "DataSourceName": "cwl_source",
                "DataSourceType": cloud_watch(ROLE_ARN),
                "OpenSearchArns": [COLLECTION_ARN],
            }),
            update: json!({
                "DataSourceType": cloud_watch(OTHER_ROLE_ARN),
                "Description": "Logs read through the second role.",
                "OpenSearchArns": [SECOND_COLLECTION_ARN],
            }),
            expected_type: cloud_watch(OTHER_ROLE_ARN),
            expected_description: Some("Logs read through the second role.".to_string()),
            expected_arns: Some(vec![SECOND_COLLECTION_ARN]),
            expected_policy: None,
        },
        // A move across union members, from CloudWatch Logs to Security Lake, with the
        // collection set GROWN to three and the access policy replaced.
        Row {
            case: "cloudwatch logs to security lake, three collections, a new policy",
            name: "crossing_members",
            initial: json!({
                "DataSourceName": "crossing_members",
                "DataSourceType": cloud_watch(ROLE_ARN),
                "Description": "the first description",
                "OpenSearchArns": [COLLECTION_ARN],
                "DataSourceAccessPolicy": PRECISE_POLICY,
            }),
            update: json!({
                "DataSourceType": security_lake(ROLE_ARN),
                "Description": "the second description",
                "OpenSearchArns": [
                    COLLECTION_ARN,
                    SECOND_COLLECTION_ARN,
                    THIRD_COLLECTION_ARN,
                ],
                "DataSourceAccessPolicy": SPACED_POLICY,
            }),
            expected_type: security_lake(ROLE_ARN),
            expected_description: Some("the second description".to_string()),
            expected_arns: Some(vec![
                COLLECTION_ARN,
                SECOND_COLLECTION_ARN,
                THIRD_COLLECTION_ARN,
            ]),
            expected_policy: Some(SPACED_POLICY_AS_STORED.to_string()),
        },
        // A move onto Prometheus, the one member that asks for no collection at all: the
        // collections the data source was associated with go with the member that asked
        // for them, and the policy the update says nothing about stays.
        Row {
            case: "security lake to prometheus, which names no collection",
            name: "onto_prometheus",
            initial: json!({
                "DataSourceName": "onto_prometheus",
                "DataSourceType": security_lake(ROLE_ARN),
                "Description": "events from the lake",
                "OpenSearchArns": [COLLECTION_ARN, SECOND_COLLECTION_ARN],
                "DataSourceAccessPolicy": PRECISE_POLICY,
            }),
            update: json!({
                "DataSourceType": prometheus(ROLE_ARN, WORKSPACE_ARN),
                "Description": "metrics from the workspace",
            }),
            expected_type: prometheus(ROLE_ARN, WORKSPACE_ARN),
            expected_description: Some("metrics from the workspace".to_string()),
            expected_arns: None,
            expected_policy: Some(PRECISE_POLICY.to_string()),
        },
        // A move OFF Prometheus, onto a member that asks for collections, and the
        // description emptied to the empty string the constraint allows (0).
        Row {
            case: "prometheus to cloudwatch logs, the description emptied",
            name: "off_prometheus",
            initial: json!({
                "DataSourceName": "off_prometheus",
                "DataSourceType": prometheus(ROLE_ARN, WORKSPACE_ARN),
                "Description": "metrics from the workspace",
            }),
            update: json!({
                "DataSourceType": cloud_watch(ROLE_ARN),
                "Description": "",
                "OpenSearchArns": [THIRD_COLLECTION_ARN],
            }),
            expected_type: cloud_watch(ROLE_ARN),
            expected_description: Some(String::new()),
            expected_arns: Some(vec![THIRD_COLLECTION_ARN]),
            expected_policy: None,
        },
        // The description DROPPED: the update names none, and the model states no
        // "remains unchanged" rule for that member, so the data source carries none
        // afterwards. The collection set shrinks from two to one.
        Row {
            case: "the description dropped, two collections down to one",
            name: "dropping_the_description",
            initial: json!({
                "DataSourceName": "dropping_the_description",
                "DataSourceType": security_lake(ROLE_ARN),
                "Description": "d".repeat(1000),
                "OpenSearchArns": [COLLECTION_ARN, SECOND_COLLECTION_ARN],
            }),
            update: json!({
                "DataSourceType": security_lake(OTHER_ROLE_ARN),
                "OpenSearchArns": [SECOND_COLLECTION_ARN],
            }),
            expected_type: security_lake(OTHER_ROLE_ARN),
            expected_description: None,
            expected_arns: Some(vec![SECOND_COLLECTION_ARN]),
            expected_policy: None,
        },
        // The description at its longest (1000) where there was a short one, and the
        // access policy CLEARED by the blank document `PolicyDocument`'s own minimum
        // allows, which names no policy.
        Row {
            case: "the longest description, the policy cleared by a blank document",
            name: "clearing_the_policy",
            initial: json!({
                "DataSourceName": "clearing_the_policy",
                "DataSourceType": cloud_watch(ROLE_ARN),
                "Description": "short",
                "OpenSearchArns": [COLLECTION_ARN],
                "DataSourceAccessPolicy": PRECISE_POLICY,
            }),
            update: json!({
                "DataSourceType": cloud_watch(ROLE_ARN),
                "Description": "e".repeat(1000),
                "OpenSearchArns": [COLLECTION_ARN],
                "DataSourceAccessPolicy": "",
            }),
            expected_type: cloud_watch(ROLE_ARN),
            expected_description: Some("e".repeat(1000)),
            expected_arns: Some(vec![COLLECTION_ARN]),
            expected_policy: None,
        },
    ]
}

/// Assert the record `name` names is the one `row` says the update leaves.
async fn assert_record(emulator: &Emulator, row: &Row, arn: &str) {
    let answer = read(emulator, row.name, row.case).await;
    assert_eq!(
        member(&answer, "DataSourceName"),
        Some(row.name.to_string()),
        "case {:?}: expected the name the update addressed, got {:?}",
        row.case,
        member(&answer, "DataSourceName")
    );
    assert_eq!(
        answer.body["DataSourceType"], row.expected_type,
        "case {:?}: expected the union member the update named, got {:?}",
        row.case, answer.body["DataSourceType"]
    );
    assert_eq!(
        member(&answer, "Description"),
        row.expected_description,
        "case {:?}: expected the description the update left, got {:?}",
        row.case,
        member(&answer, "Description")
    );
    assert_eq!(
        reported_arns(&answer),
        row.expected_arns
            .as_ref()
            .map(|arns| arns.iter().map(|arn| (*arn).to_string()).collect()),
        "case {:?}: expected the collections the update left, got {:?}",
        row.case,
        reported_arns(&answer)
    );
    assert_eq!(
        member(&answer, "DataSourceAccessPolicy"),
        row.expected_policy,
        "case {:?}: expected the access policy the update left, got {:?}",
        row.case,
        member(&answer, "DataSourceAccessPolicy")
    );
    assert_eq!(
        member(&answer, "DataSourceArn").as_deref(),
        Some(arn),
        "case {:?}: expected the read to report the identifier the data source was minted with, got {:?}",
        row.case,
        member(&answer, "DataSourceArn")
    );
}

/// RULE: an update of a data source the account holds replaces the members the request
/// speaks about, and a read of its name reports the updated record member for member.
#[tokio::test]
async fn an_update_replaces_the_configuration_a_read_reports() {
    let emulator = Emulator::new();
    for row in update_rows() {
        let minted = added(&emulator, row.initial.clone(), row.case).await;
        let answered = updated(&emulator, row.name, row.update.clone(), row.case).await;

        // The identifier is the data source's own, minted when it was added, so the
        // update reports the one the add did rather than a second one.
        assert_eq!(
            answered,
            expected_arn(row.name),
            "case {:?}: expected the identifier of this account's data source, got {answered:?}",
            row.case
        );
        assert_eq!(
            answered, minted,
            "case {:?}: expected the update to answer with the identifier the add minted ({minted:?}), got {answered:?}",
            row.case
        );

        assert_record(&emulator, &row, &minted).await;
    }
}

/// RULE: an update is settled by the time it answers, so applying the same one again
/// leaves the same record and answers the same way.
#[tokio::test]
async fn applying_an_update_twice_leaves_the_same_record() {
    let emulator = Emulator::new();
    for row in update_rows() {
        let minted = added(&emulator, row.initial.clone(), row.case).await;
        updated(&emulator, row.name, row.update.clone(), row.case).await;
        let once = read(&emulator, row.name, row.case).await;

        updated(&emulator, row.name, row.update.clone(), row.case).await;
        let twice = read(&emulator, row.name, row.case).await;
        assert_eq!(
            twice.body, once.body,
            "case {:?}: expected the second update to leave the same record as the first, got {:?}",
            row.case, twice.body
        );
        assert_record(&emulator, &row, &minted).await;
    }
}

/// One document a data source is added carrying, one an update names, and the document
/// a read reports once the update has been served.
struct PolicyRow {
    /// What the case is called when an assertion fails.
    case: &'static str,
    /// The `DataSourceAccessPolicy` the add names, absent where it names none.
    added: Option<String>,
    /// The `DataSourceAccessPolicy` the update names, absent where it names none.
    sent: Option<String>,
    /// The document a read reports afterwards, absent where it reports none.
    stored: Option<String>,
}

/// A [`PolicyRow`] from the three documents it is about, so a row reads as the three
/// documents it names rather than as a tuple of conversions.
fn policy_row(
    case: &'static str,
    added: Option<&str>,
    sent: Option<&str>,
    stored: Option<&str>,
) -> PolicyRow {
    PolicyRow {
        case,
        added: added.map(str::to_owned),
        sent: sent.map(str::to_owned),
        stored: stored.map(str::to_owned),
    }
}

/// RULE: the access policy "remains unchanged" where the update names none, is replaced
/// by the document the update does name — the tokens the caller wrote, with only the
/// insignificant whitespace between them dropped, so every value survives the round trip
/// as written — and is cleared where the update names a blank document.
#[tokio::test]
async fn an_update_carries_the_access_policy_through_as_the_document_it_denotes() {
    let emulator = Emulator::new();
    // A document at a thousand characters, which `PolicyDocument`'s `@length(min: 0,
    // max: 102400)` allows many times over.
    let long_document = format!(r#"{{"Sid":"{}"}}"#, "s".repeat(1000));
    let rows: Vec<PolicyRow> = vec![
        policy_row(
            "an integer past what a double holds exactly, and nanosecond digits",
            None,
            Some(PRECISE_POLICY),
            Some(PRECISE_POLICY),
        ),
        policy_row(
            "a document written with whitespace between its tokens",
            None,
            Some(SPACED_POLICY),
            Some(SPACED_POLICY_AS_STORED),
        ),
        policy_row(
            "integers at the edges of i64",
            Some(PRECISE_POLICY),
            Some(r#"{"Budget":9223372036854775807,"Floor":-9223372036854775808}"#),
            Some(r#"{"Budget":9223372036854775807,"Floor":-9223372036854775808}"#),
        ),
        policy_row(
            "decimals that go past a double's precision, and an exponent",
            None,
            Some(r#"{ "Rate": 0.1234567890123456789, "Scale": 1e308 }"#),
            Some(r#"{"Rate":0.1234567890123456789,"Scale":1e308}"#),
        ),
        policy_row(
            "member order a re-render through a sorted map would not keep",
            None,
            Some("{  \"Statement\" : [ ],  \"Version\" : \"2012-10-17\"  }"),
            Some(r#"{"Statement":[],"Version":"2012-10-17"}"#),
        ),
        policy_row(
            "non-ASCII text, an escaped quote and braces inside a string",
            None,
            Some("{ \"Sid\": \"données-de-journal-日本語 \\\" { }\" }"),
            Some("{\"Sid\":\"données-de-journal-日本語 \\\" { }\"}"),
        ),
        policy_row(
            "a document at a thousand characters",
            Some(PRECISE_POLICY),
            Some(&long_document),
            Some(&long_document),
        ),
        policy_row(
            "an update that names no document, over a data source carrying one",
            Some(PRECISE_POLICY),
            None,
            Some(PRECISE_POLICY),
        ),
        policy_row(
            "an update that names no document, over a data source carrying none",
            None,
            None,
            None,
        ),
        policy_row(
            "the blank document the constraint allows, which names no policy",
            Some(PRECISE_POLICY),
            Some(""),
            None,
        ),
        policy_row(
            "a document of whitespace only, which names no policy either",
            Some(SPACED_POLICY),
            Some("   \t "),
            None,
        ),
    ];
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("policy_update_{index}");
        let mut initial = json!({
            "DataSourceName": name,
            "DataSourceType": cloud_watch(ROLE_ARN),
            "OpenSearchArns": [COLLECTION_ARN],
        });
        if let Some(policy) = &row.added {
            initial["DataSourceAccessPolicy"] = json!(policy);
        }
        added(&emulator, initial, case).await;

        let mut body = json!({
            "DataSourceType": cloud_watch(ROLE_ARN),
            "OpenSearchArns": [COLLECTION_ARN],
        });
        if let Some(policy) = &row.sent {
            body["DataSourceAccessPolicy"] = json!(policy);
        }
        updated(&emulator, &name, body, case).await;

        let answer = read(&emulator, &name, case).await;
        assert_eq!(
            member(&answer, "DataSourceAccessPolicy"),
            row.stored,
            "case {case:?}: expected the document the service holds, got {:?}",
            member(&answer, "DataSourceAccessPolicy")
        );
    }
}

/// RULE: an access policy document "must be in valid JSON format", so a text that is not
/// JSON is a malformed request and the stored record stands.
#[tokio::test]
async fn an_update_naming_a_document_that_is_not_json_is_refused() {
    let emulator = Emulator::new();
    // (case, the text sent as the document)
    let rows: &[(&str, &str)] = &[
        ("text that is not JSON at all", "allow everything"),
        ("a document that stops mid-way", r#"{"Version":"2012-10-17""#),
        ("a trailing comma JSON does not allow", r#"{"Version":"2012-10-17",}"#),
        ("a single quoted member name", r#"{'Version':'2012-10-17'}"#),
        ("an unquoted member name", r#"{Version:"2012-10-17"}"#),
    ];
    let name = "policy_refusals";
    added(
        &emulator,
        json!({
            "DataSourceName": name,
            "DataSourceType": cloud_watch(ROLE_ARN),
            "OpenSearchArns": [COLLECTION_ARN],
            "DataSourceAccessPolicy": PRECISE_POLICY,
        }),
        "the data source the refused updates address",
    )
    .await;
    let before = read(&emulator, name, "before the refusals").await;

    for (case, policy) in rows {
        let answer = update(
            &emulator,
            name,
            json!({
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
        assert_eq!(
            error_message(&answer),
            "DataSourceAccessPolicy must be a policy document in valid JSON format.",
            "case {case:?}: expected the refusal naming the member that carried it, got {:?}",
            error_message(&answer)
        );

        let after = read(&emulator, name, case).await;
        assert_eq!(
            after.body, before.body,
            "case {case:?}: expected the refused update to leave the record as it was, got {:?}",
            after.body
        );
    }
}

/// `PolicyDocument`'s modelled `@pattern`, as a refusal quotes it
/// (`smithy_query(query="shape",shape="PolicyDocument")` — `@length(min: 0, max: 102400)`,
/// `@pattern ".*"`): the expression the model declares, which carries no anchors to strip.
const POLICY_DOCUMENT_PATTERN: &str = ".*";

/// The constraint report a document outside that pattern is refused with: the framework's
/// envelope over one clause naming the submitted value, the member path and the
/// expression — the same sentence every other constraint refusal of this service states.
fn pattern_refusal(document: &str) -> String {
    format!(
        "1 validation error detected: Value '{document}' at 'dataSourceAccessPolicy' \
         failed to satisfy constraint: Member must satisfy regular expression pattern: \
         {POLICY_DOCUMENT_PATTERN}"
    )
}

/// RULE: a string member's modelled `@pattern` is satisfied only by a value that matches
/// it over its WHOLE length, so an access policy document carrying a line terminator —
/// which a dot does not match — is refused as a malformed request on every operation
/// carrying that member, and nothing of the data source changes.
#[tokio::test]
async fn an_access_policy_document_outside_its_whole_value_pattern_is_refused() {
    let emulator = Emulator::new();
    // (case, the document sent) — each row differs in where the line terminators sit,
    // how many there are and what else the document holds. Every one of them CONTAINS a
    // run of characters the expression matches, which is what a find accepts.
    let rows: Vec<(&str, String)> = vec![
        (
            "a document pretty-printed across several lines",
            "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": []\n}".to_string(),
        ),
        (
            "a leading line feed",
            "\n{\"Version\":\"2012-10-17\"}".to_string(),
        ),
        (
            "a trailing line feed, as a file on disk ends",
            "{\"Version\":\"2012-10-17\"}\n".to_string(),
        ),
        (
            "a carriage return beside the line feed",
            "{\"Version\":\"2012-10-17\",\r\n\"Statement\":[]}".to_string(),
        ),
        (
            "two line feeds between members",
            "{\"Sid\":\"a\",\n\"Effect\":\"Allow\",\n\"Resource\":\"*\"}".to_string(),
        ),
        (
            "nothing but a line feed",
            "\n".to_string(),
        ),
        (
            // Whitespace alone names no policy, so this row is what shows the
            // constraint is settled on the VALUE before anything is read out of it.
            "whitespace carrying a line feed, which names no policy",
            "  \n  ".to_string(),
        ),
        (
            "a line feed inside a string, which no JSON parser accepts either",
            "{\"Sid\":\"two\nlines\"}".to_string(),
        ),
        (
            "a document at a thousand characters with one line feed in it",
            format!(r#"{{"Sid":"{}","Extra":{}"x"}}"#, "s".repeat(1000), "\n"),
        ),
    ];
    let name = "whole_value_pattern";
    added(
        &emulator,
        json!({
            "DataSourceName": name,
            "DataSourceType": cloud_watch(ROLE_ARN),
            "OpenSearchArns": [COLLECTION_ARN],
            "DataSourceAccessPolicy": PRECISE_POLICY,
        }),
        "the data source the refused updates address",
    )
    .await;
    let before = read(&emulator, name, "before the refusals").await;

    for (index, (case, document)) in rows.iter().enumerate() {
        let answer = update(
            &emulator,
            name,
            json!({
                "DataSourceType": cloud_watch(ROLE_ARN),
                "OpenSearchArns": [COLLECTION_ARN],
                "DataSourceAccessPolicy": document,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a document outside the member's pattern, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected the modelled refusal, got {:?}",
            error_code(&answer)
        );
        assert_eq!(
            error_message(&answer),
            pattern_refusal(document),
            "case {case:?}: expected the constraint report naming the member and the pattern, got {:?}",
            error_message(&answer)
        );

        let after = read(&emulator, name, case).await;
        assert_eq!(
            after.body, before.body,
            "case {case:?}: expected the refused update to leave the record as it was, got {:?}",
            after.body
        );

        // The rule is the MEMBER's, not the operation's: the add that creates a data
        // source carries the same member under the same pattern, so it refuses the same
        // document in the same words and persists nothing.
        let fresh = format!("whole_value_pattern_add_{index}");
        let add_answer = add(
            &emulator,
            json!({
                "DataSourceName": fresh,
                "DataSourceType": cloud_watch(ROLE_ARN),
                "OpenSearchArns": [COLLECTION_ARN],
                "DataSourceAccessPolicy": document,
            }),
        )
        .await;
        assert_eq!(
            add_answer.status, 400,
            "case {case:?}: expected the add to refuse the same document, got {} with {:?}",
            add_answer.status, add_answer.body
        );
        assert_eq!(
            error_message(&add_answer),
            pattern_refusal(document),
            "case {case:?}: expected the add to state the same constraint report, got {:?}",
            error_message(&add_answer)
        );
        let absent = get(&emulator, &fresh).await;
        assert_eq!(
            absent.status, 400,
            "case {case:?}: expected the refused add to persist nothing, got {} with {:?}",
            absent.status, absent.body
        );
    }
}

/// RULE: an update addresses a data source the account already holds, so a name it holds
/// none under is refused as the value it is — the modelled `ValidationException`, in the
/// words the service states that condition in — and the update creates nothing.
#[tokio::test]
async fn an_update_of_a_name_that_names_no_data_source_is_refused() {
    let emulator = Emulator::new();
    added(
        &emulator,
        json!({
            "DataSourceName": "cwl_source",
            "DataSourceType": cloud_watch(ROLE_ARN),
            "OpenSearchArns": [COLLECTION_ARN],
        }),
        "the one data source the account holds",
    )
    .await;

    // (case, the name the update addresses)
    let rows: &[(&str, &str)] = &[
        ("a name nothing was ever added under", "never_added_source"),
        ("another such name", "zzz_absent"),
        (
            // The held name is `cwl_source`; this differs from it only in a trailing
            // character, so the name resolves whole rather than as a prefix.
            "a name one character from a held one",
            "cwl_sourcex",
        ),
        (
            "the shortest name the constraint allows",
            "abc",
        ),
        (
            "the longest name the constraint allows",
            "absent_at_the_bound_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        ),
    ];
    for (case, name) in rows {
        let answer = update(
            &emulator,
            name,
            json!({
                "DataSourceType": cloud_watch(ROLE_ARN),
                "Description": "an update of a data source that is not there",
                "OpenSearchArns": [COLLECTION_ARN],
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a name that names nothing, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected the modelled refusal, got {:?}",
            error_code(&answer)
        );
        assert_eq!(
            error_message(&answer),
            "Data source not found",
            "case {case:?}: expected the refusal the service words for this condition, got {:?}",
            error_message(&answer)
        );

        // An update creates nothing, so the name still names no data source.
        let after = get(&emulator, name).await;
        assert_eq!(
            after.status, 400,
            "case {case:?}: expected the refused update to create nothing, got {} with {:?}",
            after.status, after.body
        );
    }
}

/// RULE: the OpenSearch collections a data source is associated with are required for
/// exactly the two union members the model says they are required for, and optional for
/// the third — on an update as on an add, because it is a rule about the type the request
/// names.
#[tokio::test]
async fn collections_are_required_on_an_update_for_the_members_that_ask_for_them() {
    let emulator = Emulator::new();
    // (case, the union member the update names, the `OpenSearchArns` it names, whether
    // the update is served)
    let rows: Vec<(&str, Value, Option<Vec<&str>>, bool)> = vec![
        (
            "cloudwatch logs naming one collection",
            cloud_watch(ROLE_ARN),
            Some(vec![COLLECTION_ARN]),
            true,
        ),
        (
            "cloudwatch logs naming none",
            cloud_watch(ROLE_ARN),
            None,
            false,
        ),
        (
            "cloudwatch logs naming an empty list, which names none either",
            cloud_watch(ROLE_ARN),
            Some(Vec::new()),
            false,
        ),
        (
            "security lake naming two collections",
            security_lake(ROLE_ARN),
            Some(vec![COLLECTION_ARN, SECOND_COLLECTION_ARN]),
            true,
        ),
        (
            "security lake naming none",
            security_lake(ROLE_ARN),
            None,
            false,
        ),
        (
            "security lake naming an empty list",
            security_lake(ROLE_ARN),
            Some(Vec::new()),
            false,
        ),
        (
            "prometheus naming none, the member that asks for none",
            prometheus(ROLE_ARN, WORKSPACE_ARN),
            None,
            true,
        ),
        (
            "prometheus naming one anyway",
            prometheus(ROLE_ARN, WORKSPACE_ARN),
            Some(vec![COLLECTION_ARN]),
            true,
        ),
    ];
    for (index, (case, data_source_type, arns, served)) in rows.iter().enumerate() {
        let name = format!("collection_rule_{index}");
        added(
            &emulator,
            json!({
                "DataSourceName": name,
                "DataSourceType": prometheus(ROLE_ARN, WORKSPACE_ARN),
                "Description": "the record the update is applied to",
            }),
            case,
        )
        .await;
        let before = read(&emulator, &name, case).await;

        let mut body = json!({ "DataSourceType": data_source_type });
        if let Some(arns) = arns {
            body["OpenSearchArns"] = json!(arns);
        }
        let answer = update(&emulator, &name, body).await;

        if *served {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the update to be served, got {} with {:?}",
                answer.status, answer.body
            );
            let after = read(&emulator, &name, case).await;
            assert_eq!(
                after.body["DataSourceType"], *data_source_type,
                "case {case:?}: expected the union member the update named, got {:?}",
                after.body["DataSourceType"]
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a type whose collections are required, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected the modelled refusal, got {:?}",
            error_code(&answer)
        );
        // The refusal names the member and the type that asks for it, so the caller can
        // fix the request from the refusal alone. The type it names is the union member
        // the request carried, read off the row rather than off the answer.
        let member_name = data_source_type
            .as_object()
            .and_then(|union| union.keys().next().cloned())
            .unwrap_or_default();
        assert_eq!(
            error_message(&answer),
            format!("OpenSearchArns is required for the {member_name} data source type."),
            "case {case:?}: expected the refusal to name the member and the type, got {:?}",
            error_message(&answer)
        );

        let after = read(&emulator, &name, case).await;
        assert_eq!(
            after.body, before.body,
            "case {case:?}: expected the refused update to leave the record as it was, got {:?}",
            after.body
        );
    }
}

/// RULE: every element of `OpenSearchArns` is an ARN, so a value that is not one at all
/// is a malformed request and the stored record stands.
#[tokio::test]
async fn an_update_naming_a_collection_that_is_not_an_arn_is_refused() {
    let emulator = Emulator::new();
    // (case, the value sent in `OpenSearchArns`) — each is at least the 20 characters
    // `ARN`'s own `@length(min: 20, max: 2048)` asks for, so the request boundary
    // accepts it and the service is what refuses it.
    let rows: &[(&str, &str)] = &[
        ("a sentence of the right length", "not an arn at all, no"),
        ("a URL", "https://example.invalid/collections/a"),
        ("an ARN missing its prefix", "aws:es:us-east-1:123456789012:domain/x"),
        ("a name with no partition", "arn-es-us-east-1-123456789012-domain"),
    ];
    let name = "collection_arn_refusals";
    added(
        &emulator,
        json!({
            "DataSourceName": name,
            "DataSourceType": cloud_watch(ROLE_ARN),
            "OpenSearchArns": [COLLECTION_ARN],
        }),
        "the data source the refused updates address",
    )
    .await;
    let before = read(&emulator, name, "before the refusals").await;

    for (case, arn) in rows {
        let answer = update(
            &emulator,
            name,
            json!({
                "DataSourceType": cloud_watch(ROLE_ARN),
                "OpenSearchArns": [arn],
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a value that is not an ARN, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected the modelled refusal, got {:?}",
            error_code(&answer)
        );

        let after = read(&emulator, name, case).await;
        assert_eq!(
            after.body, before.body,
            "case {case:?}: expected the refused update to leave the record as it was, got {:?}",
            after.body
        );
    }
}

/// RULE: an update speaks about the data source's configuration only, so the tags it
/// carries — which no member of this operation names — are the resource's own across an
/// update, read under the identifier the data source was minted with.
#[tokio::test]
async fn an_update_leaves_the_tags_the_data_source_carries() {
    let emulator = Emulator::new();
    let tags = vec![tag("project", "trinity"), tag("env", "prod")];
    let name = "tagged_across_updates";
    let minted = added(
        &emulator,
        json!({
            "DataSourceName": name,
            "DataSourceType": cloud_watch(ROLE_ARN),
            "OpenSearchArns": [COLLECTION_ARN],
            "TagList": wire_tags(&tags),
        }),
        "a tagged data source",
    )
    .await;

    for (case, body) in [
        (
            "an update onto another member",
            json!({
                "DataSourceType": prometheus(ROLE_ARN, WORKSPACE_ARN),
                "Description": "metrics",
            }),
        ),
        (
            "an update back, naming two collections",
            json!({
                "DataSourceType": security_lake(ROLE_ARN),
                "OpenSearchArns": [COLLECTION_ARN, SECOND_COLLECTION_ARN],
            }),
        ),
    ] {
        let answered = updated(&emulator, name, body, case).await;
        assert_eq!(
            answered, minted,
            "case {case:?}: expected the identifier the add minted ({minted:?}), got {answered:?}",
            );
        assert_eq!(
            stored_tags(&emulator, &minted, case).await,
            as_set(&tags),
            "case {case:?}: expected the update to leave the tags the data source carries",
        );
    }
}
