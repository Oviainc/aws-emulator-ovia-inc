// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteDirectQueryDataSource`: what a delete releases, and
//! what every read path reports once the record is gone.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the tagging paths from [`super::resource_tags`];
//! this suite adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a delete of a data source this account holds answers with the modelled empty
//!   response and the record is gone where it stood: its own read, the inventory and
//!   the tagging paths under its ARN all report it no more, while every OTHER data
//!   source the account holds is reported exactly as before — asserted over a table of
//!   adds differing in the union member they carry, in the name's length (both
//!   published bounds) and character classes, in whether a description, an access
//!   policy, OpenSearch collections or tags were supplied, and in how many collections
//!   they named;
//! * a name the account holds no data source under is refused as the value it is: the
//!   modelled `ValidationException`, in the words the service states that condition in,
//!   and nothing is deleted — including a second delete of a name the first one already
//!   released;
//! * what the record held goes with it: the name is free for another add, which mints a
//!   NEW data source carrying none of the removed one's description, collections, policy
//!   or tags;
//! * the bound this account's store is kept under counts the records it holds, so a
//!   deleted record frees exactly one slot in it and no more.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::DIRECT_QUERY_DATA_SOURCE_QUOTA;

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX, REGION};
use super::resource_tags::{as_set, error_message, list_tags, stored_tags, tag, wire_tags};

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

/// `DeleteDirectQueryDataSource`:
/// `DELETE /2021-01-01/opensearch/directQueryDataSource/{DataSourceName}`.
async fn delete(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/directQueryDataSource/{name}"),
            None,
        )
        .await
}

/// `ListDirectQueryDataSources`:
/// `GET /2021-01-01/opensearch/directQueryDataSource`, paged by `nexttoken`.
async fn list(emulator: &Emulator, query: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/directQueryDataSource{query}"),
            None,
        )
        .await
}

/// Every name the inventory reports, walked across its pages, so an assertion about
/// the whole stored set does not depend on one page's size.
async fn inventory(emulator: &Emulator, case: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut query = String::new();
    loop {
        let page = list(emulator, &query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected 200 from ListDirectQueryDataSources, got {} with {:?}",
            page.status, page.body
        );
        let rows = page.body["DirectQueryDataSources"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a DirectQueryDataSources list, got {:?}",
                    page.body
                )
            })
            .clone();
        for row in &rows {
            names.push(row["DataSourceName"].as_str().unwrap_or_default().to_owned());
        }
        match page.body["NextToken"].as_str() {
            Some(token) if !token.is_empty() => query = format!("?nexttoken={token}"),
            _ => return names,
        }
    }
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

/// A policy document "in valid JSON format", for a row that names one.
const ACCESS_POLICY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                             \"Principal\":{\"AWS\":\"*\"},\"Action\":\"es:ESHttpGet\"}]}";

/// One data source the account holds, and therefore one record a delete can release.
struct Row {
    /// What the case is called when an assertion fails.
    case: &'static str,
    /// The `DataSourceName` the add carries, which is also the name that addresses the
    /// record afterwards.
    name: String,
    /// The `DataSourceType` union, as the wire carries it.
    data_source_type: Value,
    /// The `Description` to send, absent where `None`.
    description: Option<&'static str>,
    /// The `OpenSearchArns` to send, absent where `None`.
    open_search_arns: Option<Vec<&'static str>>,
    /// The `DataSourceAccessPolicy` to send, absent where `None`.
    policy: Option<&'static str>,
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
        if let Some(description) = self.description {
            body["Description"] = json!(description);
        }
        if let Some(arns) = &self.open_search_arns {
            body["OpenSearchArns"] = json!(arns);
        }
        if let Some(policy) = self.policy {
            body["DataSourceAccessPolicy"] = json!(policy);
        }
        if !self.tags.is_empty() {
            body["TagList"] = wire_tags(&self.tags);
        }
        body
    }
}

/// The table every rule below is asserted over: one row per way one stored data source
/// differs from another — the union member, the name's length at each published bound
/// and its character classes, the optional members it carries, and how many collections
/// it names — because what a delete releases is the same whatever the record holds.
///
/// `DirectQueryDataSourceName` is `@length(min: 3, max: 80)` with pattern
/// `^[a-z][a-z0-9_]+$`
/// (`smithy_query(query="operation",operation="DeleteDirectQueryDataSource")`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "cloudwatch logs, the bare required members, at the 3-character bound",
            name: "cwl".to_string(),
            data_source_type: cloud_watch(ROLE_ARN),
            description: None,
            open_search_arns: Some(vec![COLLECTION_ARN]),
            policy: None,
            tags: Vec::new(),
        },
        Row {
            case: "security lake, a description and two collections",
            name: "security_lake_source".to_string(),
            data_source_type: security_lake(ROLE_ARN),
            description: Some("security lake tables of this account"),
            open_search_arns: Some(vec![COLLECTION_ARN, SECOND_COLLECTION_ARN]),
            policy: None,
            tags: Vec::new(),
        },
        Row {
            case: "prometheus, naming no collection at all",
            name: "prom_workspace".to_string(),
            data_source_type: prometheus(ROLE_ARN, WORKSPACE_ARN),
            description: None,
            open_search_arns: None,
            policy: None,
            tags: Vec::new(),
        },
        Row {
            case: "an access policy document",
            name: "policy_source".to_string(),
            data_source_type: prometheus(ROLE_ARN, WORKSPACE_ARN),
            description: Some("a source carrying an access policy"),
            open_search_arns: None,
            policy: Some(ACCESS_POLICY),
            tags: Vec::new(),
        },
        Row {
            case: "tags named by the add",
            name: "tagged_source_1".to_string(),
            data_source_type: cloud_watch(ROLE_ARN),
            description: None,
            open_search_arns: Some(vec![COLLECTION_ARN]),
            policy: None,
            tags: vec![tag("team", "search"), tag("cost-center", "")],
        },
        Row {
            // Exactly 80 characters: the upper bound of the name's modelled length.
            case: "an 80-character name at the upper bound, with every allowed class",
            name: format!("a_9{}", "z".repeat(77)),
            data_source_type: security_lake(ROLE_ARN),
            description: Some("named at the published upper bound"),
            open_search_arns: Some(vec![SECOND_COLLECTION_ARN]),
            policy: Some(ACCESS_POLICY),
            tags: vec![tag("stage", "beta")],
        },
    ]
}

/// Add every row of the table and hand back the name and ARN each one is addressed by.
async fn seeded(emulator: &Emulator, rows: &[Row]) -> Vec<(String, String)> {
    let mut held = Vec::new();
    for row in rows {
        let added = add(emulator, row.body()).await;
        assert_eq!(
            added.status, 200,
            "case {:?}: expected 200 from AddDirectQueryDataSource, got {} with {:?}",
            row.case, added.status, added.body
        );
        let arn = added.body["DataSourceArn"].as_str().unwrap_or_default();
        assert_eq!(
            arn,
            expected_arn(&row.name),
            "case {:?}: expected the data source's own ARN, got {arn:?}",
            row.case
        );
        held.push((row.name.clone(), arn.to_string()));
    }
    held
}

/// The data source `name` names, asserted to be one no read path reports any more.
async fn assert_gone(emulator: &Emulator, name: &str, arn: &str, case: &str) {
    let read = get(emulator, name).await;
    assert_eq!(
        read.status, 400,
        "case {case:?}: a released data source resolves for nobody, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        error_code(&read),
        "ValidationException",
        "case {case:?}: expected the modelled refusal, got {:?}",
        read.body
    );
    assert_eq!(
        error_message(&read),
        "Data source not found",
        "case {case:?}: expected the refusal the service words for this condition, got {:?}",
        error_message(&read)
    );

    let listed = inventory(emulator, case).await;
    assert!(
        !listed.contains(&name.to_string()),
        "case {case:?}: the inventory reports no released data source, got {listed:?}"
    );

    // The tag set the tagging paths resolve that ARN to lived ON the record, so it went
    // with the record: the ARN names no resource of this account any more.
    let orphaned = list_tags(emulator, arn).await;
    assert_eq!(
        orphaned.status, 400,
        "case {case:?}: the ARN of a released data source resolves for nobody, got {} with {:?}",
        orphaned.status, orphaned.body
    );
    assert_eq!(
        error_code(&orphaned),
        "ValidationException",
        "case {case:?}: the tagging paths refuse an unresolvable ARN, got {:?}",
        orphaned.body
    );
}

/// RULE: a delete of a resource this account holds answers with the modelled response
/// and releases exactly that record — every read path stops reporting it, and every
/// other record the account holds is reported as before.
#[tokio::test]
async fn deleting_a_data_source_releases_that_record_and_no_other() {
    let emulator = Emulator::new();
    let table = rows();
    let held = seeded(&emulator, &table).await;

    // Every row resolves before any delete runs, so what each assertion below pins is
    // the delete rather than an add that never landed.
    let mut remaining: Vec<(String, String)> = held.clone();
    assert_eq!(
        as_sorted(&inventory(&emulator, "seeded").await),
        as_sorted(&remaining.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>()),
        "case seeded: the inventory reports every added data source"
    );

    for (row, (name, arn)) in table.iter().zip(held.iter()) {
        let dropped = delete(&emulator, name).await;
        assert_eq!(
            dropped.status, 200,
            "case {:?}: expected 200 from DeleteDirectQueryDataSource, got {} with {:?}",
            row.case, dropped.status, dropped.body
        );
        // This operation's output targets `smithy.api#Unit`, so the answer carries no
        // members: the wire path serves it with no body at all.
        assert!(
            dropped.body.is_null()
                || dropped
                    .body
                    .as_object()
                    .is_some_and(|members| members.is_empty()),
            "case {:?}: expected an answer carrying no members, got {:?}",
            row.case,
            dropped.body
        );

        assert_gone(&emulator, name, arn, row.case).await;

        // Only the named record went: the rest of the account's set is reported exactly
        // as it was, tags and all.
        remaining.retain(|(held_name, _)| held_name != name);
        assert_eq!(
            as_sorted(&inventory(&emulator, row.case).await),
            as_sorted(
                &remaining
                    .iter()
                    .map(|(held_name, _)| held_name.clone())
                    .collect::<Vec<_>>()
            ),
            "case {:?}: a delete removes one record and leaves the rest standing",
            row.case
        );
        for (other, other_arn) in &remaining {
            let read = get(&emulator, other).await;
            assert_eq!(
                read.status, 200,
                "case {:?}: the data source {other:?} another delete did not name still \
                 resolves, got {} with {:?}",
                row.case, read.status, read.body
            );
            let other_row = table
                .iter()
                .find(|candidate| candidate.name == *other)
                .expect("every remaining name belongs to a row of the table");
            assert_eq!(
                stored_tags(&emulator, other_arn, row.case).await,
                as_set(&other_row.tags),
                "case {:?}: the tags of {other:?} are untouched by another delete",
                row.case
            );
        }
    }

    assert!(
        inventory(&emulator, "drained").await.is_empty(),
        "case drained: an account whose every data source was released holds none"
    );
}

/// The same names sorted, so an assertion compares SETS rather than one order.
fn as_sorted(names: &[String]) -> Vec<String> {
    let mut sorted = names.to_vec();
    sorted.sort();
    sorted
}

/// RULE: a name the account holds no such resource under names nothing to delete, so
/// the call is refused as the value it is and nothing is released.
#[tokio::test]
async fn a_name_that_names_no_data_source_is_refused_and_deletes_nothing() {
    let emulator = Emulator::new();
    let table = rows();
    let held = seeded(&emulator, &table).await;
    let before = as_sorted(&inventory(&emulator, "seeded").await);

    // A data source this account did hold, released before the rows below run: its name
    // was minted here and names nothing by the time the second delete asks for it.
    let (released, released_arn) = held[0].clone();
    assert_eq!(
        delete(&emulator, &released).await.status,
        200,
        "case released: the first delete is served"
    );
    assert_gone(&emulator, &released, &released_arn, "released").await;

    // One row per shape of well-formed name the account holds nothing under: the
    // shortest the constraint allows, the longest, a digits-and-underscores one, a held
    // name with one character changed, and the name the first delete already released.
    let (held_name, _) = held[1].clone();
    let absent: Vec<(String, String)> = vec![
        ("the 3-character lower bound".to_string(), "abc".to_string()),
        ("the 80-character upper bound".to_string(), format!("z{}", "9".repeat(79))),
        (
            "digits and underscores only after the first letter".to_string(),
            "a_0_1_2".to_string(),
        ),
        (
            "a held name with its last character changed".to_string(),
            format!("{}x", &held_name[..held_name.len() - 1]),
        ),
        (
            "a data source this account already released".to_string(),
            released.clone(),
        ),
    ];

    for (case, name) in absent {
        let dropped = delete(&emulator, &name).await;
        assert_eq!(
            dropped.status, 400,
            "case {case:?}: expected 400 for a name that names nothing, got {} with {:?}",
            dropped.status, dropped.body
        );
        assert_eq!(
            error_code(&dropped),
            "ValidationException",
            "case {case:?}: expected the modelled refusal, got {:?}",
            dropped.body
        );
        assert_eq!(
            error_message(&dropped),
            "Data source not found",
            "case {case:?}: expected the refusal the service words for this condition, got {:?}",
            error_message(&dropped)
        );
    }

    // Every record the account still holds is exactly the set the released one left
    // behind: a refused delete released nothing of its own.
    let mut expected = before;
    expected.retain(|name| *name != released);
    assert_eq!(
        as_sorted(&inventory(&emulator, "untouched").await),
        expected,
        "case untouched: a refused delete leaves this account's data sources as they were"
    );
}

/// RULE: what the record held goes with the record, so the name it occupied can be used
/// again and the add that takes it mints a NEW resource rather than reviving one.
#[tokio::test]
async fn a_released_name_can_be_added_again_as_a_new_data_source() {
    let emulator = Emulator::new();

    for row in rows() {
        let added = add(&emulator, row.body()).await;
        assert_eq!(
            added.status, 200,
            "case {:?}: expected 200 from AddDirectQueryDataSource, got {} with {:?}",
            row.case, added.status, added.body
        );
        let arn = expected_arn(&row.name);

        // The name is this account's for as long as it holds the data source: a second
        // add of it is refused rather than becoming a second record.
        let clash = add(&emulator, row.body()).await;
        assert_eq!(
            error_code(&clash),
            "ValidationException",
            "case {:?}: the name is taken while the data source is held, got {:?}",
            row.case,
            clash.body
        );

        assert_eq!(
            delete(&emulator, &row.name).await.status,
            200,
            "case {:?}: the delete is served",
            row.case
        );
        assert_gone(&emulator, &row.name, &arn, row.case).await;

        // The name is free, and what it is added with now is what a read reports: none
        // of the released record's description, collections, policy or tags come back.
        let again = add(
            &emulator,
            json!({
                "DataSourceName": row.name,
                "DataSourceType": prometheus(ROLE_ARN, WORKSPACE_ARN),
            }),
        )
        .await;
        assert_eq!(
            again.status, 200,
            "case {:?}: the name a released data source held can be used again, got {} with {:?}",
            row.case, again.status, again.body
        );
        let read = get(&emulator, &row.name).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the new data source resolves, got {} with {:?}",
            row.case, read.status, read.body
        );
        assert_eq!(
            read.body["DataSourceType"],
            prometheus(ROLE_ARN, WORKSPACE_ARN),
            "case {:?}: the new data source carries the type IT was added with, got {:?}",
            row.case,
            read.body
        );
        for member in ["Description", "OpenSearchArns", "DataSourceAccessPolicy"] {
            assert!(
                read.body[member].is_null(),
                "case {:?}: the new data source carries no {member} of the released one, got {:?}",
                row.case,
                read.body[member]
            );
        }
        assert!(
            stored_tags(&emulator, &arn, row.case).await.is_empty(),
            "case {:?}: the new data source carries none of the released one's tags",
            row.case
        );

        // Leave the account empty for the next row.
        assert_eq!(
            delete(&emulator, &row.name).await.status,
            200,
            "case {:?}: the second delete is served",
            row.case
        );
    }
}

/// RULE: the bound this account's store is kept under counts the records it holds, so a
/// released record frees exactly one slot in it — one, and no more.
#[tokio::test]
async fn a_released_data_source_frees_the_slot_it_took_in_the_accounts_bound() {
    let emulator = Emulator::new();

    let mut names = Vec::new();
    for index in 0..DIRECT_QUERY_DATA_SOURCE_QUOTA {
        let name = format!("bound_source_{index}");
        let added = add(
            &emulator,
            json!({
                "DataSourceName": name,
                "DataSourceType": prometheus(ROLE_ARN, WORKSPACE_ARN),
            }),
        )
        .await;
        assert_eq!(
            added.status, 200,
            "case fill {index}: expected 200, got {} with {:?}",
            added.status, added.body
        );
        names.push(name);
    }

    let one_too_many = json!({
        "DataSourceName": "one_too_many",
        "DataSourceType": prometheus(ROLE_ARN, WORKSPACE_ARN),
    });
    let refused = add(&emulator, one_too_many.clone()).await;
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "case one_too_many: a full account refuses another add, got {:?}",
        refused.body
    );

    assert_eq!(
        delete(&emulator, &names[0]).await.status,
        200,
        "case free_a_slot: the delete is served"
    );

    let served = add(&emulator, one_too_many).await;
    assert_eq!(
        served.status, 200,
        "case one_too_many: the slot the released data source took is free, got {} with {:?}",
        served.status, served.body
    );
    let next = add(
        &emulator,
        json!({
            "DataSourceName": "two_too_many",
            "DataSourceType": prometheus(ROLE_ARN, WORKSPACE_ARN),
        }),
    )
    .await;
    assert_eq!(
        error_code(&next),
        "LimitExceededException",
        "case two_too_many: one release frees one slot and no more, got {:?}",
        next.body
    );
}
