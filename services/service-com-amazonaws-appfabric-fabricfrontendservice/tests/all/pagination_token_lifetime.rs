// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the LIFETIME of a `nextToken`, over every paginated
//! listing this service serves.
//!
//! The model documents the lifetime on the `nextToken` member of each of them:
//! a pagination token expires after 24 hours, and a call carrying an expired one
//! is refused with an HTTP 400. The age of a token is therefore part of whether
//! a listing honours it, which is what these tests state as a rule over a table
//! of ages — a token younger than the lifetime is a position, one older than it
//! is a refusal — and over all four listings, because they all mint and read
//! their tokens through one helper.
//!
//! An age is set by minting the token at a chosen ISSUE INSTANT through the same
//! `state` helpers the handlers use, so no test has to wait for, or move, the
//! clock.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::state;

use super::app_authorization_and_ingestion_dependency::create_destination;
use super::app_bundle_ingestion_lifecycle::{arn_of, encode, uuid_of, Answer, Emulator};

/// One paginated listing of this service, as everything a test needs to drive it
/// and to mint a token of its own.
pub(crate) struct Listing {
    /// The case name every assertion below reports.
    pub(crate) case: &'static str,
    /// The request path of the listing, without a query string.
    pub(crate) path: String,
    /// The member of the response holding the page.
    pub(crate) items: &'static str,
    /// The listing name its tokens are scoped to, from the `state` helpers.
    pub(crate) listing: String,
}

/// A store holding TWO records in each of the four paginated listings, so every
/// listing really does issue a `nextToken` for a page of one, and the page after
/// that token holds a record.
pub(crate) async fn two_records_in_every_listing(emulator: &Emulator) -> Vec<Listing> {
    // A second bundle, so `ListAppBundles` has two records. A Region holds only
    // one app bundle, hence two Regions.
    let bundle = emulator.fixture_bundle(0).await;
    let _second_bundle = emulator.fixture_bundle(1).await;

    // Two ingestions of two tenants — which also leaves two connected app
    // authorizations under the bundle, one per tenant.
    let mut ingestions: Vec<String> = Vec::new();
    for index in 0..2 {
        let created = emulator
            .create_ingestion(
                &bundle,
                json!({"app": "SLACK", "tenantId": format!("tenant-{index}"),
                       "ingestionType": "auditLog"}),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "fixture ingestion {index}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        ingestions.push(arn_of(&created.body, "ingestion"));
    }

    for index in 0..2 {
        let created =
            create_destination(emulator, &bundle, &ingestions[0], &format!("bucket-{index}")).await;
        assert_eq!(
            created.status, 201,
            "fixture destination {index}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
    }

    vec![
        Listing {
            case: "ListAppBundles",
            path: "/appbundles".to_string(),
            items: "appBundleSummaryList",
            listing: state::APP_BUNDLE_LISTING.to_string(),
        },
        Listing {
            case: "ListIngestions",
            path: format!("/appbundles/{}/ingestions", encode(&bundle)),
            items: "ingestions",
            listing: state::ingestion_listing(&uuid_of(&bundle)),
        },
        Listing {
            case: "ListAppAuthorizations",
            path: format!("/appbundles/{}/appauthorizations", encode(&bundle)),
            items: "appAuthorizationSummaryList",
            listing: state::app_authorization_listing(&uuid_of(&bundle)),
        },
        Listing {
            case: "ListIngestionDestinations",
            path: format!(
                "/appbundles/{}/ingestions/{}/ingestiondestinations",
                encode(&bundle),
                encode(&ingestions[0])
            ),
            items: "ingestionDestinations",
            listing: state::ingestion_destination_listing(&uuid_of(&ingestions[0])),
        },
    ]
}

/// The first page of `listing`, one record wide, with the `nextToken` it hands
/// out for the page after it.
pub(crate) async fn first_page_token(emulator: &Emulator, listing: &Listing) -> String {
    let answer = emulator
        .call("GET", &format!("{}?maxResults=1", listing.path), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: expected HTTP 200 for the first page, got {} with body {}",
        listing.case, answer.status, answer.body
    );
    answer
        .body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a nextToken for a page of one, got {}",
                listing.case, answer.body
            )
        })
        .to_string()
}

/// `listing` replayed with `token`.
pub(crate) async fn replay(emulator: &Emulator, listing: &Listing, token: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{}?maxResults=1&nextToken={}", listing.path, encode(token)),
            None,
        )
        .await
}

/// The same token the service issued for `listing`'s first page, re-minted as if
/// it had been issued `age` ago: the position it names is unchanged, only its
/// issue instant moves.
fn token_aged(listing: &Listing, issued_token: &str, age: Duration) -> String {
    let now = SystemTime::now();
    let sequence = state::decode_page_token(&listing.listing, issued_token, now).unwrap_or_else(
        |refusal| {
            panic!(
                "case {:?}: the token the service issued must decode, got {refusal:?}",
                listing.case
            )
        },
    );
    let issued_at = now
        .checked_sub(age)
        .unwrap_or_else(|| panic!("case {:?}: an issue instant {age:?} ago", listing.case));
    state::encode_page_token(&listing.listing, sequence, issued_at)
}

/// The `message` of a refusal, or `""`.
pub(crate) fn message(answer: &Answer) -> String {
    answer
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// RULE: a `nextToken` is a position only while it is younger than the lifetime
/// the model documents for it; once it is older, the call carrying it is refused
/// with HTTP 400 `ValidationException` naming `nextToken`, instead of being
/// answered with a page.
///
/// The table spans the age of the token — from freshly issued to each side of the
/// published bound and well past it — and every paginated listing of the service,
/// because they all mint and read their tokens through one helper.
#[tokio::test]
async fn a_next_token_expires_after_the_documented_lifetime() {
    // The published figure, from the `nextToken` member's own documentation
    // ("Each pagination token expires after 24 hours"), which the helper under
    // test carries as its lifetime.
    let lifetime = state::PAGE_TOKEN_LIFETIME;
    assert_eq!(
        lifetime,
        Duration::from_secs(24 * 60 * 60),
        "the documented pagination-token lifetime is 24 hours, got {lifetime:?}"
    );

    // (case, how long before the replay the token was issued, expected status)
    let ages: Vec<(&str, Duration, u16)> = vec![
        ("issued at this instant", Duration::ZERO, 200),
        ("a second old", Duration::from_secs(1), 200),
        ("an hour old", Duration::from_secs(60 * 60), 200),
        (
            "a minute inside the bound",
            lifetime - Duration::from_secs(60),
            200,
        ),
        (
            "a second past the bound",
            lifetime + Duration::from_secs(1),
            400,
        ),
        (
            "an hour past the bound",
            lifetime + Duration::from_secs(60 * 60),
            400,
        ),
        ("a week old", Duration::from_secs(7 * 24 * 60 * 60), 400),
    ];

    let emulator = Emulator::new();
    let listings = two_records_in_every_listing(&emulator).await;

    for listing in &listings {
        let issued = first_page_token(&emulator, listing).await;

        // The token exactly as the service handed it out is honoured, so the
        // rows below are about AGE and nothing else.
        let answer = replay(&emulator, listing, &issued).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: a listing must honour the token it just issued, got {} with body {}",
            listing.case, answer.status, answer.body
        );

        for (age_case, age, expected_status) in &ages {
            let case = format!("{} with a token {age_case}", listing.case);
            let token = token_aged(listing, &issued, *age);
            let answer = replay(&emulator, listing, &token).await;
            assert_eq!(
                answer.status, *expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {} with body {}",
                answer.status, answer.body
            );

            if *expected_status == 200 {
                let items = answer
                    .body
                    .get(listing.items)
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected {} in {}", listing.items, answer.body)
                    });
                assert_eq!(
                    items.len(),
                    1,
                    "case {case:?}: expected the one record after the token's position, got {}",
                    answer.body
                );
                continue;
            }

            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {}",
                answer.body
            );
            assert_eq!(
                answer.body.get("reason").and_then(Value::as_str),
                Some("fieldValidationFailed"),
                "case {case:?}: expected the refusal to fault the member, got {}",
                answer.body
            );
            let field = answer
                .body
                .get("fieldList")
                .and_then(Value::as_array)
                .and_then(|list| list.first())
                .and_then(|entry| entry.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                field, "nextToken",
                "case {case:?}: expected the refusal to name nextToken, got {field:?} in {}",
                answer.body
            );
            let refusal = message(&answer);
            assert!(
                refusal.contains("expired") && refusal.contains("24 hours"),
                "case {case:?}: expected the refusal to say the token expired after the \
                 documented lifetime, got {refusal:?}"
            );
        }
    }
}

/// RULE: the two ways a `nextToken` can be bad are reported apart — a token this
/// service never issued is refused as not a valid token, while one it issued and
/// that has since expired is refused as expired — so a caller can tell a token
/// to stop using from a token to re-request.
#[tokio::test]
async fn an_expired_token_is_refused_in_different_words_from_a_token_never_issued() {
    let emulator = Emulator::new();
    let listings = two_records_in_every_listing(&emulator).await;
    let listing = listings
        .first()
        .expect("the fixture builds at least one listing");

    let issued = first_page_token(&emulator, listing).await;
    let expired = replay(
        &emulator,
        listing,
        &token_aged(
            listing,
            &issued,
            state::PAGE_TOKEN_LIFETIME + Duration::from_secs(1),
        ),
    )
    .await;
    assert_eq!(
        expired.status, 400,
        "an expired token must be refused, got {} with body {}",
        expired.status, expired.body
    );

    // Each row is a token shaped unlike anything the service mints, so none of
    // them carries an issue instant at all.
    for malformed in ["not-a-token", "v1-", "v1-appbundles-zzzzzzzzzzzzzzzz"] {
        let answer = replay(&emulator, listing, malformed).await;
        assert_eq!(
            answer.status, 400,
            "case {malformed:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {malformed:?}: expected ValidationException, got {}",
            answer.body
        );
        assert_ne!(
            message(&answer),
            message(&expired),
            "case {malformed:?}: a token the service never issued must not be reported as expired"
        );
        assert!(
            !message(&answer).contains("expired"),
            "case {malformed:?}: expected a refusal that does not claim expiry, got {:?}",
            message(&answer)
        );
    }
}
