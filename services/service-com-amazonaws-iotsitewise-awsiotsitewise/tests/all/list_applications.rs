// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListApplications`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * the listing enumerates the applications the store HOLDS — every application a
//!   create persisted, once, whatever workspace it was created in, and none a
//!   delete removed;
//! * every summary is read off the stored record, so a listed application reports
//!   what `DescribeApplication` reports for it, the derived status included;
//! * the enumeration is in one stable order, oldest first, cut into pages of at
//!   most `maxResults` entries, with a `nextToken` only while another entry
//!   follows, so the pages joined are the whole enumeration once whatever the page
//!   size;
//! * a `nextToken` this operation never issued is the modelled
//!   `InvalidRequestException` rather than an empty page, and a `maxResults`
//!   outside the modelled range is refused at the boundary;
//! * the listing REPORTS: a read leaves what the next caller sees untouched.
//!
//! The HTTP harness (`call`, `error_code`, `hex_token`, `router`) and the
//! application creates are the ones the sibling suites already have; this suite
//! reuses them rather than growing a second copy.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::ACTIVATION_DELAY;

use crate::application_lifecycle::{create, create_workspace, describe};
use crate::asset_lifecycle::{call, error_code, hex_token, router, REGION};

/// `GET /applications`, as `OPERATIONS.md` reports the route: the listing takes no
/// workspace at all, because an application's id identifies it across every
/// workspace.
const APPLICATIONS: &str = "/applications";

/// The published maximum of `maxResults` on this listing (`@range(min: 1, max:
/// 250)`; `smithy_query(query="operation", operation="ListApplications")`).
const MAX_RESULTS_MAX: i64 = 250;

/// The published minimum of the same range.
const MAX_RESULTS_MIN: i64 = 1;

/// The page size this listing serves when the caller sends no `maxResults`, which
/// is the default every paginated listing of this service shares. The fixture is
/// shorter than it, so an unpaged call answers in ONE page.
const DEFAULT_MAX_RESULTS: usize = 50;

/// One application the fixture creates.
#[derive(Debug)]
struct ApplicationCase {
    /// What a failed assertion names this row by.
    label: &'static str,
    workspace: &'static str,
    name: String,
    description: Option<&'static str>,
    idc_instance_arn: &'static str,
    tags: Option<Value>,
}

/// The rows: the applications the listing has to enumerate, differing along what
/// the enumeration generalises over rather than being copies of one shape — the
/// workspace they live in (several, including one holding more than one and one
/// holding exactly one), the length and punctuation `ApplicationName` admits
/// (smithy_query query="operation" operation="CreateApplication": `name` len
/// 1..256, `^[A-Za-z0-9](?:[A-Za-z0-9 ._()\-]*[A-Za-z0-9._()\-])?$`), a
/// description present and absent, the Identity Center instance ARN forms the `ARN`
/// pattern admits, and tags absent, one and several.
fn application_cases() -> Vec<ApplicationCase> {
    vec![
        ApplicationCase {
            label: "required members only, the shortest name",
            workspace: "plant-one",
            name: "A".to_string(),
            description: None,
            idc_instance_arn: "arn:aws:sso:::instance/ssoins-1111111111111111",
            tags: None,
        },
        ApplicationCase {
            label: "a name carrying spaces and punctuation",
            workspace: "plant-one",
            name: "Line 3 (west) _monitor.v2-final".to_string(),
            description: Some("the west line monitor"),
            idc_instance_arn: "arn:aws:sso::123456789012:instance/ssoins-2222222222222222",
            tags: Some(json!({ "only": "one" })),
        },
        ApplicationCase {
            label: "the longest name the model admits",
            workspace: "plant-one",
            // 256 characters, the longest `ApplicationName` the model admits.
            name: format!("N{}", "a".repeat(255)),
            description: None,
            idc_instance_arn: "arn:aws-us-gov:sso:::instance/ssoins-3333333333333333",
            tags: Some(json!({ "line": "3", "shift": "night", "owner": "controls" })),
        },
        ApplicationCase {
            label: "a second workspace, named at the shortest length",
            workspace: "w",
            name: "Second.Workspace".to_string(),
            description: Some("in the one-character workspace"),
            idc_instance_arn: "arn:aws-cn:sso:::instance/ssoins-4444444444444444",
            tags: None,
        },
        ApplicationCase {
            label: "a third workspace, underscores in its name",
            workspace: "plant_two-3",
            name: "Third_Workspace App.3".to_string(),
            description: None,
            idc_instance_arn: "arn:aws:sso:::identitystore/d-9999999999",
            tags: Some(json!({ "zone": "b" })),
        },
        ApplicationCase {
            label: "a fourth workspace holding exactly one application",
            workspace: "lonely",
            name: "Only-One".to_string(),
            description: Some("alone in its workspace"),
            idc_instance_arn: "arn:aws:sso::123456789012:instance/ssoins-5555555555555555",
            tags: None,
        },
        ApplicationCase {
            label: "a second application of the one-character workspace",
            workspace: "w",
            name: "W.Two".to_string(),
            description: None,
            idc_instance_arn: "arn:aws:sso:::instance/ssoins-6666666666666666",
            tags: Some(json!({ "zone": "a", "owner": "controls" })),
        },
    ]
}

/// A created application of the fixture: what the create answered with, kept so the
/// listing can be held against the store rather than against itself.
#[derive(Debug, Clone)]
struct Created {
    label: &'static str,
    workspace: String,
    name: String,
    id: String,
}

/// Open every workspace the rows name and create every application in them, in row
/// order, so the creation order is known.
async fn fixture(router: &MultiServiceRouter) -> Vec<Created> {
    let cases = application_cases();
    let mut opened: BTreeSet<&str> = BTreeSet::new();
    let mut created: Vec<Created> = Vec::new();

    for case in &cases {
        if opened.insert(case.workspace) {
            create_workspace(router, case.workspace).await;
        }
        let mut body = json!({
            "workspaceName": case.workspace,
            "name": case.name.clone(),
            "idcInstanceArn": case.idc_instance_arn,
        });
        let object = body.as_object_mut().expect("an object body");
        if let Some(description) = case.description {
            object.insert("description".to_string(), json!(description));
        }
        if let Some(tags) = case.tags.clone() {
            object.insert("tags".to_string(), tags);
        }
        let (status, answer) = create(router, body).await;
        assert_eq!(
            status, 200,
            "case {:?}: the create expected 200, got {status} {answer}",
            case.label
        );
        let id = answer["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no id in {answer}", case.label))
            .to_string();
        created.push(Created {
            label: case.label,
            workspace: case.workspace.to_string(),
            name: case.name.clone(),
            id,
        });
    }
    created
}

/// The URI of one page, carrying only the paging members the call asks for.
fn page_uri(page_size: Option<i64>, token: Option<&str>) -> String {
    let mut query: Vec<String> = Vec::new();
    if let Some(size) = page_size {
        query.push(format!("maxResults={size}"));
    }
    if let Some(token) = token {
        query.push(format!("nextToken={token}"));
    }
    if query.is_empty() {
        return APPLICATIONS.to_string();
    }
    format!("{APPLICATIONS}?{}", query.join("&"))
}

/// One page, asserting it was served.
async fn page(
    router: &MultiServiceRouter,
    page_size: Option<i64>,
    token: Option<&str>,
) -> Value {
    let uri = page_uri(page_size, token);
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(status, 200, "{uri}: expected 200, got {status} {body}");
    body
}

/// The summaries one answer reports, in the order it reported them.
fn summaries(body: &Value) -> Vec<Value> {
    body["applications"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an applications list, got {body}"))
        .clone()
}

/// The ids one answer reports, in the order it reported them.
fn ids(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| {
            summary["id"]
                .as_str()
                .unwrap_or_else(|| panic!("expected an id in {summary}"))
                .to_string()
        })
        .collect()
}

/// Walk the whole enumeration at `page_size` (or with no `maxResults` at all when
/// it is `None`): the ids in the order the pages reported them, and how many pages
/// it took.
async fn walk(
    router: &MultiServiceRouter,
    page_size: Option<i64>,
    entries: usize,
) -> (Vec<String>, usize) {
    let case = format!("the walk at {page_size:?}");
    let limit = page_size.unwrap_or(DEFAULT_MAX_RESULTS as i64) as usize;
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let body = page(router, page_size, token.as_deref()).await;
        let reported = ids(&body);
        pages += 1;
        assert!(
            reported.len() <= limit,
            "case {case:?}: a page holds at most {limit} entries, got {} in {body}",
            reported.len()
        );
        seen.extend(reported.iter().cloned());

        token = body["nextToken"].as_str().map(str::to_string);
        match &token {
            // A token is issued only while an entry follows the page, and a page
            // another entry follows is a FULL page.
            Some(_) => assert_eq!(
                reported.len(),
                limit,
                "case {case:?}: a page followed by a nextToken must be full, got {body}"
            ),
            None => break,
        }
        assert!(
            pages <= entries + 1,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
    }
    (seen, pages)
}

/// The whole enumeration in one page: the order every paged walk has to reproduce.
async fn baseline(router: &MultiServiceRouter, entries: usize) -> Vec<String> {
    let (reference, pages) = walk(router, Some(MAX_RESULTS_MAX), entries).await;
    assert_eq!(
        pages, 1,
        "the whole enumeration fits one page of {MAX_RESULTS_MAX}, took {pages}"
    );
    assert_eq!(
        reference.len(),
        entries,
        "expected {entries} entries, got {reference:?}"
    );
    reference
}

#[tokio::test]
async fn every_created_application_is_listed_once_and_agrees_with_describe() {
    let router = router();
    let created = fixture(&router).await;

    let body = page(&router, Some(MAX_RESULTS_MAX), None).await;
    let listed = summaries(&body);
    assert_eq!(
        listed.len(),
        created.len(),
        "every created application is listed exactly once, got {body}"
    );

    for case in &created {
        let summary = listed
            .iter()
            .find(|summary| summary["id"].as_str() == Some(case.id.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the application {} the create minted must be listed, got {body}",
                    case.label, case.id
                )
            });

        // The summary is read off the stored record, so it reports what the describe
        // reports for the same application.
        let (status, described) = describe(&router, &case.workspace, &case.id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the describe expected 200, got {status} {described}",
            case.label
        );
        for member in ["arn", "id", "name", "createdAt", "workspaceName"] {
            assert_eq!(
                summary[member], described[member],
                "case {:?}: the listed {member} must be the one the describe reports, \
                 got {summary} against {described}",
                case.label
            );
        }
        assert_eq!(
            summary["name"].as_str(),
            Some(case.name.as_str()),
            "case {:?}: the listed name must be the one the create stored, got {summary}",
            case.label
        );
        assert_eq!(
            summary["workspaceName"].as_str(),
            Some(case.workspace.as_str()),
            "case {:?}: an application is listed under the workspace it was created in, \
             got {summary}",
            case.label
        );
        // The ARN carries the application's own id, in the region the request was
        // signed for.
        assert_eq!(
            summary["arn"].as_str(),
            Some(
                format!(
                    "arn:aws:iotsitewise:{REGION}:{}:application/{}",
                    engine::EMULATOR_ACCOUNT_ID, case.id
                )
                .as_str()
            ),
            "case {:?}: unexpected ARN in {summary}",
            case.label
        );
    }

    // The enumeration spans every workspace the fixture opened, so no listed
    // application is missing because of the workspace it lives in.
    let listed_workspaces: BTreeSet<String> = listed
        .iter()
        .map(|summary| summary["workspaceName"].as_str().unwrap_or("").to_string())
        .collect();
    let created_workspaces: BTreeSet<String> =
        created.iter().map(|case| case.workspace.clone()).collect();
    assert_eq!(
        listed_workspaces, created_workspaces,
        "the listing takes no workspace, so it spans every one, got {body}"
    );

    // Oldest first: the creation instants the listing reports never decrease.
    let instants: Vec<f64> = listed
        .iter()
        .map(|summary| {
            summary["createdAt"]
                .as_f64()
                .unwrap_or_else(|| panic!("expected a createdAt in {summary}"))
        })
        .collect();
    assert!(
        instants.windows(2).all(|pair| pair[0] <= pair[1]),
        "the enumeration is oldest first, got {instants:?}"
    );
}

#[tokio::test]
async fn the_enumeration_pages_the_same_whatever_the_page_size() {
    let router = router();
    let created = fixture(&router).await;
    let entries = created.len();
    let reference = baseline(&router, entries).await;

    // Rows differ in page size: the published minimum, sizes that divide the
    // enumeration and sizes that do not, exactly it, one past it, the published
    // maximum, and no `maxResults` at all (the default this service serves).
    let page_sizes = [
        Some(MAX_RESULTS_MIN),
        Some(2),
        Some(3),
        Some(entries as i64),
        Some(entries as i64 + 1),
        Some(MAX_RESULTS_MAX),
        None,
    ];
    for page_size in page_sizes {
        let case = format!("maxResults={page_size:?}");
        let (seen, pages) = walk(&router, page_size, entries).await;
        assert_eq!(
            seen, reference,
            "case {case:?}: the pages joined must be the whole enumeration once, in one \
             order, got {seen:?}"
        );
        let expected_pages =
            entries.div_ceil(page_size.unwrap_or(DEFAULT_MAX_RESULTS as i64) as usize);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages over {entries} entries, \
             walked {pages}"
        );
    }
}

#[tokio::test]
async fn a_token_resumes_at_the_entry_after_the_page_it_was_issued_with() {
    let router = router();
    let created = fixture(&router).await;
    let entries = created.len();
    let reference = baseline(&router, entries).await;

    // Rows: a resume from each prefix of the enumeration, so the token is exercised
    // at the first page, in the middle and at the last entry that has a successor.
    for taken in 1..entries {
        let case = format!("a resume after {taken} entries");
        let first = page(&router, Some(taken as i64), None).await;
        let token = first["nextToken"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: a page another entry follows carries a token, got {first}")
        });
        // The form every listing of this service issues a token in: hex, which is
        // inside the `^[A-Za-z0-9+/=]+$` the paging members model.
        assert!(
            !token.is_empty()
                && token.len() % 2 == 0
                && token.chars().all(|ch| ch.is_ascii_hexdigit()),
            "case {case:?}: expected an even-length hex token, got {token:?}"
        );
        assert_eq!(
            token,
            hex_token(&reference[taken]),
            "case {case:?}: the token must name the entry the next page resumes at"
        );

        let resumed = page(&router, Some(MAX_RESULTS_MAX), Some(token)).await;
        assert_eq!(
            ids(&resumed),
            reference[taken..].to_vec(),
            "case {case:?}: the rest of the enumeration must follow the token, got {resumed}"
        );
    }
}

#[tokio::test]
async fn a_token_this_listing_never_issued_is_refused_and_changes_nothing() {
    let router = router();
    let created = fixture(&router).await;
    let entries = created.len();
    let reference = baseline(&router, entries).await;

    // Rows differ in HOW the token is wrong: not hex at all, an odd-length hex run,
    // the hex of a well-formed id no application carries, and the hex of text that
    // is no key of this enumeration.
    let forged = [
        ("not hex at all", "zzzz".to_string()),
        ("an odd-length hex run", "abc".to_string()),
        (
            "the hex of an unknown id",
            hex_token("11111111-2222-3333-4444-555555555555"),
        ),
        ("the hex of text that is no key", hex_token("no-such-entry")),
    ];
    for (how, token) in &forged {
        let uri = page_uri(None, Some(token));
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 400,
            "case {how:?}: a forged nextToken must be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {how:?}: unexpected error code in {body}"
        );
    }

    // The refusals changed nothing a later caller sees, and neither did the reads: a
    // listing REPORTS.
    let (again, _) = walk(&router, Some(MAX_RESULTS_MAX), entries).await;
    assert_eq!(
        again, reference,
        "the refusals and the reads must leave the enumeration as it was, got {again:?}"
    );
}

#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let router = router();
    let created = fixture(&router).await;
    let entries = created.len();

    // Each published bound of `maxResults` (range 1..250) and one value past it.
    let cases = [
        ("one below the minimum", MAX_RESULTS_MIN - 1, 400),
        ("the published minimum", MAX_RESULTS_MIN, 200),
        ("the published maximum", MAX_RESULTS_MAX, 200),
        ("one above the maximum", MAX_RESULTS_MAX + 1, 400),
    ];
    for (case, max_results, expected) in cases {
        let uri = page_uri(Some(max_results), None);
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, expected,
            "case {case:?} (maxResults={max_results}): expected {expected}, got {status} {body}"
        );
        if expected == 400 {
            assert!(
                !error_code(&body).is_empty(),
                "case {case:?}: a refusal must carry a modelled error code, got {body}"
            );
        }
    }

    // The refusals persisted nothing: the enumeration is what it was.
    let listed = baseline(&router, entries).await;
    assert_eq!(
        listed.len(),
        entries,
        "the refused page sizes must leave every application listed, got {listed:?}"
    );
}

#[tokio::test]
async fn an_empty_store_lists_no_application_and_issues_no_token() {
    let router = router();
    // No create at all: the listing reports the empty enumeration rather than
    // fabricating an entry, and a required list member is still present.
    for page_size in [None, Some(MAX_RESULTS_MIN), Some(MAX_RESULTS_MAX)] {
        let body = page(&router, page_size, None).await;
        assert_eq!(
            summaries(&body),
            Vec::<Value>::new(),
            "case maxResults={page_size:?}: an empty store lists nothing, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case maxResults={page_size:?}: nothing follows the page, so no token is \
             issued, got {body}"
        );
    }
}

#[tokio::test]
async fn a_listed_status_is_the_derived_one_the_describe_reports() {
    let router = router();
    let created = fixture(&router).await;

    // The create owes a CREATING -> ACTIVE transition the clock makes due, so a
    // listing taken straight after the creates still reports CREATING.
    let body = page(&router, Some(MAX_RESULTS_MAX), None).await;
    for summary in summaries(&body) {
        assert_eq!(
            summary["status"].as_str(),
            Some("CREATING"),
            "an application reports CREATING until the owed transition falls due, got {summary}"
        );
    }

    tokio::time::sleep(ACTIVATION_DELAY + Duration::from_millis(200)).await;

    let settled = page(&router, Some(MAX_RESULTS_MAX), None).await;
    for summary in summaries(&settled) {
        assert_eq!(
            summary["status"].as_str(),
            Some("ACTIVE"),
            "the owed CREATING -> ACTIVE transition must fall due, got {summary}"
        );
    }
    // And it is the SAME status the describe derives for each of them.
    for case in &created {
        let (status, described) = describe(&router, &case.workspace, &case.id).await;
        assert_eq!(
            status, 200,
            "case {:?}: the describe expected 200, got {status} {described}",
            case.label
        );
        assert_eq!(
            described["status"].as_str(),
            Some("ACTIVE"),
            "case {:?}: the listing and the describe derive one status, got {described}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_deleted_application_leaves_the_enumeration_and_the_others_stay() {
    let router = router();
    let created = fixture(&router).await;
    let entries = created.len();
    let mut remaining = baseline(&router, entries).await;

    // Rows: each application of the fixture, deleted in turn, so the listing is held
    // against the store after every one rather than after a single delete.
    for case in &created {
        let (status, body) = call(
            &router,
            "DELETE",
            &format!("/workspaces/{}/applications/{}", case.workspace, case.id),
            None,
        )
        .await;
        assert_eq!(
            status, 204,
            "case {:?}: the delete expected 204, got {status} {body}",
            case.label
        );
        remaining.retain(|id| id != &case.id);

        let listed = walk(&router, Some(MAX_RESULTS_MAX), entries).await.0;
        assert_eq!(
            listed, remaining,
            "case {:?}: a deleted application must leave the enumeration and the rest stay \
             in order, got {listed:?}",
            case.label
        );
    }
}
