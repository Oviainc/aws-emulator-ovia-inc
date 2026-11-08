// SPDX-License-Identifier: CC0-1.0

//! The deletion time an API key reports, end to end over HTTP: one value per key,
//! derived from the key's expiration.
//!
//! Two rules, each asserted over a TABLE spanning the class it generalises over rather
//! than one recorded request:
//!
//! 1. a key's `deletes` is its `expires` plus the post-expiry retention the service
//!    keeps an expired key for, floored to the hour — the key is kept for 60 days after
//!    it expires, so the two members are exactly one retention span apart;
//! 2. the derivation is the key's, not the reporting operation's: `CreateApiKey`,
//!    `UpdateApiKey` and `ListApiKeys` all report the same `deletes` for the same key;
//! 3. rules 1 and 2 together make `deletes` a FUNCTION of the key's expiration alone:
//!    two keys whose expirations floor to the same hour report the same deletion time,
//!    whichever API of whichever resource family holds them, whichever call set the
//!    expiration, and however often they are read.
//!
//! The rows differ along the one thing rule 1 reads — the key's expiration: each
//! published validity bound and values well inside the window, instants on and off an
//! hour boundary, and a request that names no expiration at all and takes the
//! documented default.
//!
//! The harness (`call`, `create_api`, `router`, `Answer`) is the error-contract suite's.
//! This suite's own constants, helpers and rule-1 assertion are `pub(crate)`, so the
//! sibling suite that varies rule 2's dimension (`api_key_deletes_listing_agreement`)
//! asserts the same arithmetic from the same single place rather than a second copy.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, create_api, router, Answer};
use crate::event_api::{create_event_api, event_config};

/// One hour in seconds — the granularity `ApiKey$expires` and `ApiKey$deletes` are
/// documented with: both are "seconds since the epoch, rounded down to the nearest
/// hour". Source: `smithy_query(query="doc", shape="ApiKey", member="deletes")`.
pub(crate) const HOUR: i64 = 3_600;

/// One day in seconds — the unit the API-key validity window is stated in.
pub(crate) const DAY: i64 = 86_400;

/// How long an expired API key is kept before the service reclaims it — the span
/// between a key's `expires` and its `deletes`. Source: the `ApiKey` shape's doc for
/// the `da2` key version this service mints, "Expired API keys are kept for 60 days
/// after the expiration time" (`smithy_query(query="doc", shape="ApiKey")`), which
/// `ListApiKeys` restates as "API keys are deleted automatically 60 days after they
/// expire".
pub(crate) const RETENTION: i64 = 60 * DAY;

/// `CreateApiKey`'s documented default validity: "The default value for this parameter
/// is 7 days from creation time."
pub(crate) const DEFAULT_VALIDITY: i64 = 7 * DAY;

/// The accepted validity window, measured from the call that sets the expiration — the
/// class every row below draws its expiration from. Source:
/// `ApiKeyValidityOutOfBoundsException` — "The API key expiration must be set to a value
/// between 1 and 365 days from creation (for CreateApiKey) or from update (for
/// UpdateApiKey)".
pub(crate) const MIN_VALIDITY: i64 = DAY;
pub(crate) const MAX_VALIDITY: i64 = 365 * DAY;

/// How far inside a published validity bound a row sits, so the seconds elapsing
/// between this test's clock read and the handler's — across every write a row makes —
/// cannot push the row across the bound. The bounds come from
/// `ApiKeyValidityOutOfBoundsException` — "between 1 and 365 days from creation (for
/// CreateApiKey) or from update (for UpdateApiKey)".
pub(crate) const BOUND_SLACK: i64 = 60;

/// Now, as whole seconds since the Unix epoch — the form `ApiKey$expires` takes.
pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// `seconds` rounded DOWN to the nearest hour, the form both time members take.
pub(crate) fn floor_to_hour(seconds: i64) -> i64 {
    seconds - seconds.rem_euclid(HOUR)
}

/// The deletion time the service owes a key whose expiration is `expires`: rule 1.
pub(crate) fn expected_deletes(expires: i64) -> i64 {
    floor_to_hour(expires + RETENTION)
}

/// One row of the rule-3 table: its label, and the expiration it submits, as a
/// function of the clock the row itself reads.
type ExpirationRow = (&'static str, fn(i64) -> i64);

/// Create one Event API and return the `apiId` the service minted for it — an API of
/// the OTHER resource family whose ids share the API-key operations' `apiId` label.
///
/// Both the create call and the configuration it carries are the Event-API suite's
/// (`crate::event_api`), here authorized by API key.
async fn event_api_id(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "eventConfig": event_config(&["API_KEY"], &["API_KEY"], &["API_KEY"], &["API_KEY"], None),
    });
    let answer = create_event_api(router, body).await;
    assert_eq!(
        answer.status, 200,
        "case create the Event API {name:?}: expected CreateApi 200, got {answer:?}"
    );
    answer.body["api"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `CreateApiKey` under `api_id`.
pub(crate) async fn create_key(router: &MultiServiceRouter, api_id: &str, body: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(body),
    )
    .await
}

/// `UpdateApiKey` on one stored key.
pub(crate) async fn update_key(
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

/// The key `key_id` as `ListApiKeys` reports it, or `Value::Null` when the API holds no
/// such key.
pub(crate) async fn listed_key(router: &MultiServiceRouter, api_id: &str, key_id: &str) -> Value {
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

/// Assert rule 1 on one reported key, and return its expiration.
pub(crate) fn assert_retention_span(case: &str, source: &str, key: &Value) -> i64 {
    let expires = key["expires"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?} from {source}: expected an expires, got {key:?}"));
    let deletes = key["deletes"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?} from {source}: expected a deletes, got {key:?}"));
    assert_eq!(
        deletes,
        expected_deletes(expires),
        "case {case:?} from {source}: expected deletes {} (the expiration {expires} plus \
         the retention, floored to the hour), got {deletes}",
        expected_deletes(expires)
    );
    assert_eq!(
        deletes - expires,
        RETENTION,
        "case {case:?} from {source}: expected the two members exactly {RETENTION} \
         seconds apart, got {deletes} and {expires}"
    );
    assert_eq!(
        deletes.rem_euclid(HOUR),
        0,
        "case {case:?} from {source}: expected deletes on an hour boundary, got {deletes}"
    );
    expires
}

/// Rules 1 and 2 on a created key: the deletion time a create mints is the expiration
/// plus the retention, and the listing reports the same one.
///
/// The rows differ along the submitted expiration — both published bounds, values well
/// inside the window, an instant already on an hour boundary and ones off it, and a
/// request naming no expiration at all.
#[tokio::test]
async fn a_created_key_reports_its_expiration_plus_the_retention() {
    let router = router();
    let api_id = create_api(&router, "Api_key_deletion_window_create").await;

    // (case, validity in seconds past the submitting clock read, or None for the
    // documented default)
    let cases: &[(&str, Option<i64>)] = &[
        ("just inside a day", Some(DAY + BOUND_SLACK)),
        ("two days", Some(2 * DAY)),
        ("a fortnight", Some(14 * DAY)),
        ("sixty days, the retention span itself", Some(60 * DAY)),
        ("a hundred days and an odd instant", Some(100 * DAY + 7 * HOUR + 13)),
        ("two hundred days on an hour boundary", Some(200 * DAY + 3 * HOUR)),
        ("just inside a year", Some(365 * DAY - BOUND_SLACK)),
        ("no expiration at all", None),
    ];

    for (case, validity) in cases {
        let before = unix_now();
        let mut body = json!({ "description": *case });
        if let Some(validity) = validity {
            body["expires"] = json!(before + validity);
        }
        let answer = create_key(&router, &api_id, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateApiKey 200, got {answer:?}"
        );
        let created = answer.body["apiKey"].clone();
        let key_id = created["id"].as_str().unwrap_or_default().to_string();

        let expires = assert_retention_span(case, "CreateApiKey", &created);
        match validity {
            Some(validity) => assert_eq!(
                expires,
                floor_to_hour(before + validity),
                "case {case:?}: expected the submitted expiration floored to the hour, \
                 got {created:?}"
            ),
            // The handler reads its own clock, which sits at or after this test's.
            None => assert!(
                (floor_to_hour(before + DEFAULT_VALIDITY)
                    ..=floor_to_hour(unix_now() + DEFAULT_VALIDITY))
                    .contains(&expires),
                "case {case:?}: expected the documented 7-day default expiration, \
                 got {created:?}"
            ),
        }

        // Rule 2: the listing projects the stored key, so it reports the same value.
        let listed = listed_key(&router, &api_id, &key_id).await;
        assert_retention_span(case, "ListApiKeys", &listed);
        assert_eq!(
            listed, created,
            "case {case:?}: ListApiKeys must report the key CreateApiKey stored, \
             got {listed:?}"
        );
    }
}

/// Rules 1 and 2 across an update: a new expiration carries a new deletion time, and
/// the create, the update and the listing never disagree about a key.
///
/// The walk's steps are extensions and shortenings in no particular order, including an
/// instant off an hour boundary, so the reported deletion time is derived from the
/// expiration each step left rather than from the one before it.
#[tokio::test]
async fn an_updated_key_reports_one_deletion_time_to_every_operation() {
    let router = router();
    let api_id = create_api(&router, "Api_key_deletion_window_update").await;

    let answer = create_key(
        &router,
        &api_id,
        json!({ "description": "walked", "expires": unix_now() + 30 * DAY }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case walk start: expected CreateApiKey 200, got {answer:?}"
    );
    let created = answer.body["apiKey"].clone();
    let key_id = created["id"].as_str().unwrap_or_default().to_string();
    assert_retention_span("walk start", "CreateApiKey", &created);

    let walk: &[i64] = &[
        DAY + BOUND_SLACK,
        365 * DAY - BOUND_SLACK,
        60 * DAY,
        45 * DAY + 11 * HOUR + 59,
        3 * DAY,
        90 * DAY + 1,
    ];

    for validity in walk {
        let case = format!("walk to {validity} seconds");
        let submitted = unix_now() + validity;
        let answer = update_key(&router, &api_id, &key_id, json!({ "expires": submitted })).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateApiKey 200, got {answer:?}"
        );
        let updated = answer.body["apiKey"].clone();
        let expires = assert_retention_span(&case, "UpdateApiKey", &updated);
        assert_eq!(
            expires,
            floor_to_hour(submitted),
            "case {case:?}: expected the submitted expiration floored to the hour, \
             got {updated:?}"
        );

        let listed = listed_key(&router, &api_id, &key_id).await;
        assert_retention_span(&case, "ListApiKeys", &listed);
        assert_eq!(
            listed, updated,
            "case {case:?}: ListApiKeys must report the key UpdateApiKey stored, \
             got {listed:?}"
        );
    }

    // An update that names no expiration leaves both time members as they were, so the
    // listing still agrees with the last accepted write.
    let before = listed_key(&router, &api_id, &key_id).await;
    let answer = update_key(&router, &api_id, &key_id, json!({ "description": "renamed" })).await;
    assert_eq!(
        answer.status, 200,
        "case an update naming no expiration: expected UpdateApiKey 200, got {answer:?}"
    );
    let updated = answer.body["apiKey"].clone();
    assert_eq!(
        updated["expires"].as_i64(),
        before["expires"].as_i64(),
        "case an update naming no expiration: expected the stored expiration to stand, \
         got {updated:?}"
    );
    assert_retention_span("an update naming no expiration", "UpdateApiKey", &updated);
    assert_eq!(
        updated["deletes"].as_i64(),
        before["deletes"].as_i64(),
        "case an update naming no expiration: expected the stored deletion time to \
         stand, got {updated:?}"
    );
    assert_eq!(
        listed_key(&router, &api_id, &key_id).await,
        updated,
        "case an update naming no expiration: ListApiKeys must report the stored key, \
         got {:?}",
        listed_key(&router, &api_id, &key_id).await
    );
}

/// Rule 3: the deletion time is a function of the expiration ALONE.
///
/// Each row submits ONE absolute expiration to keys held by three different APIs —
/// two `GraphqlApi`s and an `Api` (an Event API), so the two resource families whose
/// ids share the `apiId` label are both covered — and then to a key that reaches the
/// same expiration through `UpdateApiKey` instead of `CreateApiKey`. All of them must
/// report one expiration and one deletion time, so nothing but the expiration takes
/// part in the derivation.
///
/// The rows differ along the submitted instant's position in its hour (already on the
/// boundary, one second past it, one second short of the next one, mid-hour) and along
/// how far out it sits, including both published validity bounds.
#[tokio::test]
async fn the_deletion_time_is_a_function_of_the_expiration_alone() {
    let router = router();
    let holders: [(&str, String); 3] = [
        (
            "a GraphQL API",
            create_api(&router, "Api_key_deletes_function_first").await,
        ),
        (
            "a second GraphQL API",
            create_api(&router, "Api_key_deletes_function_second").await,
        ),
        (
            "an Event API",
            event_api_id(&router, "Api_key_deletes_function_events").await,
        ),
    ];

    // Each row names the instant it submits as a function of the clock, so the row
    // reads the clock itself: a row sitting just inside a validity bound must be
    // measured from an instant close to the calls it makes, not from one the earlier
    // rows have left behind.
    let cases: &[ExpirationRow] = &[
        ("an instant already on an hour boundary", |now| {
            floor_to_hour(now + 30 * DAY)
        }),
        ("one second past an hour boundary", |now| {
            floor_to_hour(now + 30 * DAY) + 1
        }),
        ("one second short of the next hour", |now| {
            floor_to_hour(now + 30 * DAY) + HOUR - 1
        }),
        ("an instant mid-hour", |now| now + 45 * DAY + 1_234),
        ("just inside the minimum validity", |now| {
            now + DAY + BOUND_SLACK
        }),
        ("just inside the maximum validity", |now| {
            now + 365 * DAY - BOUND_SLACK
        }),
    ];

    for (case, instant) in cases {
        let case = *case;
        let now = unix_now();
        let submitted = instant(now);
        let owed = expected_deletes(floor_to_hour(submitted));

        for (holder, api_id) in &holders {
            let answer = create_key(
                &router,
                api_id,
                json!({ "description": case, "expires": submitted }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} on {holder}: expected CreateApiKey 200, got {answer:?}"
            );
            let created = answer.body["apiKey"].clone();
            let key_id = created["id"].as_str().unwrap_or_default().to_string();
            let expires = assert_retention_span(case, holder, &created);
            assert_eq!(
                expires,
                floor_to_hour(submitted),
                "case {case:?} on {holder}: expected the submitted expiration floored to \
                 the hour, got {created:?}"
            );
            assert_eq!(
                created["deletes"].as_i64(),
                Some(owed),
                "case {case:?} on {holder}: expected deletes {owed} for this expiration, \
                 got {created:?}"
            );

            // Reading the key twice reports the same derived instant both times: the
            // listing projects the stored key rather than recomputing it per call.
            let listed = listed_key(&router, api_id, &key_id).await;
            assert_eq!(
                listed, created,
                "case {case:?} on {holder}: ListApiKeys must report the key CreateApiKey \
                 stored, got {listed:?}"
            );
            assert_eq!(
                listed_key(&router, api_id, &key_id).await,
                created,
                "case {case:?} on {holder}: a second listing must report the same key, \
                 got {listed:?}"
            );
        }

        // The same expiration reached by an UPDATE derives the same deletion time: the
        // call that set it is not part of the derivation either.
        let (holder, api_id) = &holders[0];
        let answer = create_key(
            &router,
            api_id,
            json!({ "description": "updated later", "expires": now + 7 * DAY }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} on {holder} before the update: expected CreateApiKey 200, \
             got {answer:?}"
        );
        let key_id = answer.body["apiKey"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let answer = update_key(&router, api_id, &key_id, json!({ "expires": submitted })).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?} on {holder}: expected UpdateApiKey 200, got {answer:?}"
        );
        let updated = answer.body["apiKey"].clone();
        assert_retention_span(case, "UpdateApiKey", &updated);
        assert_eq!(
            updated["deletes"].as_i64(),
            Some(owed),
            "case {case:?} on {holder}: expected an updated key to report deletes {owed}, \
             the same value a created key with this expiration reports, got {updated:?}"
        );
        assert_eq!(
            listed_key(&router, api_id, &key_id).await,
            updated,
            "case {case:?} on {holder}: ListApiKeys must report the key UpdateApiKey \
             stored, got {updated:?}"
        );
    }
}

/// Rule 2 over a PAGED listing: every key of an API reports the deletion time its own
/// expiration derives, whichever page it is read on, and reading the keys again reports
/// the same values — a listing is a read.
#[tokio::test]
async fn every_listed_key_reports_the_deletion_time_its_expiration_derives() {
    let router = router();
    let api_id = create_api(&router, "Api_key_deletion_window_list").await;

    // Several keys with different expirations, so one listing spans the class.
    let validities: &[i64] = &[
        DAY + BOUND_SLACK,
        7 * DAY,
        30 * DAY + 17 * HOUR + 41,
        60 * DAY,
        180 * DAY,
        365 * DAY - BOUND_SLACK,
    ];
    let mut created: Vec<Value> = Vec::new();
    for validity in validities {
        let answer = create_key(
            &router,
            &api_id,
            json!({ "description": format!("key for {validity} seconds"), "expires": unix_now() + validity }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case key for {validity} seconds: expected CreateApiKey 200, got {answer:?}"
        );
        created.push(answer.body["apiKey"].clone());
    }

    // Page the listing one key at a time, so the projection is exercised on every page
    // rather than only on the first.
    let mut paged: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    for page in 0..validities.len() {
        let case = format!("page {page}");
        let path = match &next_token {
            Some(token) => format!("/v1/apis/{api_id}/apikeys?maxResults=1&nextToken={token}"),
            None => format!("/v1/apis/{api_id}/apikeys?maxResults=1"),
        };
        let answer = call(&router, "GET", &path, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListApiKeys 200, got {answer:?}"
        );
        let keys = answer.body["apiKeys"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            keys.len(),
            1,
            "case {case:?}: expected one key per page, got {answer:?}"
        );
        assert_retention_span(&case, "ListApiKeys", &keys[0]);
        paged.push(keys[0].clone());
        next_token = answer.body["nextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }

    assert_eq!(
        paged, created,
        "case the whole listing: expected the keys the creates stored, in creation \
         order, got {paged:?}"
    );

    // The same read again answers the same thing: nothing about a key moved because it
    // was listed.
    let again = call(&router, "GET", &format!("/v1/apis/{api_id}/apikeys"), None).await;
    assert_eq!(
        again.status, 200,
        "case a repeated listing: expected ListApiKeys 200, got {again:?}"
    );
    assert_eq!(
        again.body["apiKeys"].as_array().cloned().unwrap_or_default(),
        created,
        "case a repeated listing: expected the same keys with the same deletion times, \
         got {again:?}"
    );
}
