// SPDX-License-Identifier: CC0-1.0

//! The deletion time `ListApiKeys` projects for a key, over the WHOLE class of
//! expirations the service accepts, with the expirations DRAWN rather than hand-picked.
//!
//! The invariant: for every expiration inside the accepted validity window, the key the
//! listing reports carries `expires` equal to the submitted instant floored to the hour
//! and `deletes` equal to that expiration plus the post-expiry retention, floored to the
//! hour — one retention span, whatever the instant is and wherever in its hour it sits.
//! An expiration OUTSIDE that window mints no key at all, so the derivation has nothing
//! to report for it. Together that makes `deletes` a function of the expiration over the
//! whole window rather than over the handful of instants a table can name: the epoch
//! seconds of any one run are one instance of the template, never the template.
//!
//! The generator draws from the window `ApiKeyValidityOutOfBoundsException` publishes —
//! "The API key expiration must be set to a value between 1 and 365 days from creation
//! (for CreateApiKey) or from update (for UpdateApiKey)" — with a bounded number of
//! cases, nothing written to disk, and the drawn seed carried in every assertion message
//! so a failure replays exactly. Both published bounds, and instants at each position in
//! their hour, are covered as well, and a draw just past each bound — and one well past
//! it — is asserted to be refused.
//!
//! The arithmetic itself, its constants and their sources are
//! [`crate::api_key_deletion_window`]'s, reused here rather than restated; what this
//! suite adds is that the rule holds for an expiration nobody chose. Every request goes
//! through the router the production emulator mounts.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{json, Value};

use crate::api_key_deletion_window::{
    assert_retention_span, create_key, expected_deletes, floor_to_hour, listed_key, unix_now,
    update_key, BOUND_SLACK, DAY, HOUR, MAX_VALIDITY, MIN_VALIDITY, RETENTION,
};
use crate::error_contract::{call, create_api, router};

/// The key version this service mints, which the `ApiKey` shape's documentation states
/// the expiration and deletion times are reported for: "`da2` keys ... ListApiKeys
/// returns the expiration time and deletion time in seconds"
/// (`smithy_query(query="doc", shape="ApiKey")`).
const KEY_VERSION_PREFIX: &str = "da2-";

/// How many expirations are drawn from inside the window, beside the bound and
/// position-in-hour cases. Bounded so the suite stays a test, and well under the number
/// of keys one API holds.
const DRAWN_EXPIRATIONS: usize = 8;

/// The validity range a row that also varies the instant's position inside its hour
/// draws from: the flooring moves the submitted instant by up to an hour either way, so a
/// row has to stay that much further inside each published bound to be a case about the
/// derivation rather than about the window.
const WITHIN_HOUR_RANGE: std::ops::RangeInclusive<i64> =
    MIN_VALIDITY + HOUR + BOUND_SLACK..=MAX_VALIDITY - HOUR - BOUND_SLACK;

/// How one case names the instant it submits, as a function of the clock the case itself
/// reads — a case sitting just inside a bound has to be measured from an instant close to
/// the call it makes, not from one the earlier cases left behind.
#[derive(Debug, Clone, Copy)]
enum Instant {
    /// An expiration that many seconds past the clock read.
    Validity(i64),
    /// An expiration that many seconds past the hour boundary the validity lands in, so
    /// the instant's position inside its hour is what the case varies.
    WithinHour { validity: i64, within: i64 },
}

impl Instant {
    /// The absolute expiration this case submits, given the clock it read.
    fn submitted(self, now: i64) -> i64 {
        match self {
            Self::Validity(validity) => now + validity,
            Self::WithinHour { validity, within } => floor_to_hour(now + validity) + within,
        }
    }
}

/// One generated case: its label and the instant it submits.
struct Drawn {
    case: String,
    instant: Instant,
}

/// The cases one run asserts: both published bounds, every position in the hour that the
/// flooring distinguishes, and [`DRAWN_EXPIRATIONS`] expirations drawn from inside the
/// window.
fn drawn_cases(rng: &mut StdRng) -> Vec<Drawn> {
    let mut cases = vec![
        Drawn {
            case: "just inside the minimum validity".to_string(),
            instant: Instant::Validity(MIN_VALIDITY + BOUND_SLACK),
        },
        Drawn {
            case: "just inside the maximum validity".to_string(),
            instant: Instant::Validity(MAX_VALIDITY - BOUND_SLACK),
        },
    ];
    // The positions in an hour the flooring has to treat alike: the boundary itself, the
    // second after it, the second before the next one, and a drawn second between them.
    let drawn_within = rng.random_range(1..HOUR - 1);
    for within in [0, 1, HOUR - 1, drawn_within] {
        let validity = rng.random_range(WITHIN_HOUR_RANGE);
        cases.push(Drawn {
            case: format!("{within} seconds into the hour of a {validity}-second validity"),
            instant: Instant::WithinHour { validity, within },
        });
    }
    for _ in 0..DRAWN_EXPIRATIONS {
        let validity = rng.random_range(MIN_VALIDITY + BOUND_SLACK..=MAX_VALIDITY - BOUND_SLACK);
        cases.push(Drawn {
            case: format!("a drawn validity of {validity} seconds"),
            instant: Instant::Validity(validity),
        });
    }
    cases
}

/// One stored key: the case that minted it, its id, and the projection its last write
/// reported.
struct Stored {
    case: String,
    key_id: String,
    last: Value,
}

/// Assert one reported key carries the expiration that was submitted and the deletion
/// time that expiration derives, and return the key as reported.
fn assert_projection(case: &str, source: &str, submitted: i64, key: &Value) {
    let expires = assert_retention_span(case, source, key);
    assert_eq!(
        expires,
        floor_to_hour(submitted),
        "case {case:?} from {source}: expected the submitted expiration {submitted} \
         floored to the hour, got {key:?}"
    );
    assert_eq!(
        key["deletes"].as_i64(),
        Some(expected_deletes(floor_to_hour(submitted))),
        "case {case:?} from {source}: expected deletes {} (the expiration plus the \
         {RETENTION}-second retention, floored to the hour), got {key:?}",
        expected_deletes(floor_to_hour(submitted))
    );
}

/// The listed key `key_id` of `api_id`, or a panic naming the case.
async fn require_listed(
    router: &smithy_server_runtime::MultiServiceRouter,
    api_id: &str,
    key: &Stored,
) -> Value {
    let listed = listed_key(router, api_id, &key.key_id).await;
    assert!(
        !listed.is_null(),
        "case {:?}: expected the listing to hold the key {}, got nothing",
        key.case,
        key.key_id
    );
    listed
}

/// Every drawn expiration inside the window derives its own deletion time, and the
/// listing reports that one — for a key created with it, and for a key moved onto it by
/// an update.
#[tokio::test]
async fn every_drawn_expiration_is_listed_with_the_deletion_time_it_derives() {
    let seed: u64 = rand::random();
    let mut rng = StdRng::seed_from_u64(seed);
    let cases = drawn_cases(&mut rng);

    let router = router();
    let api_id = create_api(&router, "Api_key_deletes_drawn_expirations").await;

    let mut stored: Vec<Stored> = Vec::new();
    for drawn in &cases {
        let case = format!("seed {seed}: {}", drawn.case);
        let submitted = drawn.instant.submitted(unix_now());
        let answer = create_key(
            &router,
            &api_id,
            json!({ "description": drawn.case, "expires": submitted }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected CreateApiKey 200, got {answer:?}"
        );
        let created = answer.body["apiKey"].clone();
        let key_id = created["id"].as_str().unwrap_or_default().to_string();
        assert!(
            key_id.starts_with(KEY_VERSION_PREFIX),
            "case {case:?}: expected a {KEY_VERSION_PREFIX} key id, got {created:?}"
        );
        assert_projection(&case, "CreateApiKey", submitted, &created);

        // The listing projects the stored key, so it reports the same derivation.
        let listed = listed_key(&router, &api_id, &key_id).await;
        assert_projection(&case, "ListApiKeys", submitted, &listed);
        assert_eq!(
            listed, created,
            "case {case:?}: expected ListApiKeys to report the key CreateApiKey stored, \
             got {listed:?}"
        );
        stored.push(Stored {
            case,
            key_id,
            last: created,
        });
    }

    // A key moved onto a freshly drawn expiration is listed with the deletion time that
    // NEW expiration derives: the derivation follows the expiration, not the create.
    for key in &mut stored {
        let case = format!("{} moved by an update", key.case);
        let validity = rng.random_range(MIN_VALIDITY + BOUND_SLACK..=MAX_VALIDITY - BOUND_SLACK);
        let submitted = unix_now() + validity;
        let answer = update_key(&router, &api_id, &key.key_id, json!({ "expires": submitted })).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected UpdateApiKey 200, got {answer:?}"
        );
        let updated = answer.body["apiKey"].clone();
        assert_projection(&case, "UpdateApiKey", submitted, &updated);
        key.last = updated;
    }

    // The whole listing agrees, key by key, with the last write of each key — and says
    // the same thing when it is read again, because listing is a read.
    for key in &stored {
        let listed = require_listed(&router, &api_id, key).await;
        assert_retention_span(&key.case, "ListApiKeys", &listed);
        assert_eq!(
            listed, key.last,
            "case {:?}: expected the listed key to echo the value its last write \
             reported, got {listed:?}",
            key.case
        );
        let again = require_listed(&router, &api_id, key).await;
        assert_eq!(
            again, listed,
            "case {:?}: expected a repeated listing to report the same key, got {again:?}",
            key.case
        );
    }

    // Each case got a key of its own, so none of the assertions above read a key another
    // case had already moved.
    let mut ids: Vec<&str> = stored.iter().map(|key| key.key_id.as_str()).collect();
    ids.sort_unstable();
    let minted = ids.len();
    ids.dedup();
    assert_eq!(
        ids.len(),
        minted,
        "seed {seed}: expected one distinct key per drawn case, got {ids:?}"
    );
}

/// An expiration OUTSIDE the published window mints no key, so the inventory the
/// deletion time is read off never holds one derived from it.
///
/// The rows sit just outside each published bound and at a drawn instant well past each,
/// so the refusal belongs to the window rather than to one chosen value. A row is never
/// placed within [`BOUND_SLACK`] of a bound: the handler judges the validity against its
/// OWN clock, which it reads after this test's, and a row that close would be a race
/// rather than a case.
#[tokio::test]
async fn an_expiration_outside_the_window_adds_no_key_to_the_inventory() {
    let seed: u64 = rand::random();
    let mut rng = StdRng::seed_from_u64(seed);

    let router = router();
    let api_id = create_api(&router, "Api_key_deletes_drawn_out_of_window").await;

    // One key inside the window, so each refusal below leaves a NON-empty inventory that
    // is asserted to be unchanged.
    let inside = unix_now() + 30 * DAY;
    let answer = create_key(
        &router,
        &api_id,
        json!({ "description": "inside the window", "expires": inside }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case seed {seed}: inside the window: expected CreateApiKey 200, got {answer:?}"
    );
    let kept = answer.body["apiKey"].clone();
    assert_projection(
        &format!("seed {seed}: inside the window"),
        "CreateApiKey",
        inside,
        &kept,
    );

    // (case, validity in seconds past the clock read, outside the window on one side)
    let rows: Vec<(String, i64)> = vec![
        // Below the minimum, the handler's later clock read only makes the validity
        // smaller, so one second short of the bound is a stable case.
        (
            "one second short of the minimum validity".to_string(),
            MIN_VALIDITY - 1,
        ),
        (
            "just past the maximum validity".to_string(),
            MAX_VALIDITY + BOUND_SLACK,
        ),
        {
            let validity = rng.random_range(0..MIN_VALIDITY - BOUND_SLACK);
            (format!("a drawn {validity}-second validity, below the window"), validity)
        },
        {
            let validity =
                rng.random_range(MAX_VALIDITY + BOUND_SLACK..=MAX_VALIDITY + 365 * DAY);
            (format!("a drawn {validity}-second validity, above the window"), validity)
        },
    ];

    for (row, validity) in &rows {
        let case = format!("seed {seed}: {row}");
        let answer = create_key(
            &router,
            &api_id,
            json!({ "description": row, "expires": unix_now() + validity }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected CreateApiKey 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "ApiKeyValidityOutOfBoundsException",
            "case {case:?}: expected error type ApiKeyValidityOutOfBoundsException, \
             got {answer:?}"
        );

        let listing = call(&router, "GET", &format!("/v1/apis/{api_id}/apikeys"), None).await;
        assert_eq!(
            listing.status, 200,
            "case {case:?}: expected ListApiKeys 200, got {listing:?}"
        );
        assert_eq!(
            listing.body["apiKeys"],
            json!([kept]),
            "case {case:?}: expected the inventory to hold only the key inside the \
             window, got {listing:?}"
        );
    }
}
