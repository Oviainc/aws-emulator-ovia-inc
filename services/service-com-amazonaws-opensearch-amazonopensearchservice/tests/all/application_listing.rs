// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListApplications`: the listing of the OpenSearch UI
//! applications one account holds.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`]
//! and the application a row reports from [`super::data_source_attachments`]; this suite
//! adds no second copy of either, and leaves [`super::applications`] to assert the create
//! and the single read.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the listing reports every application the account holds, each row from the SAME
//!   stored record its own `GetApplication` answers with, over a table of accounts
//!   differing in how many applications they hold (none, one, several) and in what each
//!   create carried (a name at either published bound, data sources, configuration
//!   settings, tags, a customer-managed key);
//! * `maxResults` bounds the page and `nextToken` names the application the next page
//!   starts at, so the pages of any page size concatenate to exactly the whole listing,
//!   in one order, with no application reported twice and no token on the last page;
//! * `statuses` selects the applications in the statuses it names, and the clock moving
//!   an application moves which filter selects it — the listing materialises what the
//!   clock owes an application just as a single read does;
//! * a change to an application moves what the listing reports for it, because both the
//!   listing and the read answer from the one stored record;
//! * a page size outside the published range, a status outside the published set and a
//!   `nextToken` the listing never issued are each refused with the modelled
//!   `ValidationException`, and the listing itself is left alone.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::APPLICATION_STATUS_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    APPLICATION_STATUS_ACTIVE, APPLICATION_STATUS_CREATING,
};

use super::data_source_attachments::application;
use super::domain_lifecycle::{error_code, string_member, Answer, Emulator, PREFIX, SETTLE_WAIT};

/// A customer-managed key ARN satisfying `KmsKeyArn`'s own pattern, for a create that
/// names one.
const KMS_KEY: &str = "arn:aws:kms:us-east-1:123456789012:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// The members `ApplicationSummary` and `GetApplicationResponse` share
/// (`smithy_query(query="shape",shape="ApplicationSummary")`), so a row and the read of
/// the same application are compared across all of them.
const SHARED_MEMBERS: &[&str] = &[
    "id",
    "arn",
    "name",
    "endpoint",
    "status",
    "createdAt",
    "lastUpdatedAt",
];

/// `ListApplications`: `GET /2021-01-01/opensearch/list-applications`, with `query`
/// carrying whatever the case asks for (`&maxResults=…`, `&statuses=…`, `&nextToken=…`).
async fn list_applications(emulator: &Emulator, query: &str) -> Answer {
    let query = query.strip_prefix('&').unwrap_or(query);
    let uri = if query.is_empty() {
        format!("{PREFIX}/opensearch/list-applications")
    } else {
        format!("{PREFIX}/opensearch/list-applications?{query}")
    };
    emulator.call("GET", &uri, None).await
}

/// `CreateApplication`: `POST /2021-01-01/opensearch/application`.
async fn create_application(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/application"),
            Some(body),
        )
        .await
}

/// `GetApplication`: `GET /2021-01-01/opensearch/application/{id}`.
async fn get_application(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call("GET", &format!("{PREFIX}/opensearch/application/{id}"), None)
        .await
}

/// `UpdateApplication`: `PUT /2021-01-01/opensearch/application/{id}`.
async fn update_application(emulator: &Emulator, id: &str, body: Value) -> Answer {
    emulator
        .call(
            "PUT",
            &format!("{PREFIX}/opensearch/application/{id}"),
            Some(body),
        )
        .await
}

/// A timestamp member of a response body, as `restJson1` carries one (epoch seconds),
/// with the case named on failure.
fn timestamp_member(body: &Value, member: &str, case: &str) -> f64 {
    body[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a timestamp {member}, got {body:?}"))
}

/// The rows a listing answer reports, in the order it reported them.
fn listed(answer: &Answer, case: &str) -> Vec<Value> {
    answer.body["ApplicationSummaries"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected an ApplicationSummaries list, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `id`s a listing answer reports, in the order it reported them.
fn listed_ids(answer: &Answer, case: &str) -> Vec<String> {
    listed(answer, case)
        .iter()
        .map(|row| string_member(row, "id", case))
        .collect()
}

/// RULE: the listing reports every application the account holds, each row from the SAME
/// stored record a single read of that application answers with.
#[tokio::test]
async fn the_listing_reports_every_application_from_the_stored_record() {
    // (case, the creates the account holds). Each row differs along what the rule
    // generalises over: how many applications the account holds and what each create
    // carried — the listing reports the record, whatever shape the create had.
    let rows: Vec<(&str, Vec<Value>)> = vec![
        ("an account holding no application", vec![]),
        (
            "one application, named at the 3-character lower bound",
            vec![json!({ "name": "ui1" })],
        ),
        (
            "two applications, one carrying data sources",
            vec![
                json!({ "name": "abcdefghij-klmnopqrst-uvwxyz12" }),
                json!({
                    "name": "search-ui",
                    "dataSources": [{
                        "dataSourceArn": "arn:aws:es:us-east-1:123456789012:domain/logs",
                        "dataSourceDescription": "the logs domain",
                    }],
                }),
            ],
        ),
        (
            "several applications of different shapes",
            vec![
                json!({ "name": "plain-ui" }),
                json!({
                    "name": "tagged-ui",
                    "tagList": [{ "Key": "team", "Value": "search" }],
                }),
                json!({ "name": "keyed-ui", "kmsKeyArn": KMS_KEY }),
                json!({
                    "name": "configured-ui",
                    "appConfigs": [{
                        "key": "opensearchDashboards.dashboardAdmin.groups",
                        "value": "operators",
                    }],
                }),
                json!({ "name": "logs9", "dataSources": [], "appConfigs": [] }),
            ],
        ),
    ];

    for (case, creates) in rows {
        let emulator = Emulator::new();
        let mut created: Vec<String> = Vec::new();
        for body in &creates {
            let answer = create_application(&emulator, body.clone()).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200 from CreateApplication({body:?}), got {} with {:?}",
                answer.status, answer.body
            );
            created.push(string_member(&answer.body, "id", case));
        }

        // Nothing asked for a move, so the clock is left to settle every application it
        // owes one: the listing and the reads below then report one stable status, and
        // what is compared is the record rather than the instant it was read at.
        tokio::time::sleep(SETTLE_WAIT).await;

        let answer = list_applications(&emulator, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from ListApplications, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            listed_ids(&answer, case),
            created,
            "case {case:?}: expected the applications the account holds, in the order they \
             were created, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["nextToken"],
            Value::Null,
            "case {case:?}: a listing that ended carries no token, got {:?}",
            answer.body
        );

        for row in listed(&answer, case) {
            let id = string_member(&row, "id", case);
            let read = get_application(&emulator, &id).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected 200 from GetApplication({id}), got {} with {:?}",
                read.status, read.body
            );
            for member in SHARED_MEMBERS {
                assert_eq!(
                    row[member], read.body[member],
                    "case {case:?}: expected the row's {member} to be the stored record's, got \
                     {:?} in the listing and {:?} from the read",
                    row[member], read.body[member]
                );
            }
            assert!(
                APPLICATION_STATUS_VALUES.contains(&string_member(&row, "status", case).as_str()),
                "case {case:?}: expected one of the published statuses \
                 {APPLICATION_STATUS_VALUES:?}, got {row:?}"
            );
        }
    }
}

/// RULE: `maxResults` bounds the page and `nextToken` names the application the next page
/// starts at, so the pages of any page size concatenate to exactly the whole listing, in
/// one order, with no application reported twice and no token on the last page.
#[tokio::test]
async fn any_page_size_walks_the_whole_listing_once() {
    let emulator = Emulator::new();
    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        created.push(application(&emulator, &format!("paged-ui-{index}")).await);
    }

    // Every page size from one row to one past the whole listing, the modelled default of
    // zero — the request that asks for no particular size — and the published maximum.
    for page_size in [0usize, 1, 2, 3, 4, 5, 6, 100] {
        let case = format!("page size {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..created.len() + 2 {
            let query = match (&token, page_size) {
                (None, 0) => String::new(),
                (None, size) => format!("maxResults={size}"),
                (Some(next), 0) => format!("nextToken={next}"),
                (Some(next), size) => format!("maxResults={size}&nextToken={next}"),
            };
            let answer = list_applications(&emulator, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case} page {page}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            let ids = listed_ids(&answer, &case);
            let bound = if page_size == 0 {
                created.len()
            } else {
                page_size
            };
            assert!(
                ids.len() <= bound,
                "case {case} page {page}: expected at most {bound} row(s), got {}",
                ids.len()
            );
            walked.extend(ids);
            token = answer.body["nextToken"].as_str().map(str::to_owned);
            if token.is_none() {
                break;
            }
            assert_eq!(
                listed(&answer, &case).len(),
                bound,
                "case {case} page {page}: a page that left more to report is a FULL page, got \
                 {:?}",
                answer.body
            );
        }
        assert_eq!(
            token, None,
            "case {case}: expected the walk to end with no token, got {token:?}"
        );
        assert_eq!(
            walked, created,
            "case {case}: expected the pages to concatenate to the whole listing in one order, \
             got {walked:?}"
        );
    }
}

/// RULE: `statuses` selects the applications in the statuses it names, and the clock
/// moving an application moves which filter selects it — the listing materialises what
/// the clock owes an application just as a single read does.
#[tokio::test]
async fn a_statuses_filter_selects_the_applications_in_those_statuses() {
    let emulator = Emulator::new();
    let mut created: Vec<String> = Vec::new();
    for index in 0..3 {
        created.push(application(&emulator, &format!("filtered-ui-{index}")).await);
    }

    // The instant the creates returned, every application is in the status a create
    // leaves it in, and no filter naming another status selects any of them. Each row is
    // a different query: one status, several together, and none at all.
    let all = created.clone();
    for (case, query, expected) in [
        (
            "the status a create leaves an application in",
            format!("statuses={APPLICATION_STATUS_CREATING}"),
            all.clone(),
        ),
        (
            "the status provisioning has not reached yet",
            format!("statuses={APPLICATION_STATUS_ACTIVE}"),
            vec![],
        ),
        ("updating, which nothing here is", "statuses=UPDATING".to_string(), vec![]),
        ("deleting, which nothing here is", "statuses=DELETING".to_string(), vec![]),
        ("failed, which nothing here is", "statuses=FAILED".to_string(), vec![]),
        (
            "two statuses together select the union",
            format!(
                "statuses={APPLICATION_STATUS_CREATING}&statuses={APPLICATION_STATUS_ACTIVE}"
            ),
            all.clone(),
        ),
        (
            "two statuses neither of which anything is in",
            "statuses=FAILED&statuses=DELETING".to_string(),
            vec![],
        ),
        ("no status at all narrows nothing", String::new(), all.clone()),
    ] {
        let answer = list_applications(&emulator, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            listed_ids(&answer, case),
            expected,
            "case {case:?}: expected the applications the filter {query:?} selects, got {:?}",
            answer.body
        );
    }

    // Nothing asked for a move: the clock owes every application the status provisioning
    // leaves it in, and this listing is a call that materialises it.
    tokio::time::sleep(SETTLE_WAIT).await;

    for (case, query, expected) in [
        (
            "active once provisioning fell due",
            format!("statuses={APPLICATION_STATUS_ACTIVE}"),
            all.clone(),
        ),
        (
            "creating once provisioning fell due",
            format!("statuses={APPLICATION_STATUS_CREATING}"),
            vec![],
        ),
        (
            "the settled status beside one nothing is in",
            format!("statuses={APPLICATION_STATUS_ACTIVE}&statuses=FAILED"),
            all.clone(),
        ),
    ] {
        let answer = list_applications(&emulator, &query).await;
        assert_eq!(
            listed_ids(&answer, case),
            expected,
            "case {case:?}: expected the applications the filter {query:?} selects, got {:?}",
            answer.body
        );
    }

    // The settled status is what the unfiltered listing reports too, and it is the one a
    // single read of the same application reports.
    let whole = list_applications(&emulator, "").await;
    for row in listed(&whole, "the settled listing") {
        let id = string_member(&row, "id", "the settled listing");
        assert_eq!(
            row["status"],
            json!(APPLICATION_STATUS_ACTIVE),
            "case the settled listing: an application whose provisioning fell due is up, got \
             {row:?}"
        );
        let read = get_application(&emulator, &id).await;
        assert_eq!(
            read.body["status"], row["status"],
            "case the settled listing: expected the row's status to be the stored record's, got \
             {:?} from the read",
            read.body
        );
    }
}

/// RULE: a change to an application moves what the listing reports for it, because the
/// listing and the single read answer from the ONE stored record.
#[tokio::test]
async fn a_change_to_an_application_moves_what_the_listing_reports() {
    let emulator = Emulator::new();
    let id = application(&emulator, "changed-ui").await;
    let before = listed(&list_applications(&emulator, "").await, "before the update");
    let before = before
        .first()
        .cloned()
        .expect("the account holds the application just created");

    // One row per shape of change, so what is asserted is that a change reaches the
    // listing rather than that one member does.
    let changes: Vec<(&str, Value)> = vec![
        (
            "a data source linked",
            json!({ "dataSources": [{ "dataSourceArn": "arn:aws:s3:::changed" }] }),
        ),
        (
            "a configuration setting added",
            json!({
                "appConfigs": [{
                    "key": "opensearchDashboards.dashboardAdmin.users",
                    "value": "arn:aws:iam::123456789012:user/ana",
                }],
            }),
        ),
        ("the data sources cleared", json!({ "dataSources": [] })),
    ];

    let mut previous = timestamp_member(&before, "lastUpdatedAt", "before the update");
    for (case, body) in changes {
        let updated = update_application(&emulator, &id, body.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected 200 from UpdateApplication, got {} with {:?}",
            updated.status, updated.body
        );

        let answer = list_applications(&emulator, "").await;
        let rows = listed(&answer, case);
        assert_eq!(
            rows.len(),
            1,
            "case {case:?}: the account still holds one application, got {:?}",
            answer.body
        );
        let read = get_application(&emulator, &id).await;
        for member in SHARED_MEMBERS {
            assert_eq!(
                rows[0][member], read.body[member],
                "case {case:?}: expected the row's {member} to be the stored record's, got {:?} \
                 in the listing and {:?} from the read",
                rows[0][member], read.body[member]
            );
        }
        assert_eq!(
            rows[0]["createdAt"], before["createdAt"],
            "case {case:?}: a change does not move when the application was created, got {:?}",
            rows[0]
        );

        // `lastUpdatedAt` is a member only the listing and the read report, and an update
        // is what moves it: it is never earlier than the one the previous update left.
        let now = timestamp_member(&rows[0], "lastUpdatedAt", case);
        assert!(
            now >= previous,
            "case {case:?}: expected lastUpdatedAt at or after {previous}, got {now}"
        );
        previous = now;
    }
}

/// RULE: a request the listing cannot run — a page size outside the published range, a
/// status outside the published set, or a token it never issued — is refused with the
/// modelled `ValidationException`, and the listing itself is unchanged.
#[tokio::test]
async fn a_request_the_listing_cannot_run_is_refused() {
    let emulator = Emulator::new();
    let mut created: Vec<String> = Vec::new();
    for index in 0..2 {
        created.push(application(&emulator, &format!("refused-ui-{index}")).await);
    }

    let rows: Vec<(&str, String)> = vec![
        // `MaxResults` is `@range(max: 100)`, so one past the maximum is no page size the
        // listing publishes, and neither is any larger value.
        ("a page size one past the published maximum", "maxResults=101".to_string()),
        ("a page size far past the maximum", format!("maxResults={}", i32::MAX)),
        ("a negative page size", "maxResults=-1".to_string()),
        ("a page size far below zero", format!("maxResults={}", i32::MIN)),
        (
            "a status outside the published set",
            "statuses=NOT_A_STATUS".to_string(),
        ),
        (
            "a published status in another case",
            "statuses=active".to_string(),
        ),
        (
            "a published status beside one outside the set",
            format!("statuses={APPLICATION_STATUS_CREATING}&statuses=NOT_A_STATUS"),
        ),
        ("a token nobody minted", "nextToken=nosuchapplication".to_string()),
        (
            // A token is scoped to the listing it was issued under: a token naming an
            // application the filter left out names no place in the filtered listing.
            "a token naming an application the filter left out",
            format!("statuses=FAILED&nextToken={}", created[0]),
        ),
    ];

    for (case, query) in rows {
        let answer = list_applications(&emulator, &query).await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException from {query:?}, got {} with {:?}",
            answer.status,
            answer.body
        );
    }

    let answer = list_applications(&emulator, "").await;
    assert_eq!(
        listed_ids(&answer, "after the refusals"),
        created,
        "case after the refusals: expected the listing a refused request left alone, got {:?}",
        answer.body
    );
}
