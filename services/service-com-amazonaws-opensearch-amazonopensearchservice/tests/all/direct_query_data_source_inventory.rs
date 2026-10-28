// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListDirectQueryDataSources` — the inventory of an ACCOUNT's
//! direct query data sources.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the tagging helpers from
//! [`super::resource_tags`]; this suite adds no second copy of either, and the add
//! and read paths it drives are the ones [`super::direct_query_data_sources`]
//! covers on their own.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the inventory reports every data source the account holds, each row projecting
//!   the SAME stored record a single read reports — member for member, over a table
//!   of data sources differing in the union member they carry, in whether they name
//!   a description, OpenSearch collections or tags, and in how many of each — and
//!   it reports the rows the element shape declares and no others;
//! * the inventory is paged: a page carries at most the service's page size, the
//!   `NextToken` it answers names the data source the next page starts at, the last
//!   page carries none, and the pages concatenated are the whole inventory once
//!   each in the order the account holds it;
//! * a `NextToken` the operation never issued names no place in the inventory, so it
//!   is refused as the malformed value it is rather than served as an empty page,
//!   while the EMPTY token starts from the first page;
//! * reporting the inventory changes nothing: two listings of one account agree, and
//!   a read taken after a listing reports what it did before.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::DIRECT_QUERY_DATA_SOURCE_QUOTA;

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX, REGION};
use super::resource_tags::{as_set, error_message, tag, wire_tags};

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

/// `ListDirectQueryDataSources`:
/// `GET /2021-01-01/opensearch/directQueryDataSource`, whose only input member is
/// `NextToken`, bound to the `nexttoken` query parameter
/// (`smithy_query(query="operation",operation="ListDirectQueryDataSources")`).
async fn list(emulator: &Emulator, query: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/directQueryDataSource{query}"),
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
/// `DirectQueryDataSourceRoleArn`'s pattern
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

/// The members `DirectQueryDataSource` declares — "the configuration details for a
/// data source that can be directly queried"
/// (`smithy_query(query="shape",shape="DirectQueryDataSource")`) — and the five of
/// them `GetDirectQueryDataSource` reports too, which is what lets a row be compared
/// to that read member for member.
const SHARED_MEMBERS: [&str; 5] = [
    "DataSourceName",
    "DataSourceType",
    "Description",
    "OpenSearchArns",
    "DataSourceArn",
];

/// One data source the account holds, and therefore one row of the inventory.
struct Row {
    /// What the case is called when an assertion fails.
    case: &'static str,
    /// The `DataSourceName` the add carries, which is also the name that addresses
    /// the record afterwards.
    name: String,
    /// The `DataSourceType` union, as the wire carries it.
    data_source_type: Value,
    /// The `Description` to send, absent where `None`.
    description: Option<String>,
    /// The `OpenSearchArns` to send, absent where `None`.
    open_search_arns: Option<Vec<String>>,
    /// The `TagList` to send, empty where the add names none.
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

/// The inventory table: one row per way a data source the listing reports differs
/// from another — which member of the union it carries, whether it names a
/// description at all / an empty one / a long one, no collections / one / two, and no
/// tags / one / two. The names are deliberately NOT in the order the rows are added
/// in, so the order the listing reports is asserted rather than the order of the
/// adds.
///
/// `DirectQueryDataSourceName` is `@length(min: 3, max: 80)` with pattern
/// `^[a-z][a-z0-9_]+$` and `DirectQueryDataSourceDescription` is `@length(min: 0,
/// max: 1000)` (`smithy_query(query="operation",operation="AddDirectQueryDataSource")`),
/// so every row is inside the modelled constraints and the service itself is what
/// accepts it.
fn inventory_rows() -> Vec<Row> {
    vec![
        Row {
            case: "security lake, two collections, two tags",
            name: "m_security_lake".to_string(),
            data_source_type: security_lake(ROLE_ARN),
            description: Some("Lake reads for the shared account.".to_string()),
            open_search_arns: Some(vec![
                COLLECTION_ARN.to_string(),
                SECOND_COLLECTION_ARN.to_string(),
            ]),
            tags: vec![tag("project", "trinity"), tag("env", "prod")],
        },
        Row {
            case: "prometheus naming no collection, no description, no tags",
            name: "a_prometheus".to_string(),
            data_source_type: prometheus(ROLE_ARN, WORKSPACE_ARN),
            description: None,
            open_search_arns: None,
            tags: Vec::new(),
        },
        Row {
            case: "the shortest name, cloudwatch logs, one collection",
            name: "zcw".to_string(),
            data_source_type: cloud_watch(ROLE_ARN),
            description: Some(String::new()),
            open_search_arns: Some(vec![COLLECTION_ARN.to_string()]),
            tags: vec![tag("k", "")],
        },
        Row {
            case: "the longest name, cloudwatch logs, the longest description",
            name: format!("f_cloud_watch_{}", "x".repeat(66)),
            data_source_type: cloud_watch(ROLE_ARN),
            description: Some("d".repeat(1000)),
            open_search_arns: Some(vec![SECOND_COLLECTION_ARN.to_string()]),
            tags: Vec::new(),
        },
    ]
}

/// Add `row` and assert the add was served.
async fn added(emulator: &Emulator, body: Value, case: &str) {
    let answer = add(emulator, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from AddDirectQueryDataSource, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The `DirectQueryDataSources` of a served listing answer.
fn reported(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from ListDirectQueryDataSources, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DirectQueryDataSources"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a DirectQueryDataSources list, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The names a listing answer reports, in the order it reports them.
fn names(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|row| row["DataSourceName"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The `TagList` one row carries, sorted by key — what [`as_set`] compares against.
/// `reported_tags` reads the member off a whole answer, which an element of a listing
/// is not, so the pairs are taken off the row here.
fn row_tags(row: &Value, case: &str) -> Vec<(String, String)> {
    let mut tags: Vec<(String, String)> = row["TagList"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|entry| {
                    (
                        entry["Key"].as_str().unwrap_or_default().to_owned(),
                        entry["Value"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_else(|| {
            assert!(
                row["TagList"].is_null(),
                "case {case}: expected TagList to be a list or absent, got {:?}",
                row["TagList"]
            );
            Vec::new()
        });
    tags.sort();
    tags
}

/// Every row of the inventory, read by following the token from page to page, with
/// the paging rule asserted on the way: a page is served, no page but the last
/// carries a token, and the token names the data source the next page starts at.
/// Hands back the rows and how many pages carried them.
async fn whole_inventory(emulator: &Emulator, case: &str) -> (Vec<Value>, usize) {
    let mut rows: Vec<Value> = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            None => String::new(),
            Some(value) => format!("?nexttoken={value}"),
        };
        let answer = list(emulator, &query).await;
        let page = reported(&answer, case);
        pages += 1;
        token = answer.body["NextToken"].as_str().map(str::to_owned);
        if let Some(next) = &token {
            assert!(
                !page.is_empty(),
                "case {case} (page {pages}): a page that carries a NextToken reports rows, \
                 got {:?}",
                answer.body
            );
            assert_eq!(
                names(&page).len(),
                page.len(),
                "case {case} (page {pages}): every row reports a name, got {page:?}"
            );
            assert!(
                !names(&rows).contains(next) && !names(&page).contains(next),
                "case {case} (page {pages}): expected NextToken to name a data source no \
                 earlier page reported, got {next:?}"
            );
        }
        rows.extend(page);
        let Some(next) = token.clone() else { break };
        // The token names the place the next page resumes from, so the next page must
        // start exactly there.
        let resumed = list(emulator, &format!("?nexttoken={next}")).await;
        let first = reported(&resumed, case);
        assert_eq!(
            names(&first).first().map(String::as_str),
            Some(next.as_str()),
            "case {case} (page {pages}): expected the next page to start at {next:?}, got {:?}",
            names(&first)
        );
        assert!(
            pages <= DIRECT_QUERY_DATA_SOURCE_QUOTA,
            "case {case}: paging did not end within {DIRECT_QUERY_DATA_SOURCE_QUOTA} pages"
        );
    }
    (rows, pages)
}

/// RULE: the inventory reports every data source the account holds, each row
/// projecting the same stored record a single read reports, member for member — and
/// only the members the element shape declares.
#[tokio::test]
async fn the_inventory_reports_every_stored_data_source_as_its_own_read_does() {
    let emulator = Emulator::new();

    // An account that holds nothing has an EMPTY inventory rather than none, and it
    // ends the listing there.
    let empty = list(&emulator, "").await;
    let page = reported(&empty, "an account holding no data source");
    assert!(
        page.is_empty(),
        "case an account holding no data source: expected no rows, got {page:?}"
    );
    assert!(
        empty.body["NextToken"].is_null(),
        "case an account holding no data source: an ended listing carries no NextToken, got {:?}",
        empty.body["NextToken"]
    );

    // Each add makes the inventory one row longer, so the listing is asserted at
    // every size from one up to the whole table rather than only at the end.
    let rows = inventory_rows();
    for (added_count, row) in rows.iter().enumerate() {
        added(&emulator, row.body(), row.case).await;

        let (reported_rows, _) = whole_inventory(&emulator, row.case).await;
        let mut expected: Vec<String> = rows[..=added_count]
            .iter()
            .map(|row| row.name.clone())
            .collect();
        expected.sort();
        assert_eq!(
            names(&reported_rows),
            expected,
            "case {:?}: expected the inventory to report every data source added so far, in the \
             order the account holds them",
            row.case
        );

        for added_row in &rows[..=added_count] {
            let case = added_row.case;
            let listed = reported_rows
                .iter()
                .find(|listed| listed["DataSourceName"].as_str() == Some(added_row.name.as_str()))
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a row for {:?}", added_row.name)
                });

            // The five members the element shape shares with the single read come off
            // ONE stored record, so a row and that read cannot disagree about any of
            // them.
            let read = get(&emulator, &added_row.name).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected 200 from GetDirectQueryDataSource, got {} with {:?}",
                read.status, read.body
            );
            for member in SHARED_MEMBERS {
                assert_eq!(
                    listed[member], read.body[member],
                    "case {case}: expected the listing's {member} to be the read's, got {:?} \
                     against {:?}",
                    listed[member], read.body[member]
                );
            }

            assert_eq!(
                listed["DataSourceArn"].as_str(),
                Some(expected_arn(&added_row.name).as_str()),
                "case {case}: expected the row to carry this account's ARN for the data source, \
                 got {:?}",
                listed["DataSourceArn"]
            );
            assert_eq!(
                listed["Description"].as_str().map(str::to_owned),
                added_row.description,
                "case {case}: expected the description the add carried, got {:?}",
                listed["Description"]
            );
            assert_eq!(
                listed["OpenSearchArns"].as_array().map(|arns| arns
                    .iter()
                    .map(|arn| arn.as_str().unwrap_or_default().to_owned())
                    .collect::<Vec<String>>()),
                added_row.open_search_arns,
                "case {case}: expected the collections the add named, got {:?}",
                listed["OpenSearchArns"]
            );
            assert_eq!(
                row_tags(listed, case),
                as_set(&added_row.tags),
                "case {case}: expected the tags the add carried, got {:?}",
                listed["TagList"]
            );

            // `DirectQueryDataSource` declares no access policy, so the inventory
            // reports none — the document is the single read's answer.
            assert!(
                listed["DataSourceAccessPolicy"].is_null(),
                "case {case}: expected the row to carry no DataSourceAccessPolicy, got {:?}",
                listed["DataSourceAccessPolicy"]
            );
        }
    }
}

/// RULE: the inventory is paged — a page carries at most the service's page size, the
/// token it answers names where the next page resumes, the last page carries none,
/// and the pages concatenated are the whole inventory once each.
#[tokio::test]
async fn the_inventory_is_paged_and_the_pages_concatenated_are_the_whole_set() {
    let emulator = Emulator::new();

    // The account is filled to its bound, which is the largest inventory any listing
    // of it can have to page through.
    let mut expected: Vec<String> = Vec::new();
    for index in 0..DIRECT_QUERY_DATA_SOURCE_QUOTA {
        let name = format!("paged_source_{index:02}");
        let case = "a full account";
        added(
            &emulator,
            json!({
                "DataSourceName": name,
                "DataSourceType": prometheus(ROLE_ARN, WORKSPACE_ARN),
            }),
            case,
        )
        .await;
        expected.push(name);
    }
    expected.sort();

    let (rows, pages) = whole_inventory(&emulator, "a full account").await;
    assert_eq!(
        names(&rows),
        expected,
        "case a full account: expected every data source once, in the order the account holds \
         them, got {:?}",
        names(&rows)
    );
    assert!(
        pages >= 2,
        "case a full account: expected the page size to be smaller than the account's bound, so \
         {DIRECT_QUERY_DATA_SOURCE_QUOTA} data sources span more than one page, got {pages}"
    );

    // Every page but the last is one page size, and no page is larger than the first:
    // a caller that ignores the token cannot mistake a first page for the whole
    // inventory.
    let first = reported(&list(&emulator, "").await, "a full account");
    assert!(
        first.len() < expected.len(),
        "case a full account: expected the first page to be smaller than the inventory, got {} \
         of {}",
        first.len(),
        expected.len()
    );
    assert_eq!(
        names(&first),
        expected[..first.len()].to_vec(),
        "case a full account: expected the first page to start the inventory, got {:?}",
        names(&first)
    );

    // The last page ends the listing, so it carries no token at all.
    let last_start = expected[expected.len() - 1].clone();
    let last = list(&emulator, &format!("?nexttoken={last_start}")).await;
    let last_rows = reported(&last, "the last page");
    assert_eq!(
        names(&last_rows),
        vec![last_start.clone()],
        "case the last page: expected the one data source left, got {:?}",
        names(&last_rows)
    );
    assert!(
        last.body["NextToken"].is_null(),
        "case the last page: an ended listing carries no NextToken, got {:?}",
        last.body["NextToken"]
    );
}

/// RULE: a `NextToken` is only ever a value the operation handed out, so one naming
/// no place in the inventory is refused rather than served as an empty page — while
/// the empty token, the one value a client can send without having been given it,
/// starts from the first page.
#[tokio::test]
async fn a_token_the_listing_never_issued_is_refused_and_the_empty_token_starts_over() {
    let emulator = Emulator::new();
    for row in inventory_rows() {
        added(&emulator, row.body(), row.case).await;
    }

    for (token, case) in [
        ("not_a_token", "a token nobody minted"),
        (
            "00000000-0000-0000-0000-000000000000",
            "a GUID-shaped token naming no data source",
        ),
        ("zcw_absent", "a name no data source of the account carries"),
        ("12", "a token that is not a name at all"),
    ] {
        let answer = list(&emulator, &format!("?nexttoken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
        let message = error_message(&answer);
        assert!(
            message.contains("NextToken"),
            "case {case}: expected the refusal to name the member it is about, got {message:?}"
        );
    }

    // The EMPTY token names no place either, so it is the first page rather than a
    // refusal.
    let from_empty = list(&emulator, "?nexttoken=").await;
    let first = reported(&list(&emulator, "").await, "no token");
    assert_eq!(
        reported(&from_empty, "the empty token"),
        first,
        "case the empty token: expected the first page, got {:?}",
        from_empty.body
    );
}

/// RULE: reporting the inventory changes nothing a later caller sees.
#[tokio::test]
async fn listing_the_inventory_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let rows = inventory_rows();
    for row in &rows {
        added(&emulator, row.body(), row.case).await;
    }

    let before: Vec<Value> = {
        let mut reads = Vec::new();
        for row in &rows {
            reads.push(get(&emulator, &row.name).await.body);
        }
        reads
    };

    let (first, first_pages) = whole_inventory(&emulator, "the first listing").await;
    let (second, second_pages) = whole_inventory(&emulator, "the second listing").await;
    assert_eq!(
        first, second,
        "case two listings: expected the second listing to report what the first did, got {:?} \
         against {:?}",
        names(&first),
        names(&second)
    );
    assert_eq!(
        first_pages, second_pages,
        "case two listings: expected the same number of pages, got {first_pages} against \
         {second_pages}"
    );

    for (row, was) in rows.iter().zip(before) {
        let now = get(&emulator, &row.name).await;
        assert_eq!(
            now.body, was,
            "case {:?}: expected the read after the listings to report what it did before, got \
             {:?} against {was:?}",
            row.case, now.body
        );
    }
}
