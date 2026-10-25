// SPDX-License-Identifier: CC0-1.0

//! `UpdateApiKey`, end to end over HTTP: what an update writes onto a stored API key,
//! and which updates it refuses.
//!
//! Four rules, each asserted over a TABLE spanning the class it generalises over
//! rather than one recorded request:
//!
//! 1. an update applies the members the request carries to the STORED key and leaves
//!    the rest of it as it was — the key keeps its id and its place in the list — and
//!    the body it answers with is the one `ListApiKeys` reports next;
//! 2. the expiration an update names is kept as epoch seconds floored to the hour, and
//!    the deletion time moves with it, whether the new expiration is later or earlier
//!    than the stored one;
//! 3. the expiration has to land between 1 and 365 days from the update, and a value
//!    outside that window is refused with `ApiKeyValidityOutOfBoundsException` (400)
//!    naming the window, leaving the stored key exactly as it was;
//! 4. an update addresses one stored key of one stored API: an `apiId` that cannot name
//!    an API is a bad request (400), while an API or a key that simply is not there is a
//!    not-found (404), and neither writes anything.
//!
//! The harness (`call`, `create_api`, `router`, `Answer`) is the error-contract suite's.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, create_api, router, Answer};

/// One hour in seconds — the granularity `ApiKey$expires` and `ApiKey$deletes` are
/// documented with: both are "seconds since the epoch, rounded down to the nearest
/// hour". Source: `smithy_query(query="shape", shape="ApiKey")`.
const HOUR: i64 = 3_600;

/// One day in seconds — the unit the API-key validity window is stated in.
const DAY: i64 = 86_400;

/// How far inside or outside a validity bound a row sits.
///
/// `UpdateApiKey` judges the submitted `expires` against the clock IT reads when the
/// request arrives, which is necessarily LATER than the clock this test reads to build
/// `expires`; the elapsed time shrinks the validity the handler computes. A row placed
/// exactly on a bound, or one second past it, would therefore flip to the other side of
/// the bound whenever a second elapses first. Each row sits this far inside or outside
/// the bound it exercises — far wider than the one in-process dispatch between this
/// test's clock read (taken per row) and the handler's, and still close enough to pin
/// the bound. The bounds come from `ApiKeyValidityOutOfBoundsException`: "must be set to
/// a value between 1 and 365 days from creation (for CreateApiKey) or from update (for
/// UpdateApiKey)".
const BOUND_SLACK: i64 = 5;

/// AppSync's refusal text for an expiration that is too far off: the upper bound,
/// stated in days, which is the one wording a recorded refusal pins.
const MAXIMUM_REFUSAL: &str = "API key can be valid for a maximum of 365 days.";

/// What a refusal on the LOWER bound has to name. No published wording pins that
/// message, so this asserts the bound it must state rather than a literal text.
const MINIMUM_REFUSAL_TERMS: &[&str] = &["minimum", "1 day"];

/// Now, as whole seconds since the Unix epoch — the form `ApiKey$expires` takes.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// `seconds` rounded DOWN to the nearest hour, the form the two time members take.
fn floor_to_hour(seconds: i64) -> i64 {
    seconds - seconds.rem_euclid(HOUR)
}

/// Create one API key under `api_id` and return the `apiKey` the create answered with.
async fn create_key(
    router: &MultiServiceRouter,
    api_id: &str,
    body: Value,
    case: &str,
) -> Value {
    let answer = call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(body),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected CreateApiKey 200, got {answer:?}"
    );
    answer.body["apiKey"].clone()
}

/// `UpdateApiKey` on one stored key.
async fn update_key(
    router: &MultiServiceRouter,
    api_id: &str,
    key_id: &str,
    body: Value,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys/{key_id}"),
        Some(body),
    )
    .await
}

/// The key `key_id` as `ListApiKeys` reports it, or `Value::Null` when the API holds
/// no such key — the implemented reader this suite round-trips every write through.
async fn listed_key(router: &MultiServiceRouter, api_id: &str, key_id: &str) -> Value {
    let answer = call(router, "GET", &format!("/v1/apis/{api_id}/apikeys"), None).await;
    assert_eq!(
        answer.status, 200,
        "case list keys of {api_id}: expected ListApiKeys 200, got {answer:?}"
    );
    answer.body["apiKeys"]
        .as_array()
        .and_then(|keys| {
            keys.iter()
                .find(|key| key["id"].as_str() == Some(key_id))
                .cloned()
        })
        .unwrap_or(Value::Null)
}

/// The ids `ListApiKeys` reports for `api_id`, in the order it reports them.
async fn listed_ids(router: &MultiServiceRouter, api_id: &str) -> Vec<String> {
    let answer = call(router, "GET", &format!("/v1/apis/{api_id}/apikeys"), None).await;
    assert_eq!(
        answer.status, 200,
        "case list keys of {api_id}: expected ListApiKeys 200, got {answer:?}"
    );
    answer.body["apiKeys"]
        .as_array()
        .map(|keys| {
            keys.iter()
                .map(|key| key["id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Rules 1-3: an accepted update writes the submitted expiration, floored to the hour,
/// with the deletion time moved onto it and the submitted description replacing the
/// stored one; a submitted expiration outside the documented window is refused and
/// writes nothing. Both are asserted on the same table.
///
/// The rows differ along the one thing the rule reads — how far from the update the new
/// expiration is — and span the class: just inside each published bound, values well
/// inside the window, and just outside each bound. The stored expiration each row
/// starts from rotates with it, so an accepted row is sometimes an extension and
/// sometimes a shortening, and the submitted description rotates between a new one and
/// none at all.
#[tokio::test]
async fn an_update_writes_the_submitted_expiration_and_description_onto_the_stored_key() {
    let router = router();
    let api_id = create_api(&router, "Api_key_update_window").await;

    // (case, validity of the stored key, submitted validity, accepted)
    let cases: &[(&str, i64, i64, bool)] = &[
        ("extend to just inside a day", 2 * DAY, DAY + BOUND_SLACK, true),
        ("extend to thirty days", 2 * DAY, 30 * DAY, true),
        ("extend to a hundred days", 30 * DAY, 100 * DAY, true),
        (
            "extend to just inside a year",
            100 * DAY,
            365 * DAY - BOUND_SLACK,
            true,
        ),
        ("shorten to two days", 300 * DAY, 2 * DAY, true),
        (
            "refuse just short of a day",
            30 * DAY,
            DAY - BOUND_SLACK,
            false,
        ),
        (
            "refuse just past a year",
            30 * DAY,
            365 * DAY + BOUND_SLACK,
            false,
        ),
        ("refuse an expiration already past", 30 * DAY, -DAY, false),
    ];

    for (index, (case, stored_validity, submitted_validity, accepted)) in
        cases.iter().enumerate()
    {
        let created = create_key(
            &router,
            &api_id,
            json!({ "description": format!("{case}: before"), "expires": unix_now() + stored_validity }),
            case,
        )
        .await;
        let key_id = created["id"].as_str().unwrap_or_default().to_string();
        let before = listed_key(&router, &api_id, &key_id).await;

        // Every other row submits a new description, so the rule is asserted both with
        // one and without one.
        let submits_description = index % 2 == 0;
        // Read the clock per row, so the window between this read and the handler's own
        // is one dispatch rather than the whole table.
        let submitted = unix_now() + submitted_validity;
        let body = if submits_description {
            json!({ "description": format!("{case}: after"), "expires": submitted })
        } else {
            json!({ "expires": submitted })
        };
        let answer = update_key(&router, &api_id, &key_id, body).await;

        if !*accepted {
            assert_eq!(
                answer.status, 400,
                "case {case}: expected UpdateApiKey 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_type, "ApiKeyValidityOutOfBoundsException",
                "case {case}: expected error type ApiKeyValidityOutOfBoundsException, \
                 got {answer:?}"
            );
            if *submitted_validity < DAY {
                for term in MINIMUM_REFUSAL_TERMS {
                    assert!(
                        answer.message.contains(term),
                        "case {case}: expected the refusal to name the lower bound \
                         ({term:?}), got {answer:?}"
                    );
                }
            } else {
                assert_eq!(
                    answer.message, MAXIMUM_REFUSAL,
                    "case {case}: expected the refusal to name the upper bound \
                     ({MAXIMUM_REFUSAL:?}), got {answer:?}"
                );
            }
            assert_eq!(
                listed_key(&router, &api_id, &key_id).await,
                before,
                "case {case}: a refused update must write nothing, got {:?}",
                listed_key(&router, &api_id, &key_id).await
            );
            continue;
        }

        assert_eq!(
            answer.status, 200,
            "case {case}: expected UpdateApiKey 200, got {answer:?}"
        );
        let updated = answer.body["apiKey"].clone();
        assert_eq!(
            updated["id"].as_str(),
            Some(key_id.as_str()),
            "case {case}: an update keeps the key's id, got {updated:?}"
        );
        let expected_expires = floor_to_hour(submitted);
        assert_eq!(
            updated["expires"].as_i64(),
            Some(expected_expires),
            "case {case}: expected the submitted expiration floored to the hour \
             ({expected_expires}), got {updated:?}"
        );
        assert_eq!(
            updated["deletes"].as_i64(),
            Some(expected_expires),
            "case {case}: the deletion time must move onto the new expiration \
             ({expected_expires}), got {updated:?}"
        );
        let expected_description = if submits_description {
            format!("{case}: after")
        } else {
            format!("{case}: before")
        };
        assert_eq!(
            updated["description"].as_str(),
            Some(expected_description.as_str()),
            "case {case}: expected description {expected_description:?}, got {updated:?}"
        );
        assert_eq!(
            listed_key(&router, &api_id, &key_id).await,
            updated,
            "case {case}: ListApiKeys must report the key the update stored, got {:?}",
            listed_key(&router, &api_id, &key_id).await
        );
    }
}

/// Rule 2, as an invariant over a WALK: whatever sequence of accepted expirations a key
/// is walked through, the key reports the last one, floored to the hour, with its
/// deletion time on it — the stored expiration an update starts from never takes part.
#[tokio::test]
async fn the_deletion_time_follows_the_expiration_through_every_update() {
    let router = router();
    let api_id = create_api(&router, "Api_key_update_walk").await;
    let created = create_key(
        &router,
        &api_id,
        json!({ "description": "walked", "expires": unix_now() + 7 * DAY }),
        "walk",
    )
    .await;
    let key_id = created["id"].as_str().unwrap_or_default().to_string();

    // Validities in no particular order, so each step is an extension or a shortening
    // of whatever the step before it left: 11 hours past a whole day exercises the
    // hour flooring on a value that is not already on an hour boundary.
    let walk: &[i64] = &[
        DAY + BOUND_SLACK,
        365 * DAY - BOUND_SLACK,
        30 * DAY + 11 * HOUR + 59,
        3 * DAY,
        90 * DAY,
        DAY + BOUND_SLACK,
    ];

    for validity in walk {
        let submitted = unix_now() + validity;
        let case = format!("walk to {validity} seconds");
        let answer = update_key(&router, &api_id, &key_id, json!({ "expires": submitted })).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected UpdateApiKey 200, got {answer:?}"
        );
        let expected = floor_to_hour(submitted);
        let key = answer.body["apiKey"].clone();
        assert_eq!(
            key["expires"].as_i64(),
            Some(expected),
            "case {case}: expected expires {expected}, got {key:?}"
        );
        assert_eq!(
            key["deletes"].as_i64(),
            key["expires"].as_i64(),
            "case {case}: deletes must equal the expiration it follows, got {key:?}"
        );
        assert_eq!(
            listed_key(&router, &api_id, &key_id).await,
            key,
            "case {case}: ListApiKeys must report the key the update stored, got {:?}",
            listed_key(&router, &api_id, &key_id).await
        );
    }
}

/// Rule 1, on the members an update does NOT carry: `expires` arrives as its modelled
/// default (`0`) when the request names none, which names no new expiration, so the
/// stored one stands and the description alone is written.
///
/// The rows differ along the stored expiration the key starts with, including one that
/// is not on an hour boundary, so the kept value is the stored one rather than anything
/// recomputed from the request or the clock.
#[tokio::test]
async fn an_update_that_names_no_expiration_keeps_the_stored_one() {
    let router = router();
    let api_id = create_api(&router, "Api_key_update_partial").await;

    let cases: &[(&str, i64)] = &[
        ("a stored day", DAY + BOUND_SLACK),
        ("a stored fortnight", 14 * DAY),
        ("a stored odd instant", 45 * DAY + 7 * HOUR + 13),
        ("a stored year", 365 * DAY - BOUND_SLACK),
    ];

    for (case, validity) in cases {
        let created = create_key(
            &router,
            &api_id,
            json!({ "description": "before", "expires": unix_now() + validity }),
            case,
        )
        .await;
        let key_id = created["id"].as_str().unwrap_or_default().to_string();
        let stored_expires = created["expires"].as_i64().unwrap_or_default();

        let answer = update_key(
            &router,
            &api_id,
            &key_id,
            json!({ "description": "after" }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected UpdateApiKey 200, got {answer:?}"
        );
        let updated = answer.body["apiKey"].clone();
        assert_eq!(
            updated["expires"].as_i64(),
            Some(stored_expires),
            "case {case}: expected the stored expiration {stored_expires} to stand, \
             got {updated:?}"
        );
        assert_eq!(
            updated["deletes"].as_i64(),
            Some(stored_expires),
            "case {case}: expected the stored deletion time {stored_expires} to stand, \
             got {updated:?}"
        );
        assert_eq!(
            updated["description"].as_str(),
            Some("after"),
            "case {case}: expected the submitted description, got {updated:?}"
        );
        assert_eq!(
            listed_key(&router, &api_id, &key_id).await,
            updated,
            "case {case}: ListApiKeys must report the key the update stored, got {:?}",
            listed_key(&router, &api_id, &key_id).await
        );
    }
}

/// Rule 4: an update resolves one stored key of one stored API, and the ids it cannot
/// resolve are refused by the same contract every operation that resolves an `apiId`
/// follows — an id too long to name an API is a bad request, an id that names nothing
/// is a not-found — with a key the addressed API does not hold a not-found too.
///
/// The rows differ along WHICH part of the address cannot be resolved, including a key
/// that exists but under another API, and every row asserts that the keys the emulator
/// does hold are untouched.
#[tokio::test]
async fn an_update_resolves_one_stored_key_of_one_stored_api() {
    let router = router();
    let api_id = create_api(&router, "Api_key_update_address").await;
    let other_api_id = create_api(&router, "Api_key_update_other").await;

    let mine = create_key(
        &router,
        &api_id,
        json!({ "description": "mine", "expires": unix_now() + 30 * DAY }),
        "addressing: mine",
    )
    .await;
    let my_key_id = mine["id"].as_str().unwrap_or_default().to_string();
    let theirs = create_key(
        &router,
        &other_api_id,
        json!({ "description": "theirs", "expires": unix_now() + 30 * DAY }),
        "addressing: theirs",
    )
    .await;
    let their_key_id = theirs["id"].as_str().unwrap_or_default().to_string();

    // An AppSync API id is 26 characters; an id longer than that cannot name one.
    // Source: the ids `CreateGraphqlApi` mints, and `error_contract.rs`'s rule 1.
    let too_long = "d".repeat(27);
    let unknown_api = "nonexistent0000000000";

    // (case, apiId, key id, expected status, expected error shape)
    let cases: &[(&str, &str, &str, u16, &str)] = &[
        (
            "an apiId too long to name an API",
            &too_long,
            &my_key_id,
            400,
            "BadRequestException",
        ),
        (
            "an apiId that names no API",
            unknown_api,
            &my_key_id,
            404,
            "NotFoundException",
        ),
        (
            "a key id the API does not hold",
            &api_id,
            "da2-nosuchkey000000000",
            404,
            "NotFoundException",
        ),
        (
            "a key held by another API",
            &api_id,
            &their_key_id,
            404,
            "NotFoundException",
        ),
        (
            "an empty-looking key id",
            &api_id,
            "da2-",
            404,
            "NotFoundException",
        ),
    ];

    for (case, addressed_api, addressed_key, status, error_type) in cases {
        let answer = update_key(
            &router,
            addressed_api,
            addressed_key,
            json!({ "description": "written by a refused update", "expires": unix_now() + 2 * DAY }),
        )
        .await;
        assert_eq!(
            answer.status, *status,
            "case {case}: expected status {status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, *error_type,
            "case {case}: expected error type {error_type}, got {answer:?}"
        );
        assert_eq!(
            listed_key(&router, &api_id, &my_key_id).await,
            mine,
            "case {case}: a refused update must leave the stored key as it was, got {:?}",
            listed_key(&router, &api_id, &my_key_id).await
        );
        assert_eq!(
            listed_key(&router, &other_api_id, &their_key_id).await,
            theirs,
            "case {case}: a refused update must leave the other API's key as it was, \
             got {:?}",
            listed_key(&router, &other_api_id, &their_key_id).await
        );
    }

    // The precedence between the two refusals: the submitted expiration is judged
    // before the address is resolved, exactly as `CreateApiKey` judges it, so a request
    // that is wrong in both ways takes the window's refusal.
    for (case, addressed_api, addressed_key) in [
        ("an unknown API and an expiration past the window", unknown_api, my_key_id.as_str()),
        ("an unknown key and an expiration past the window", api_id.as_str(), "da2-nosuchkey000000000"),
    ] {
        let answer = update_key(
            &router,
            addressed_api,
            addressed_key,
            json!({ "expires": unix_now() + 400 * DAY }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected the validity refusal first (400), got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "ApiKeyValidityOutOfBoundsException",
            "case {case}: expected error type ApiKeyValidityOutOfBoundsException, got {answer:?}"
        );
    }

    // Rule 1 again, on the list: an accepted update replaces no key and adds none, so
    // the API reports exactly the keys it held, in the order it held them.
    let before = listed_ids(&router, &api_id).await;
    let answer = update_key(
        &router,
        &api_id,
        &my_key_id,
        json!({ "expires": unix_now() + 2 * DAY }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case an accepted update: expected UpdateApiKey 200, got {answer:?}"
    );
    assert_eq!(
        listed_ids(&router, &api_id).await,
        before,
        "case an accepted update: expected the same keys in the same order as {before:?}, \
         got {:?}",
        listed_ids(&router, &api_id).await
    );
}
