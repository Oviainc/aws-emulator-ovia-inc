// SPDX-License-Identifier: CC0-1.0

//! `GetServerDetails`: one server of the account's portfolio, read end to end over HTTP.
//!
//! The rule under test is the one the operation states for ANY server, not for one
//! request: the answer is the stored server record the `serverId` names — reported as the
//! same `ServerDetail` every read of this service reports — beside that server's
//! associated applications, handed out in pages of at most `maxResults` in the stored
//! order, each page carrying a `nextToken` exactly when another application follows it; a
//! `serverId` naming no server is refused with 404 `ResourceNotFoundException`, and a page
//! the operation cannot serve (a `maxResults` outside the documented 1..=100, a token it
//! never issued or one issued for another server) with 400 `ValidationException`.
//!
//! The tables below therefore span the class the rule generalises over rather than one
//! server: every application count from none to nine crossed with every page size from 1
//! to 10 (so a page smaller than, equal to, an exact divisor of and larger than the list
//! all appear), both `maxResults` bounds and the first value outside each, and four kinds
//! of token the operation must refuse.
//!
//! A server enters this service only through an import file task's ingestion of the import
//! file, which is filed separately (the import-loop issue). So these cases seed the
//! inventory records directly through the shared engine — the same `servers` map that
//! ingestion writes and this operation reads — and then drive every assertion through the
//! router, so the handler, its refusals and the serialized `ServerDetail` are exercised
//! exactly as a client reaches them.

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::ops::types::AssociatedApplication;
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::ServerRecord;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{at, call, Answer};
use crate::server_inventory::{emulator_with, server};

/// This service's message for an id that resolves to no resource on an operation that
/// reports the resource as absent, and its message for a value it cannot act on. Spelled
/// out independently of `src/state.rs`'s constants, so a wrong message in the crate fails
/// here rather than agreeing with itself.
const INVALID_RESOURCE_ID: &str = "Invalid resourceId: ";
const INVALID_REQUEST_BODY: &str = "Invalid request body";
const INVALID_OR_MISSING_PARAMETERS: &str = "Looks like you provided invalid or missing parameters.";

/// The documented maximum of `maxResults`: "The maximum number of items to include in the
/// response. The maximum value is 100."
/// (`smithy_query(query="operation",operation="GetServerDetails")` -> `maxResults`).
const MAX_RESULTS_MAX: i64 = 100;

/// The largest application count any case below seeds, and the largest page size they ask
/// for: between them the cross product covers a page smaller than the list, equal to it,
/// an exact divisor of it and larger than it.
const MAX_APPLICATIONS: usize = 9;
const MAX_PAGE_SIZE: usize = 10;

/// One associated application, as the import that brought the server in would have stored
/// it.
///
/// `AssociatedApplication` carries exactly two members —
/// `smithy_query(query="shape",shape="AssociatedApplication")`: `name` "Name of the
/// application as defined in Application Discovery Service" and `id` "ID of the
/// application as defined in Application Discovery Service", each a `String`
/// `@length(0,1024)` `@pattern .*\S.*`.
fn application(server_id: &str, index: usize) -> AssociatedApplication {
    AssociatedApplication {
        name: Some(format!("app-{server_id}-{index}")),
        id: Some(format!("d-application-{server_id}-{index:02}")),
    }
}

/// A server record holding `count` associated applications, and enough other members for
/// the reported `ServerDetail` to be checked against the record rather than against a
/// single field.
///
/// The record's values come from the model's own catalogs
/// (`smithy_query(query="shape",shape="OSType,Strategy,TargetDestination,RunTimeAssessmentStatus")`)
/// and differ per row, so no assertion can pass on a canned detail.
fn server_with(id: &str, count: usize) -> ServerRecord {
    let strategies = ["Rehost", "Replatform", "Refactor", "Retain", "Relocate"];
    let destinations = [
        "Amazon Elastic Cloud Compute (EC2)",
        "AWS Fargate",
        "Amazon Elastic Container Service (ECS)",
        "None specified",
        "Aurora PostgreSQL",
    ];
    let statuses = [
        "dataCollectionTaskSuccess",
        "dataCollectionTaskPartialSuccess",
        "dataCollectionTaskToBeScheduled",
    ];
    ServerRecord {
        name: Some(format!("srv-{id}")),
        server_type: Some(format!("VMware-{count}")),
        os_type: Some(
            if count.is_multiple_of(2) {
                "LINUX"
            } else {
                "WINDOWS"
            }
            .to_string(),
        ),
        os_version: Some(format!("Ubuntu 20.0{}", count % 10)),
        strategy: Some(strategies[count % strategies.len()].to_string()),
        target_destination: Some(destinations[count % destinations.len()].to_string()),
        data_collection_status: Some(statuses[count % statuses.len()].to_string()),
        associated_applications: (0..count).map(|i| application(id, i)).collect(),
        ..server(id)
    }
}

/// The server id a row holding `count` applications is seeded under: a `ResourceId`
/// (`^[0-9a-b]+`), distinct per row.
fn server_id(count: usize) -> String {
    format!("{count}a")
}

/// One emulator holding one server per application count in `0..=MAX_APPLICATIONS`.
fn emulator_with_each_count() -> MultiServiceRouter {
    emulator_with(
        (0..=MAX_APPLICATIONS)
            .map(|count| server_with(&server_id(count), count))
            .collect(),
    )
}

/// `GET /get-server-details/{serverId}`, with the query members the request carries.
async fn get_server_details(
    router: &MultiServiceRouter,
    server_id: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let uri = if query.is_empty() {
        format!("/get-server-details/{server_id}")
    } else {
        format!("/get-server-details/{server_id}?{}", query.join("&"))
    };
    call(router, "GET", &uri, None).await
}

/// The `(name, id)` pairs `associatedApplications` reported, in the order reported.
fn reported_applications(answer: &Answer) -> Vec<(String, String)> {
    match answer.body.get("associatedApplications").and_then(Value::as_array) {
        Some(items) => items
            .iter()
            .map(|item| {
                (
                    item["name"].as_str().unwrap_or_default().to_string(),
                    item["id"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect(),
        None => Vec::new(),
    }
}

/// The `(name, id)` pairs a record holds, in the order it holds them.
fn stored_applications(record: &ServerRecord) -> Vec<(String, String)> {
    record
        .associated_applications
        .iter()
        .map(|app| {
            (
                app.name.clone().unwrap_or_default(),
                app.id.clone().unwrap_or_default(),
            )
        })
        .collect()
}

/// The `serverDetail` the answer carries is the stored record, member for member.
fn assert_detail_is_the_record(what: &str, answer: &Answer, record: &ServerRecord) {
    let detail = at(&answer.body, "/serverDetail");
    for (pointer, expected) in [
        ("/serverDetail/id", Some(record.id.clone())),
        ("/serverDetail/name", record.name.clone()),
        ("/serverDetail/serverType", record.server_type.clone()),
        (
            "/serverDetail/dataCollectionStatus",
            record.data_collection_status.clone(),
        ),
        (
            "/serverDetail/systemInfo/osInfo/type",
            record.os_type.clone(),
        ),
        (
            "/serverDetail/systemInfo/osInfo/version",
            record.os_version.clone(),
        ),
        (
            "/serverDetail/recommendationSet/strategy",
            record.strategy.clone(),
        ),
        (
            "/serverDetail/recommendationSet/targetDestination",
            record.target_destination.clone(),
        ),
    ] {
        assert_eq!(
            answer.string(pointer),
            expected.unwrap_or_default(),
            "case {what:?}: expected {pointer} to be the stored value, got {detail:?}"
        );
    }
}

/// Every stored member of the server comes back, and every application it holds comes back
/// in the stored order, for a read that names no page size.
#[tokio::test]
async fn the_detail_and_the_applications_come_from_the_stored_record() {
    let router = emulator_with_each_count();
    for count in 0..=MAX_APPLICATIONS {
        let record = server_with(&server_id(count), count);
        let what = format!("a server holding {count} associated application(s)");
        let answer = get_server_details(&router, &record.id, None, None).await;

        assert_eq!(
            answer.status, 200,
            "case {what:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_detail_is_the_record(&what, &answer, &record);
        assert_eq!(
            reported_applications(&answer),
            stored_applications(&record),
            "case {what:?}: expected the stored applications in order, got {:?}",
            answer.body.get("associatedApplications")
        );
        // Every list of this fixture is well inside the default page, so nothing follows.
        assert_eq!(
            at(&answer.body, "/nextToken"),
            Value::Null,
            "case {what:?}: expected no nextToken on a page that ended the list, got {:?}",
            answer.body
        );
    }
}

/// Paging the applications of one server: whatever the page size, following the tokens
/// yields the stored list exactly once in order, no page is larger than asked for, a token
/// is present exactly when another application follows, and every page reports the server
/// detail.
#[tokio::test]
async fn following_the_tokens_yields_every_application_exactly_once() {
    let router = emulator_with_each_count();
    for count in 0..=MAX_APPLICATIONS {
        for size in 1..=MAX_PAGE_SIZE {
            let record = server_with(&server_id(count), count);
            let expected = stored_applications(&record);
            let what = format!("{count} application(s) paged {size} at a time");

            let mut collected: Vec<(String, String)> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let answer =
                    get_server_details(&router, &record.id, Some(size as i64), token.as_deref())
                        .await;
                assert_eq!(
                    answer.status, 200,
                    "case {what:?}: page {pages} expected HTTP 200, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_detail_is_the_record(&what, &answer, &record);
                let page = reported_applications(&answer);
                assert!(
                    page.len() <= size,
                    "case {what:?}: page {pages} expected at most {size} items, got {:?}",
                    page
                );
                collected.extend(page);
                let more = at(&answer.body, "/nextToken");
                assert_eq!(
                    more != Value::Null,
                    collected.len() < expected.len(),
                    "case {what:?}: page {pages} expected a nextToken only while applications \
                     remain ({} of {} collected), got {:?}",
                    collected.len(),
                    expected.len(),
                    answer.body
                );
                token = more.as_str().map(str::to_string);
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {what:?}: expected the walk to end, but it made {pages} pages"
                );
                if token.is_none() {
                    break;
                }
            }

            assert_eq!(
                collected, expected,
                "case {what:?}: expected the stored list exactly once in order, got {collected:?}"
            );
            let expected_pages = expected.len().div_ceil(size).max(1);
            assert_eq!(
                pages, expected_pages,
                "case {what:?}: expected {expected_pages} page(s), got {pages}"
            );
        }
    }
}

/// `maxResults` is held to its documented bounds: accepted AT each bound, refused at the
/// first value outside either.
#[tokio::test]
async fn max_results_is_held_to_its_documented_bounds() {
    let router = emulator_with_each_count();
    let id = server_id(MAX_APPLICATIONS);

    for (what, max_results) in [
        ("maxResults of 1, AT the minimum a page can hold", 1),
        ("maxResults of 2, inside the documented range", 2),
        (
            "maxResults of 100, AT the documented maximum",
            MAX_RESULTS_MAX,
        ),
    ] {
        let answer = get_server_details(&router, &id, Some(max_results), None).await;
        assert_eq!(
            answer.status, 200,
            "case {what:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
    }

    for (what, max_results) in [
        ("maxResults of 0, one below the smallest page", 0),
        ("maxResults of -1, a negative page", -1),
        (
            "maxResults of 101, one past the documented maximum",
            MAX_RESULTS_MAX + 1,
        ),
        ("maxResults of 1000, far past the maximum", 1_000),
    ] {
        let answer = get_server_details(&router, &id, Some(max_results), None).await;
        assert_eq!(
            answer.status, 400,
            "case {what:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {what:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.string("/message"),
            INVALID_REQUEST_BODY,
            "case {what:?}: expected the service's {INVALID_REQUEST_BODY:?}, got {:?}",
            answer.body
        );
    }
}

/// A `nextToken` this operation did not issue for THIS server is refused with the
/// service's sentence for an unresolvable parameter — never resolved against another
/// server's list or answered from the top.
#[tokio::test]
async fn a_token_this_server_did_not_issue_is_refused() {
    let router = emulator_with_each_count();
    let id = server_id(MAX_APPLICATIONS);
    let other = server_id(MAX_APPLICATIONS - 1);

    // A token this operation really did issue, for the OTHER server.
    let other_answer = get_server_details(&router, &other, Some(1), None).await;
    let other_token = other_answer.string("/nextToken");
    assert!(
        !other_token.is_empty(),
        "the fixture must hand out a token for server {other:?}, got {:?}",
        other_answer.body
    );

    // And a token `ListServers` issued, for a list of its own.
    let listed = call(&router, "POST", "/list-servers", Some(json!({"maxResults": 1}))).await;
    let list_token = listed.string("/nextToken");
    assert!(
        !list_token.is_empty(),
        "the fixture must hand out a ListServers token, got {:?}",
        listed.body
    );

    for (what, token) in [
        ("a token this service never minted", "not-a-token".to_string()),
        ("well-formed hex that decodes to nothing", "abcdef".to_string()),
        ("this operation's token for ANOTHER server", other_token),
        ("a ListServers token, issued for another list", list_token),
    ] {
        let answer = get_server_details(&router, &id, None, Some(&token)).await;
        assert_eq!(
            answer.status, 400,
            "case {what:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {what:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.string("/message"),
            INVALID_OR_MISSING_PARAMETERS,
            "case {what:?}: expected the service's {INVALID_OR_MISSING_PARAMETERS:?}, got {:?}",
            answer.body
        );
    }
}

/// A `serverId` that names no server of this account's portfolio is reported as an absent
/// resource, whatever the rest of the request asks for.
#[tokio::test]
async fn a_server_id_naming_no_server_is_not_found() {
    let router = emulator_with_each_count();
    for (what, id, max_results) in [
        ("an id no record was seeded under", "ab", None),
        ("another unseeded id, with a page size", "0b", Some(5)),
        ("a one-character unseeded id", "b", Some(1)),
    ] {
        let answer = get_server_details(&router, id, max_results, None).await;
        assert_eq!(
            answer.status, 404,
            "case {what:?}: expected HTTP 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {what:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.string("/message"),
            INVALID_RESOURCE_ID,
            "case {what:?}: expected the service's {INVALID_RESOURCE_ID:?}, got {:?}",
            answer.body
        );
    }
}

/// The read changes nothing a later caller sees: the same request answers the same thing
/// however many times it is made, and walking every page leaves the first page unchanged.
#[tokio::test]
async fn reading_a_server_changes_nothing() {
    let router = emulator_with_each_count();
    for count in [0usize, 1, 4, MAX_APPLICATIONS] {
        let id = server_id(count);
        let what = format!("a server holding {count} associated application(s)");

        let first = get_server_details(&router, &id, None, None).await;
        let again = get_server_details(&router, &id, None, None).await;
        assert_eq!(
            first.body, again.body,
            "case {what:?}: expected two identical reads to answer the same thing, got {:?} then \
             {:?}",
            first.body, again.body
        );

        // Walk every page of one application at a time, then read the whole thing again.
        let mut token: Option<String> = None;
        loop {
            let answer = get_server_details(&router, &id, Some(1), token.as_deref()).await;
            token = at(&answer.body, "/nextToken").as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        let after_walk = get_server_details(&router, &id, None, None).await;
        assert_eq!(
            after_walk.body, first.body,
            "case {what:?}: expected the walk to leave the server unchanged, got {:?} instead of \
             {:?}",
            after_walk.body, first.body
        );
    }
}
