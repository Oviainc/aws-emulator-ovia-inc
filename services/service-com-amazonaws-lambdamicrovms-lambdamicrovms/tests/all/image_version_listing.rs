// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListMicrovmImageVersions`: the versions one MicroVM image of
//! this account holds.
//!
//! RULE: the listing reports the versions the image the `imageIdentifier` names holds —
//! resolved by either form `MicrovmImageIdentifier` accepts — one entry per stored
//! version record, NEWEST FIRST in the `major.minor` order the versions themselves name,
//! each carrying that image's ARN and the record's own build inputs, state, status and
//! instants, paged by the operation's `@paginated` contract; a well-formed identifier
//! naming no image of this account is HTTP 404 and a value that is no identifier of this
//! API is HTTP 400.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`call`, `image_body`, …) is the one `microvm_lifecycle` owns; the listing's own
//! harness keeps the shared engine so a test can hold an image of SEVERAL versions
//! without driving an update for each.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    image_not_found_message, invalid_arn_format_message, service_state, version_order,
    MAX_LIST_PAGE,
};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_image_and_instance_paths::{delete_image, get_image};
use crate::microvm_lifecycle::{
    as_uri_value, call, expected_image_arn, image_body, ACCOUNT, PAST_THE_SPAN,
};

/// Every member a listed version shares with the `CreateMicrovmImage` response: the
/// response is the stored record and so is the entry, so each one must agree.
const SHARED_VERSION_MEMBERS: &[&str] = &[
    "imageArn",
    "imageVersion",
    "baseImageArn",
    "baseImageVersion",
    "buildRoleArn",
    "description",
    "codeArtifact",
    "logging",
    "egressNetworkConnectors",
    "cpuConfigurations",
    "resources",
    "additionalOsCapabilities",
    "hooks",
    "environmentVariables",
    "tags",
    "createdAt",
];

/// A fresh emulator holding only this service, over one shared engine — the engine comes
/// back too, so a test can seed the versions of an image the wire cannot mint yet.
fn emulator_with_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine.clone()));
    (router, engine)
}

/// `GET /2025-09-09/microvm-images/{imageIdentifier}/versions`, with whatever `query`
/// adds to the query string.
pub(crate) async fn list_versions(
    router: &MultiServiceRouter,
    identifier: &str,
    query: &str,
) -> (u16, Value) {
    let path = format!(
        "/2025-09-09/microvm-images/{}/versions",
        as_uri_value(identifier)
    );
    let uri = match query.is_empty() {
        true => path,
        false => format!("{path}?{query}"),
    };
    call(router, "GET", &uri, None).await
}

/// Create an image, asserting the create, and hand back its response.
async fn create_image(router: &MultiServiceRouter, name: &str, extra: Value) -> Value {
    let (status, body) = call(
        router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(name, extra)),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: create expected 201, got {status} {body}"
    );
    body
}

/// Give the image `name` the extra versions `versions`, each a copy of the record the
/// create persisted under a version of its own, with its build already settled.
///
/// Writing the records the handlers read is what lets a listing test hold an image of
/// several versions without driving an `UpdateMicrovmImage` for each.
fn seed_versions(engine: &engine::InMemoryEngine, name: &str, versions: &[&str]) {
    let state = service_state(engine).expect("the service's own store");
    let mut store = state.lock();
    let image = store
        .image_mut(name)
        .unwrap_or_else(|| panic!("the created image {name:?} is held"));
    let template = image
        .versions
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| panic!("the created image {name:?} holds its first version"));
    for version in versions {
        let mut record = template.clone();
        record.image_version = (*version).to_string();
        record.build_due_at = None;
        image.versions.insert((*version).to_string(), record);
    }
}

/// Remove every version of the image `name`, so the listing has nothing to report.
fn clear_versions(engine: &engine::InMemoryEngine, name: &str) {
    let state = service_state(engine).expect("the service's own store");
    let mut store = state.lock();
    store
        .image_mut(name)
        .unwrap_or_else(|| panic!("the created image {name:?} is held"))
        .versions
        .clear();
}

/// The `items` of one page.
fn page_items(body: &Value, case: &str) -> Vec<Value> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: items is a list, got {body}"))
        .clone()
}

/// The `imageVersion`s of one page, in the order the page carries them.
fn page_versions(body: &Value, case: &str) -> Vec<String> {
    page_items(body, case)
        .iter()
        .map(|item| {
            item["imageVersion"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A hex-encoded `nextToken` candidate, as the opaque token form this listing issues.
fn hex_token(value: &str) -> String {
    ::hex::encode(value.as_bytes())
}

/// Walk every page of the listing of `identifier` at page size `size`, returning the
/// versions in the order the walk served them. Fails rather than hanging on a listing
/// that never terminates.
async fn walk(
    router: &MultiServiceRouter,
    identifier: &str,
    image_arn: &str,
    size: Option<usize>,
    held: usize,
    case: &str,
) -> Vec<String> {
    let cap = size.unwrap_or(MAX_LIST_PAGE);
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;

    for _ in 0..=held {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("nextToken={token}"));
        }
        let (status, body) = list_versions(router, identifier, &query.join("&")).await;
        assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");

        let page = page_versions(&body, case);
        assert!(
            page.len() <= cap,
            "case {case}: a page carries at most {cap} versions, got {} ({page:?})",
            page.len()
        );
        for item in page_items(&body, case) {
            assert_eq!(
                item["imageArn"].as_str(),
                Some(image_arn),
                "case {case}: every entry carries the ARN of the image holding it, got {:?}",
                item["imageArn"]
            );
        }
        seen.extend(page);
        assert!(
            seen.len() <= held,
            "case {case}: the walk served more than the image holds, got {seen:?}"
        );

        token = body["nextToken"].as_str().map(str::to_string);
        match &token {
            Some(issued) => assert!(
                seen.len() < held,
                "case {case}: a nextToken ({issued}) is due only while versions remain, got it \
                 after {seen:?}"
            ),
            None => break,
        }
    }

    assert!(
        token.is_none(),
        "case {case}: the walk must end with no nextToken, got {token:?}"
    );
    seen
}

/// RULE: however the pages are cut and however the image is addressed, the listing walks
/// every version that image holds — each exactly once, NEWEST FIRST in the `major.minor`
/// order the versions name, at most `maxResults` per page — and hands back a `nextToken`
/// only while versions remain.
#[tokio::test]
async fn every_held_version_is_served_once_over_every_page_size() {
    let (router, engine) = emulator_with_store();
    create_image(&router, "paged", json!({})).await;

    // The seeded versions differ in major and in minor, and include a two-digit minor
    // and a two-digit major, which is what the `major.minor` order generalises over: a
    // lexicographic order would put `1.10` before `1.9` and `10.0` before `2.0`.
    let seeded = ["2.0", "1.9", "1.10", "10.0", "1.1"];
    seed_versions(&engine, "paged", &seeded);

    let mut expected: Vec<String> = seeded.iter().map(|v| (*v).to_string()).collect();
    expected.push("1.0".to_string()); // the version the create minted
    expected.sort_by_key(|version| ::std::cmp::Reverse(version_order(version)));
    let held = expected.len();

    // Rows differ in the identifier form and in the page size: no `maxResults` at all,
    // each published bound of `@range(min: 1, max: 50)`, and every size up to the number
    // of versions held.
    let identifiers = [
        ("the image name", "paged".to_string()),
        ("the image ARN", expected_image_arn("paged")),
    ];
    let mut sizes: Vec<Option<usize>> = vec![None, Some(1), Some(MAX_LIST_PAGE)];
    sizes.extend((2..=held).map(Some));

    for (form, identifier) in &identifiers {
        for size in &sizes {
            let case = format!("{form} {identifier:?} maxResults={size:?}");
            let seen = walk(
                &router,
                identifier,
                &expected_image_arn("paged"),
                *size,
                held,
                &case,
            )
            .await;
            assert_eq!(
                seen, expected,
                "case {case}: expected every held version exactly once, newest first, got \
                 {seen:?}"
            );
        }
    }
}

/// RULE: each entry is the stored version record, so every member it shares with the
/// create's own projection agrees with it, its state and status are the ones the build
/// clock has reached, and reading the listing changes nothing a later caller sees.
#[tokio::test]
async fn each_entry_is_the_stored_version_record_and_tracks_the_build_clock() {
    let (router, _) = emulator_with_store();

    // Rows differ in how much of the version the request specified: only the required
    // members, and every optional member a version carries.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("only the required members", "minimal", json!({})),
        (
            "every optional member",
            "specified",
            json!({
                "description": "the service tier",
                "tags": {"team": "platform", "tier": "gold"},
                "environmentVariables": {"MODE": "batch"},
                // `Capability` publishes `ALL` and `Architecture` publishes `ARM_64`
                // (`smithy_query(query="shape", shape="Capability,Architecture")`).
                "additionalOsCapabilities": ["ALL"],
                "cpuConfigurations": [{"architecture": "ARM_64"}],
                "resources": [{"minimumMemoryInMiB": 4096}],
            }),
        ),
    ];

    for (case, name, extra) in rows {
        let created = create_image(&router, name, extra).await;

        // Before the asynchronous build is due the version is reported in flight.
        let (status, during) = list_versions(&router, name, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {during}"
        );
        let items = page_items(&during, case);
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: a created image holds exactly the version the create minted, got \
             {during}"
        );
        for member in SHARED_VERSION_MEMBERS {
            assert_eq!(
                items[0][member], created[member],
                "case {case:?}: the entry's {member} is the stored record's, got {:?} listed and \
                 {:?} created",
                items[0][member], created[member]
            );
        }
        assert_eq!(
            items[0]["state"].as_str(),
            Some("IN_PROGRESS"),
            "case {case:?}: the build is still in flight, got {:?}",
            items[0]["state"]
        );
        assert_eq!(
            items[0]["status"].as_str(),
            Some("INACTIVE"),
            "case {case:?}: a version whose build is in flight launches nothing, got {:?}",
            items[0]["status"]
        );
        assert!(
            items[0]["stateReason"].is_null(),
            "case {case:?}: no build recorded a reason, so none is reported, got {:?}",
            items[0]["stateReason"]
        );

        // A reporting operation changes nothing a later caller sees.
        let (status, again) = list_versions(&router, name, "").await;
        assert_eq!(status, 200, "case {case:?}: re-list got {status} {again}");
        assert_eq!(
            again, during,
            "case {case:?}: the listing reports state, so reading it twice reports the same, got \
             {again}"
        );

        // Once the clock has made the build due the SAME version is reported settled,
        // and it is the version the image now reports as its active one.
        tokio::time::sleep(PAST_THE_SPAN).await;
        let (status, settled) = list_versions(&router, name, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {settled}"
        );
        let items = page_items(&settled, case);
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: the build settling mints no further version, got {settled}"
        );
        assert_eq!(
            items[0]["imageVersion"], created["imageVersion"],
            "case {case:?}: the settled entry is the same version, got {settled}"
        );
        assert_eq!(
            items[0]["state"].as_str(),
            Some("SUCCESSFUL"),
            "case {case:?}: the build the clock has made due has succeeded, got {:?}",
            items[0]["state"]
        );
        assert_eq!(
            items[0]["status"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: a built version can launch MicroVMs, got {:?}",
            items[0]["status"]
        );

        let (status, image) = get_image(&router, name).await;
        assert_eq!(status, 200, "case {case:?}: read the image: got {image}");
        assert_eq!(
            image["latestActiveImageVersion"], items[0]["imageVersion"],
            "case {case:?}: the image's active version is the one listed ACTIVE, got {image}"
        );
    }
}

/// RULE: the identifier is resolved against the images THIS ACCOUNT holds, by either form
/// `MicrovmImageIdentifier` accepts; a well-formed identifier of this account's own
/// namespace naming no image of it — an image the clock has already removed included — is
/// HTTP 404 `ResourceNotFoundException`, and a value that is no identifier of this API at
/// all is HTTP 400 `ValidationException`. (An identifier of the AWS-OWNED namespace is
/// refused 403 before any lookup; `managed_namespace_authorization` owns that verdict.)
#[tokio::test]
async fn an_identifier_naming_no_held_image_is_refused() {
    let (router, _) = emulator_with_store();
    create_image(&router, "held", json!({})).await;

    // Both forms of a held image's identifier serve the listing.
    for (case, identifier) in [
        ("the image name", "held".to_string()),
        ("the image ARN", expected_image_arn("held")),
    ] {
        let (status, body) = list_versions(&router, &identifier, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: {identifier:?} expected 200, got {status} {body}"
        );
        assert_eq!(
            page_versions(&body, case),
            vec!["1.0".to_string()],
            "case {case:?}: both identifier forms address the same image, got {body}"
        );
    }

    // Rows differ in HOW the identifier fails to name an image this account holds: the
    // right ARN form for a name never created, another region's ARN for a held name, and
    // values that are no identifier of this API at all.
    let never_created = expected_image_arn("never-created");
    let other_region = format!("arn:aws:lambda:eu-central-1:{ACCOUNT}:microvm-image:held");
    let rows: Vec<(&str, String, u16, String)> = vec![
        (
            "an ARN for a name never created",
            never_created.clone(),
            404,
            image_not_found_message(&never_created),
        ),
        (
            "another region's ARN for a held name",
            other_region.clone(),
            404,
            image_not_found_message(&other_region),
        ),
        (
            "a name no image of this account carries",
            "never-created".to_string(),
            400,
            invalid_arn_format_message("never-created"),
        ),
        (
            "no identifier of this API at all",
            "not-an-identifier".to_string(),
            400,
            invalid_arn_format_message("not-an-identifier"),
        ),
    ];

    for (case, identifier, expected_status, expected_message) in rows {
        let (status, body) = list_versions(&router, &identifier, "").await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: {identifier:?} expected {expected_status}, got {status} {body}"
        );
        let expected_type = match expected_status {
            404 => "ResourceNotFoundException",
            _ => "ValidationException",
        };
        assert_eq!(
            body["__type"].as_str(),
            Some(expected_type),
            "case {case:?}: expected {expected_type}, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected_message.as_str()),
            "case {case:?}: expected the refusal built from the identifier the request carried, \
             got {:?}",
            body["message"]
        );
    }

    // An image whose asynchronous deletion is in flight is still held, so its versions
    // are still listed; once the clock has removed it the listing answers the same 404 as
    // a name never created.
    let (status, deleted) = delete_image(&router, "held").await;
    assert_eq!(status, 200, "the delete expected 200, got {status} {deleted}");
    let (status, during) = list_versions(&router, "held", "").await;
    assert_eq!(
        status, 200,
        "a delete in flight still holds the image, got {status} {during}"
    );
    assert_eq!(
        page_versions(&during, "a delete in flight"),
        vec!["1.0".to_string()],
        "a delete in flight still holds the image's versions, got {during}"
    );

    tokio::time::sleep(PAST_THE_SPAN).await;
    let (status, after) = list_versions(&router, &expected_image_arn("held"), "").await;
    assert_eq!(
        status, 404,
        "a removed image holds no versions to list, got {status} {after}"
    );
    assert_eq!(
        after["message"].as_str(),
        Some(image_not_found_message(&expected_image_arn("held")).as_str()),
        "the removed image answers the lookup's own 404, got {after}"
    );
}

/// RULE: an image holding no version the request selects gets a terminal empty page —
/// `items` is `@required`, so it is projected empty rather than omitted.
#[tokio::test]
async fn an_image_holding_no_version_gets_an_empty_page() {
    let (router, engine) = emulator_with_store();
    create_image(&router, "emptied", json!({})).await;
    clear_versions(&engine, "emptied");

    // Rows differ in the page size asked for, which cannot turn an empty report into a
    // refusal or into a token.
    for size in [None, Some(1), Some(MAX_LIST_PAGE)] {
        let case = format!("maxResults={size:?}");
        let query = size.map(|n| format!("maxResults={n}")).unwrap_or_default();
        let (status, body) = list_versions(&router, "emptied", &query).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {body}"
        );
        assert_eq!(
            body["items"],
            json!([]),
            "case {case}: items is projected as an empty list, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case}: an exhausted listing issues no nextToken, got {:?}",
            body["nextToken"]
        );
    }
}

/// RULE: `maxResults` is bounded by the range the model publishes, so each bound is
/// served and a value one past either bound is refused with HTTP 400
/// `ValidationException` — `maxResults` is `@range(min: 1, max: 50)`
/// (`smithy_query(query="operation", operation="ListMicrovmImageVersions")`).
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let (router, _) = emulator_with_store();
    create_image(&router, "sized", json!({})).await;

    let rows: Vec<(&str, usize, u16)> = vec![
        ("one below the published minimum", 0, 400),
        ("the published minimum", 1, 200),
        ("the published maximum", MAX_LIST_PAGE, 200),
        ("one past the published maximum", MAX_LIST_PAGE + 1, 400),
    ];

    for (case, max_results, expected_status) in rows {
        let (status, body) =
            list_versions(&router, "sized", &format!("maxResults={max_results}")).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: maxResults={max_results} expected {expected_status}, got {status} \
             {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                body["__type"].as_str(),
                Some("ValidationException"),
                "case {case:?}: expected ValidationException, got {body}"
            );
        }
    }
}

/// RULE: a `nextToken` this listing did not issue positions nothing, so it is refused
/// with HTTP 400 `ValidationException` rather than silently restarting the listing.
#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let (router, _) = emulator_with_store();
    create_image(&router, "tokened", json!({})).await;

    // Rows differ in HOW the token fails to be one this listing issued: not the opaque
    // encoding at all, the encoding carrying no cursor of this listing, another
    // listing's token, and a marker this listing never issued.
    let rows: Vec<(&str, String)> = vec![
        ("not the opaque encoding", "not-a-token".to_string()),
        ("an empty encoded value", hex_token("")),
        ("an encoded cursor with no marker", hex_token("1.0")),
        (
            "another listing's token",
            hex_token("ListManagedMicrovmImageVersions/1:1.0"),
        ),
        (
            "a token of a marker this listing never issued",
            hex_token("ListMicrovmImageVersions/0:1.0"),
        ),
    ];

    for (case, token) in rows {
        let (status, body) =
            list_versions(&router, "tokened", &format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: nextToken={token} must be refused, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("nextToken is not a pagination token this service issued"),
            "case {case:?}: expected the refusal to name the token member, got {:?}",
            body["message"]
        );
    }
}
