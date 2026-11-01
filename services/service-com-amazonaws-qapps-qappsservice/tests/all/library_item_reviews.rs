// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the library-item review surface:
//! `CreateLibraryItem`, `AssociateLibraryItemReview` and `GetLibraryItem`.
//!
//! The RULE every test here asserts: a library item's `ratingCount` is the
//! number of DISTINCT users that have a review associated with it. A user's
//! first `AssociateLibraryItemReview` associates their review (and makes
//! `isRatedByUser` true for that user alone); a further one from the SAME user
//! conflicts with the item's current state and leaves the count where it was.
//! The rows of each table differ along what the rule generalises over — how many
//! users review, how many times each one submits, and whether they submit at the
//! same time — so the recorded single-user race is one row of one table.
//!
//! Every test drives the service the way a client does — through the router,
//! over restJson1, with the required `instance-id` header and a SigV4
//! `Authorization` header whose access key id is the caller's identity — so what
//! it asserts is the wire behaviour and not a handler's internals.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_qapps_qappsservice::ops::types::LIBRARY_ITEM_STATUS_VALUES;
use service_com_amazonaws_qapps_qappsservice::state::{
    service_state, LibraryCategory, MAX_PRINCIPAL_LENGTH, MAX_REVIEWS_PER_LIBRARY_ITEM,
};
use service_com_amazonaws_qapps_qappsservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// An instance of the Amazon Q Business application environment: the tenant
/// boundary every operation takes as the `instance-id` header.
const INSTANCE: &str = "11111111-2222-4333-8444-555555555555";
/// A second instance, to show one instance's library items are invisible from
/// another.
const OTHER_INSTANCE: &str = "99999999-8888-4777-8666-555555555555";

/// A card id, shaped as the model's `UUID`
/// (`^[\da-f]{8}-[\da-f]{4}-[45][\da-f]{3}-[89ABab][\da-f]{3}-[\da-f]{12}$`).
const CARD: &str = "aaaaaaaa-1111-4111-8111-111111111111";

/// A `UUID`-shaped id no library item was ever minted with.
const UNKNOWN_ITEM: &str = "deadbeef-0000-4000-8000-000000000000";

/// Category ids for the instance's catalog, same `UUID` shape.
const CATEGORY_A: &str = "cccccccc-1111-4111-8111-111111111111";
const CATEGORY_B: &str = "cccccccc-2222-4222-9222-222222222222";
const UNKNOWN_CATEGORY: &str = "cccccccc-3333-4333-a333-333333333333";

fn build_engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// The production router with this one service mounted on `engine`.
fn build_router(engine: &Arc<engine::InMemoryEngine>) -> Arc<MultiServiceRouter> {
    let mut router = MultiServiceRouter::new();
    // Signatures are not the subject of these tests; the credential scope in the
    // header below still carries the caller's identity and the route key.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    // Q Apps serves only a caller federated through IAM Identity Center
    // (`crate::identity_center`), so every caller these tests sign as is signed in
    // first. The suite's own subject is what the reviews surface does for an
    // AUTHORIZED caller; the front door itself is pinned in
    // `tests/all/identity_center_front_door.rs`.
    for index in 0..=MAX_REVIEWS_PER_LIBRARY_ITEM + 1 {
        crate::identity_center::federate(engine, &caller(index));
    }
    Arc::new(router)
}

/// The access key id of caller `index`. Every qapps handler derives the calling
/// principal from it, so distinct ids are distinct users.
fn caller(index: usize) -> String {
    format!("AKIDREVIEWER{index:04}")
}

/// Send one request as `akid` and return its status and JSON body
/// (`Value::Null` for an empty body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    instance: &str,
    akid: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("instance-id", instance)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/us-east-1/qapps/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(payload)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

async fn create_app(router: &MultiServiceRouter, instance: &str, akid: &str, title: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/apps.create",
        instance,
        akid,
        Some(json!({
            "title": title,
            "appDefinition": { "cards": [
                { "textInput": { "id": CARD, "title": "Topic", "type": "text-input" } },
            ]},
        })),
    )
    .await;
    assert_eq!(status, 200, "create app {title:?}: body {body}");
    body["appId"]
        .as_str()
        .unwrap_or_else(|| panic!("create app {title:?} returned no appId: {body}"))
        .to_string()
}

async fn update_app_title(
    router: &MultiServiceRouter,
    instance: &str,
    akid: &str,
    app_id: &str,
    title: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        "/apps.update",
        instance,
        akid,
        Some(json!({ "appId": app_id, "title": title })),
    )
    .await
}

async fn get_app(
    router: &MultiServiceRouter,
    instance: &str,
    akid: &str,
    app_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/apps.get?appId={app_id}"),
        instance,
        akid,
        None,
    )
    .await
}

async fn publish(
    router: &MultiServiceRouter,
    instance: &str,
    akid: &str,
    app_id: &str,
    app_version: i64,
    categories: &[&str],
) -> (u16, Value) {
    call(
        router,
        "POST",
        "/catalog.createItem",
        instance,
        akid,
        Some(json!({
            "appId": app_id,
            "appVersion": app_version,
            "categories": categories,
        })),
    )
    .await
}

async fn associate(
    router: &MultiServiceRouter,
    instance: &str,
    akid: &str,
    library_item_id: &str,
) -> (u16, Value) {
    call(
        router,
        "POST",
        "/catalog.associateItemRating",
        instance,
        akid,
        Some(json!({ "libraryItemId": library_item_id })),
    )
    .await
}

async fn get_item(
    router: &MultiServiceRouter,
    instance: &str,
    akid: &str,
    library_item_id: &str,
    app_id: Option<&str>,
) -> (u16, Value) {
    let uri = match app_id {
        Some(app_id) => format!("/catalog.getItem?libraryItemId={library_item_id}&appId={app_id}"),
        None => format!("/catalog.getItem?libraryItemId={library_item_id}"),
    };
    call(router, "GET", &uri, instance, akid, None).await
}

/// Publish a fresh Q App of `instance` to its library and hand back
/// `(appId, libraryItemId)`.
async fn published_item(
    router: &MultiServiceRouter,
    instance: &str,
    akid: &str,
    title: &str,
    categories: &[&str],
) -> (String, String) {
    let app_id = create_app(router, instance, akid, title).await;
    let (status, body) = publish(router, instance, akid, &app_id, 1, categories).await;
    assert_eq!(status, 200, "publish {title:?}: body {body}");
    assert_eq!(
        body["ratingCount"].as_i64(),
        Some(0),
        "case {title:?}: a fresh library item carries no reviews, got {body}"
    );
    let library_item_id = body["libraryItemId"]
        .as_str()
        .unwrap_or_else(|| panic!("publish {title:?} returned no libraryItemId: {body}"))
        .to_string();
    (app_id, library_item_id)
}

/// Seed the instance's category catalog the way `BatchCreateCategory` will once
/// that scaffold is implemented: the titles a `Category` reports live only in the
/// catalog, so a test that wants a filed library item has to put them there.
fn seed_categories(engine: &Arc<engine::InMemoryEngine>, instance: &str) {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let mut store = state.lock();
    for (id, title, color) in [
        (CATEGORY_A, "Productivity", Some("#1f77b4")),
        (CATEGORY_B, "Human Resources", None),
    ] {
        store.library_categories.insert(
            (instance.to_string(), id.to_string()),
            LibraryCategory {
                id: id.to_string(),
                title: title.to_string(),
                color: color.map(str::to_string),
            },
        );
    }
}

fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or_default()
}

/// RULE: `ratingCount` is the number of distinct users holding a review, and
/// `isRatedByUser` answers for the CALLER of the read.
#[tokio::test]
async fn rating_count_is_the_number_of_distinct_users_who_reviewed() {
    // (case, how many distinct users review the item)
    let cases: [(&str, usize); 4] = [
        ("nobody reviews", 0),
        ("one reviewer", 1),
        ("three reviewers", 3),
        ("eight reviewers", 8),
    ];

    for (case, reviewers) in cases {
        let engine = build_engine();
        let router = build_router(&engine);
        let (app_id, item) = published_item(&router, INSTANCE, &caller(0), case, &[]).await;

        for index in 0..reviewers {
            let (status, body) = associate(&router, INSTANCE, &caller(index), &item).await;
            assert_eq!(
                status, 200,
                "case {case:?}: reviewer {index} associates its first review and must answer \
                 200, got {status} {body}"
            );
        }

        // Read by a user who DID review (when there is one) and by a user who
        // did not: the count is the same for both, `isRatedByUser` is not.
        let expected_count = i64::try_from(reviewers).expect("the row count fits an i64");
        for (reader, expected_rated) in [(0, reviewers > 0), (reviewers, false)] {
            let (status, body) =
                get_item(&router, INSTANCE, &caller(reader), &item, Some(&app_id)).await;
            assert_eq!(
                (status, body["ratingCount"].as_i64(), body["isRatedByUser"].as_bool()),
                (200, Some(expected_count), Some(expected_rated)),
                "case {case:?}: reader {reader} expected 200 / ratingCount \
                 {expected_count} / isRatedByUser {expected_rated}, got {status} {body}"
            );
            assert!(
                LIBRARY_ITEM_STATUS_VALUES.contains(&body["status"].as_str().unwrap_or_default()),
                "case {case:?}: status must be one of the model's LibraryItemStatus values \
                 {LIBRARY_ITEM_STATUS_VALUES:?}, got {body}"
            );
            assert_eq!(
                (body["appId"].as_str(), body["appVersion"].as_i64()),
                (Some(app_id.as_str()), Some(1)),
                "case {case:?}: the item reports the app version it snapshotted, got {body}"
            );
        }
    }
}

/// RULE: a review is per user, so the same user's further submissions conflict
/// (409) and the count does not move.
#[tokio::test]
async fn a_second_review_from_one_user_conflicts_and_leaves_the_count_unchanged() {
    // (case, how many times that one user submits)
    let cases: [(&str, usize); 3] = [
        ("submits twice", 2),
        ("submits three times", 3),
        ("submits eight times", 8),
    ];

    for (case, submissions) in cases {
        let engine = build_engine();
        let router = build_router(&engine);
        let (_, item) = published_item(&router, INSTANCE, &caller(0), case, &[]).await;

        for attempt in 0..submissions {
            let (status, body) = associate(&router, INSTANCE, &caller(0), &item).await;
            let (expected_status, expected_code) = if attempt == 0 {
                (200, "")
            } else {
                (409, "ConflictException")
            };
            assert_eq!(
                status, expected_status,
                "case {case:?}: submission {attempt} expected {expected_status}, got {status} \
                 {body}"
            );
            if attempt > 0 {
                assert_eq!(
                    (error_code(&body), body["resourceType"].as_str()),
                    (expected_code, Some("library-item")),
                    "case {case:?}: submission {attempt} expected {expected_code} on the \
                     library item, got {body}"
                );
            }
        }

        // Another user still reviews it: the refusal is about THAT user, not
        // about the item being closed to reviews.
        let (status, body) = associate(&router, INSTANCE, &caller(1), &item).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a different user's first review must answer 200, got {status} {body}"
        );

        let (status, body) = get_item(&router, INSTANCE, &caller(0), &item, None).await;
        assert_eq!(
            (status, body["ratingCount"].as_i64()),
            (200, Some(2)),
            "case {case:?}: {submissions} submissions by one user plus one by another count \
             two reviews, got {status} {body}"
        );
    }
}

/// RULE: the count is maintained under one store lock, so concurrent submissions
/// by ONE user have a single winner, while concurrent submissions by DISTINCT
/// users all win.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_reviews_have_one_winner_per_user() {
    // (case, how many callers race, do they all share one identity?)
    let cases: [(&str, usize, bool); 5] = [
        ("two racers, one user", 2, true),
        ("eight racers, one user", 8, true),
        ("sixteen racers, one user", 16, true),
        ("two racers, distinct users", 2, false),
        ("eight racers, distinct users", 8, false),
    ];

    for (case, racers, share_identity) in cases {
        let engine = build_engine();
        let router = build_router(&engine);
        let (_, item) = published_item(&router, INSTANCE, &caller(0), case, &[]).await;

        let mut tasks = Vec::with_capacity(racers);
        for index in 0..racers {
            let router = Arc::clone(&router);
            let item = item.clone();
            let akid = if share_identity {
                caller(0)
            } else {
                caller(index)
            };
            tasks.push(tokio::spawn(async move {
                associate(&router, INSTANCE, &akid, &item).await
            }));
        }
        let mut accepted = 0usize;
        let mut conflicted = 0usize;
        for task in tasks {
            let (status, body) = task.await.expect("the racing review task must not panic");
            match status {
                200 => accepted += 1,
                409 => {
                    assert_eq!(
                        error_code(&body),
                        "ConflictException",
                        "case {case:?}: a 409 must be the modelled ConflictException, got {body}"
                    );
                    conflicted += 1;
                }
                other => panic!("case {case:?}: unexpected status {other}: {body}"),
            }
        }

        let expected_accepted = if share_identity { 1 } else { racers };
        assert_eq!(
            (accepted, conflicted),
            (expected_accepted, racers - expected_accepted),
            "case {case:?}: expected {expected_accepted} accepted and \
             {} conflicted, got {accepted} accepted and {conflicted} conflicted",
            racers - expected_accepted
        );

        let (status, body) = get_item(&router, INSTANCE, &caller(0), &item, None).await;
        let expected_count = i64::try_from(expected_accepted).expect("the row count fits an i64");
        assert_eq!(
            (status, body["ratingCount"].as_i64()),
            (200, Some(expected_count)),
            "case {case:?}: the item counts one review per distinct user, got {status} {body}"
        );
    }
}

/// RULE: the reviews one library item retains are bounded, and the bound is
/// reported as the modelled quota refusal — a user who already reviewed still
/// gets the conflict, because that is about the user and not about the bound.
#[tokio::test]
async fn the_reviews_one_library_item_retains_are_bounded() {
    let engine = build_engine();
    let router = build_router(&engine);
    let (_, item) = published_item(&router, INSTANCE, &caller(0), "bounded", &[]).await;

    for index in 0..MAX_REVIEWS_PER_LIBRARY_ITEM {
        let (status, body) = associate(&router, INSTANCE, &caller(index), &item).await;
        assert_eq!(
            status, 200,
            "reviewer {index} is within the bound of {MAX_REVIEWS_PER_LIBRARY_ITEM} and must \
             answer 200, got {status} {body}"
        );
    }

    // (case, the caller, expected status, expected error code)
    let at_bound = caller(MAX_REVIEWS_PER_LIBRARY_ITEM);
    let cases: [(&str, &str, u16, &str); 2] = [
        ("one reviewer past the bound", at_bound.as_str(), 402, "ServiceQuotaExceededException"),
        ("a reviewer already counted", "AKIDREVIEWER0000", 409, "ConflictException"),
    ];
    for (case, akid, expected_status, expected_code) in cases {
        let (status, body) = associate(&router, INSTANCE, akid, &item).await;
        assert_eq!(
            (status, error_code(&body)),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {body}"
        );
    }

    let (status, body) = get_item(&router, INSTANCE, &caller(0), &item, None).await;
    let expected_count =
        i64::try_from(MAX_REVIEWS_PER_LIBRARY_ITEM).expect("the bound fits an i64");
    assert_eq!(
        (status, body["ratingCount"].as_i64()),
        (200, Some(expected_count)),
        "the refused reviews did not move the count: {status} {body}"
    );
}

/// RULE: a review and a publication are attributed to the caller's access key
/// id, so a credential that cannot be one — longer than the
/// [`MAX_PRINCIPAL_LENGTH`] characters an access key id carries — names no user
/// and is refused 401 `UnauthorizedException`.
#[tokio::test]
async fn a_credential_too_long_to_be_an_access_key_id_names_no_user() {
    let engine = build_engine();
    let router = build_router(&engine);
    let app_id = create_app(&router, INSTANCE, &caller(0), "attribution").await;
    let (_, item) = published_item(&router, INSTANCE, &caller(0), "attribution target", &[]).await;

    // The published bound, and one character past it.
    let at_bound = "A".repeat(MAX_PRINCIPAL_LENGTH);
    let past_bound = "A".repeat(MAX_PRINCIPAL_LENGTH + 1);
    // Each of these callers is federated, so what the assertions below observe is the
    // ATTRIBUTION rule in the handlers rather than the Identity Center front door,
    // which would refuse any caller it has no sign-in for whatever its length.
    for akid in ["AKIDSHORT", at_bound.as_str(), past_bound.as_str()] {
        crate::identity_center::federate(&engine, akid);
    }
    // (case, the access key id, expected status)
    let cases: [(&str, &str, u16); 3] = [
        ("a short access key id", "AKIDSHORT", 200),
        ("an access key id at the bound", at_bound.as_str(), 200),
        ("one character past the bound", past_bound.as_str(), 401),
    ];

    for (case, akid, expected_status) in cases {
        let (status, body) = associate(&router, INSTANCE, akid, &item).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: AssociateLibraryItemReview expected {expected_status}, got {status} \
             {body}"
        );
        let (status, body) = publish(&router, INSTANCE, akid, &app_id, 1, &[]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: CreateLibraryItem expected {expected_status}, got {status} {body}"
        );
        if expected_status == 401 {
            assert_eq!(
                error_code(&body),
                "UnauthorizedException",
                "case {case:?}: expected the modelled 401, got {body}"
            );
        }
    }
}

/// RULE: a library item is addressed by `(instanceId, libraryItemId)` and, where
/// the request names one, by the app it snapshots — anything else names no item
/// and is refused 404 `ResourceNotFoundException`, by the write as well as the
/// read.
#[tokio::test]
async fn an_item_of_another_instance_or_another_app_is_not_found() {
    let engine = build_engine();
    let router = build_router(&engine);
    let (app_id, item) = published_item(&router, INSTANCE, &caller(0), "addressing", &[]).await;
    let (other_app_id, other_item) =
        published_item(&router, OTHER_INSTANCE, &caller(0), "other tenant", &[]).await;

    // (case, instance the request carries, item id, appId filter, expected status)
    let cases: [(&str, &str, &str, Option<&str>, u16); 6] = [
        ("the item, by its own instance", INSTANCE, item.as_str(), None, 200),
        ("the item, with its own appId", INSTANCE, item.as_str(), Some(app_id.as_str()), 200),
        (
            "the item, with another app's appId",
            INSTANCE,
            item.as_str(),
            Some(other_app_id.as_str()),
            404,
        ),
        ("an id nothing was minted with", INSTANCE, UNKNOWN_ITEM, None, 404),
        ("another instance's item", INSTANCE, other_item.as_str(), None, 404),
        ("this instance's item, read from the other", OTHER_INSTANCE, item.as_str(), None, 404),
    ];

    for (case, instance, library_item_id, app_filter, expected_status) in cases {
        let (status, body) =
            get_item(&router, instance, &caller(0), library_item_id, app_filter).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: GetLibraryItem expected {expected_status}, got {status} {body}"
        );
        if expected_status == 404 {
            assert_eq!(
                (error_code(&body), body["resourceType"].as_str()),
                ("ResourceNotFoundException", Some("library-item")),
                "case {case:?}: expected the modelled not-found on the library item, got {body}"
            );
        }
    }

    // The write refuses the same addresses, and for the same reason.
    for (case, instance, library_item_id) in [
        ("an id nothing was minted with", INSTANCE, UNKNOWN_ITEM),
        ("another instance's item", INSTANCE, other_item.as_str()),
        ("this instance's item, from the other", OTHER_INSTANCE, item.as_str()),
    ] {
        let (status, body) = associate(&router, instance, &caller(0), library_item_id).await;
        assert_eq!(
            (status, error_code(&body), body["resourceType"].as_str()),
            (404, "ResourceNotFoundException", Some("library-item")),
            "case {case:?}: AssociateLibraryItemReview expected 404 on the library item, got \
             {status} {body}"
        );
    }
}

/// RULE: a library item snapshots a RETAINED version of a stored Q App of the
/// same instance, and publishing one takes that app out of `DRAFT`.
#[tokio::test]
async fn publishing_names_a_retained_app_version_and_publishes_the_app() {
    let engine = build_engine();
    let router = build_router(&engine);
    let app_id = create_app(&router, INSTANCE, &caller(0), "versions").await;
    // A second version, so the app retains versions 1 and 2.
    let (status, body) = update_app_title(&router, INSTANCE, &caller(0), &app_id, "versions v2").await;
    assert_eq!(
        (status, body["appVersion"].as_i64()),
        (200, Some(2)),
        "the update must mint version 2: {status} {body}"
    );

    // (case, appId, appVersion, expected status)
    let cases: [(&str, &str, i64, u16); 5] = [
        ("the first retained version", app_id.as_str(), 1, 200),
        ("the current version", app_id.as_str(), 2, 200),
        ("a version the app never had", app_id.as_str(), 3, 404),
        ("the zero version the range allows", app_id.as_str(), 0, 404),
        ("an app nothing was minted with", UNKNOWN_ITEM, 1, 404),
    ];

    for (case, published_app, app_version, expected_status) in cases {
        let (status, body) =
            publish(&router, INSTANCE, &caller(0), published_app, app_version, &[]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: CreateLibraryItem expected {expected_status}, got {status} {body}"
        );
        if expected_status == 404 {
            assert_eq!(
                (error_code(&body), body["resourceType"].as_str()),
                ("ResourceNotFoundException", Some("qapp")),
                "case {case:?}: expected the modelled not-found on the Q App, got {body}"
            );
        } else {
            let library_item_id = body["libraryItemId"].as_str().unwrap_or_default();
            let (status, item) =
                get_item(&router, INSTANCE, &caller(0), library_item_id, None).await;
            assert_eq!(
                (status, item["appVersion"].as_i64()),
                (200, Some(app_version)),
                "case {case:?}: the item reports the version it was published from, got \
                 {status} {item}"
            );
        }
    }

    // Publishing moved the app itself out of DRAFT.
    let (status, body) = get_app(&router, INSTANCE, &caller(0), &app_id).await;
    assert_eq!(
        (status, body["status"].as_str()),
        (200, Some("PUBLISHED")),
        "an app with a library item is published: {status} {body}"
    );

    // An app nobody published stays a draft.
    let draft = create_app(&router, INSTANCE, &caller(0), "unpublished").await;
    let (status, body) = get_app(&router, INSTANCE, &caller(0), &draft).await;
    assert_eq!(
        (status, body["status"].as_str()),
        (200, Some("DRAFT")),
        "an app nobody published stays a draft: {status} {body}"
    );
}

/// RULE: the categories a library item is filed under are the instance's own
/// category resources — the item reports each one with the catalog's title and
/// colour and with `appCount` derived from the items filed under it, a repeated
/// id is refused 400, and an id the catalog does not hold is refused 404.
#[tokio::test]
async fn categories_are_filed_from_the_instances_catalog() {
    // (case, the ids the publisher names, expected status, the (id, title, color)
    // the item must report)
    #[allow(clippy::type_complexity)]
    let cases: [(&str, &[&str], u16, &[(&str, &str, Option<&str>)]); 5] = [
        ("no category", &[], 200, &[]),
        (
            "one category",
            &[CATEGORY_A],
            200,
            &[(CATEGORY_A, "Productivity", Some("#1f77b4"))],
        ),
        (
            "both categories, in the order named",
            &[CATEGORY_B, CATEGORY_A],
            200,
            &[
                (CATEGORY_B, "Human Resources", None),
                (CATEGORY_A, "Productivity", Some("#1f77b4")),
            ],
        ),
        ("one category named twice", &[CATEGORY_A, CATEGORY_A], 400, &[]),
        ("a category the catalog does not hold", &[UNKNOWN_CATEGORY], 404, &[]),
    ];

    for (case, named, expected_status, expected) in cases {
        let engine = build_engine();
        let router = build_router(&engine);
        seed_categories(&engine, INSTANCE);
        let app_id = create_app(&router, INSTANCE, &caller(0), case).await;

        let (status, body) = publish(&router, INSTANCE, &caller(0), &app_id, 1, named).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: CreateLibraryItem expected {expected_status}, got {status} {body}"
        );
        match expected_status {
            400 => {
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: a repeated category id is a client-input refusal, got {body}"
                );
                continue;
            }
            404 => {
                assert_eq!(
                    (error_code(&body), body["resourceType"].as_str()),
                    ("ResourceNotFoundException", Some("category")),
                    "case {case:?}: expected the modelled not-found on the category, got {body}"
                );
                continue;
            }
            _ => {}
        }

        let library_item_id = body["libraryItemId"].as_str().unwrap_or_default();
        let (status, item) = get_item(&router, INSTANCE, &caller(0), library_item_id, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: reading the published item back: {status} {item}"
        );
        let reported: Vec<(String, String, Option<String>, Option<i64>)> = item["categories"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        (
                            entry["id"].as_str().unwrap_or_default().to_string(),
                            entry["title"].as_str().unwrap_or_default().to_string(),
                            entry["color"].as_str().map(str::to_string),
                            entry["appCount"].as_i64(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let wanted: Vec<(String, String, Option<String>, Option<i64>)> = expected
            .iter()
            .map(|(id, title, color)| {
                (
                    (*id).to_string(),
                    (*title).to_string(),
                    color.map(str::to_string),
                    // This engine holds exactly one published item, and it is
                    // filed under every category of this row.
                    Some(1),
                )
            })
            .collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the catalog's categories {wanted:?}, got {reported:?} \
             in {item}"
        );
    }
}

/// RULE: `Category.appCount` is "the number of published Amazon Q Apps
/// associated with a category", so it is DERIVED from the items filed under it
/// and moves as more are published — it is not a number stored on the category.
#[tokio::test]
async fn category_app_count_counts_the_items_filed_under_it() {
    let engine = build_engine();
    let router = build_router(&engine);
    seed_categories(&engine, INSTANCE);

    // (case, the categories the next publication names, what each category's
    // appCount must be afterwards)
    type CountRow<'a> = (&'a str, &'a [&'a str], &'a [(&'a str, i64)]);
    let cases: [CountRow<'_>; 3] = [
        ("one item under A", &[CATEGORY_A], &[(CATEGORY_A, 1)]),
        (
            "a second item under A and B",
            &[CATEGORY_A, CATEGORY_B],
            &[(CATEGORY_A, 2), (CATEGORY_B, 1)],
        ),
        (
            "a third item under B only",
            &[CATEGORY_B],
            &[(CATEGORY_A, 2), (CATEGORY_B, 2)],
        ),
    ];

    // Each item, with the categories ITS publication named, so the assertions
    // below know which categories that item must report.
    let mut items: Vec<(String, Vec<&str>)> = Vec::new();
    for (case, named, expected_counts) in cases {
        let app_id = create_app(&router, INSTANCE, &caller(0), case).await;
        let (status, body) = publish(&router, INSTANCE, &caller(0), &app_id, 1, named).await;
        assert_eq!(status, 200, "case {case:?}: publishing: {status} {body}");
        items.push((
            body["libraryItemId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no libraryItemId in {body}"))
                .to_string(),
            named.to_vec(),
        ));

        // Every item published so far reports the SAME derived count for a
        // category, however many items were filed after it.
        for (item_id, filed_under) in &items {
            let (status, item) = get_item(&router, INSTANCE, &caller(0), item_id, None).await;
            assert_eq!(status, 200, "case {case:?}: reading {item_id}: {item}");
            for (category_id, expected) in expected_counts {
                let reported = item["categories"]
                    .as_array()
                    .and_then(|entries| {
                        entries
                            .iter()
                            .find(|entry| entry["id"].as_str() == Some(category_id))
                    })
                    .map(|entry| entry["appCount"].as_i64());
                // An item reports exactly the categories it was filed under, and
                // each one carries the derived count.
                let wanted = filed_under
                    .contains(category_id)
                    .then_some(Some(*expected));
                assert_eq!(
                    reported, wanted,
                    "case {case:?}: item {item_id} filed under {filed_under:?} must report \
                     category {category_id} as {wanted:?}, got {reported:?} in {item}"
                );
            }
        }
    }
}
