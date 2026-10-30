// SPDX-License-Identifier: CC0-1.0

//! What `ListApiKeys` reports for a key's deletion time, end to end over HTTP: the value
//! the service computed for that key, whatever route the key reached its expiration by.
//!
//! The arithmetic itself — a `da2` key's `deletes` is its `expires` plus the post-expiry
//! retention the service keeps an expired key for, floored to the hour — and the fact
//! that the expiration ALONE determines it are asserted by
//! [`crate::api_key_deletion_window`], whose constants, helpers and rule assertion this
//! suite reuses rather than restating. What this suite adds is the dimension that
//! listing is a PER-KEY projection: an API holding many keys reports, for each of them,
//! the value that key's own last write (`CreateApiKey` or `UpdateApiKey`) returned, and
//! none of a neighbour's.
//!
//! The rows differ along the ROUTE by which a key reached its current expiration — an
//! explicit expiration on or off an hour boundary, no expiration at all, an expiration
//! extended, shortened or walked by several updates, a rename naming no expiration, and
//! each published validity bound — and every row is run under two different APIs, each
//! of which must list its own keys with their own derived values and none of the other's.

use serde_json::{json, Value};

use crate::api_key_deletion_window::{
    assert_retention_span, create_key, floor_to_hour, unix_now, update_key, BOUND_SLACK, DAY,
    DEFAULT_VALIDITY, HOUR,
};
use crate::error_contract::{call, create_api, router};

/// One write a key takes after it is created: a new expiration that many seconds past
/// the submitting clock read, or a write naming no expiration at all.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// `UpdateApiKey` with a new expiration.
    Expiration(i64),
    /// `UpdateApiKey` carrying only a description, which names no new expiration.
    Rename,
}

/// One route to a current expiration: how the key was created, and the writes it took
/// afterwards.
struct Row {
    case: &'static str,
    /// The validity the create submits, in seconds past the clock read, or `None` for a
    /// create naming no expiration at all.
    created: Option<i64>,
    steps: &'static [Step],
}

/// One stored key as the last write that touched it reported it.
struct Written {
    case: String,
    /// Which of the APIs under test the key hangs under.
    api_index: usize,
    key_id: String,
    last: Value,
}

/// The rows: the routes a key's current expiration can be reached by.
const ROWS: &[Row] = &[
    Row {
        case: "an explicit expiration on an hour boundary",
        created: Some(3 * DAY + 2 * HOUR),
        steps: &[],
    },
    Row {
        case: "an explicit expiration off an hour boundary",
        created: Some(10 * DAY + 5 * HOUR + 37),
        steps: &[],
    },
    Row {
        case: "no expiration at all, the documented default",
        created: None,
        steps: &[],
    },
    Row {
        case: "an expiration extended by an update",
        created: Some(2 * DAY),
        steps: &[Step::Expiration(200 * DAY)],
    },
    Row {
        case: "an expiration shortened by an update",
        created: Some(90 * DAY),
        steps: &[Step::Expiration(DAY + BOUND_SLACK)],
    },
    Row {
        case: "an expiration walked by several updates, then a rename",
        created: Some(30 * DAY),
        steps: &[
            Step::Expiration(365 * DAY - BOUND_SLACK),
            Step::Expiration(45 * DAY + 11 * HOUR + 59),
            Step::Rename,
        ],
    },
    Row {
        case: "the shortest validity the service accepts",
        created: Some(DAY + BOUND_SLACK),
        steps: &[],
    },
    Row {
        case: "the longest validity the service accepts",
        created: Some(365 * DAY - BOUND_SLACK),
        steps: &[],
    },
];

/// A listed key carries the deletion time its own expiration derives, which is the one
/// the key's LAST write reported — over every route to an expiration, with the keys of
/// two APIs holding each other's neighbours.
#[tokio::test]
async fn every_listed_key_echoes_the_deletion_time_its_last_write_reported() {
    let router = router();
    let apis = [
        create_api(&router, "Api_key_deletes_routes_one").await,
        create_api(&router, "Api_key_deletes_routes_two").await,
    ];

    let mut written: Vec<Written> = Vec::new();
    for (api_index, api_id) in apis.iter().enumerate() {
        for row in ROWS {
            let case = format!("{} under api {api_index}", row.case);
            let before = unix_now();
            let mut body = json!({ "description": row.case });
            if let Some(validity) = row.created {
                body["expires"] = json!(before + validity);
            }
            let answer = create_key(&router, api_id, body).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected CreateApiKey 200, got {answer:?}"
            );
            let created = answer.body["apiKey"].clone();
            let key_id = created["id"].as_str().unwrap_or_default().to_string();
            let expires = assert_retention_span(&case, "CreateApiKey", &created);
            match row.created {
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

            let mut last = created;
            for (step_index, step) in row.steps.iter().enumerate() {
                let case = format!("{case}, write {step_index} ({step:?})");
                let body = match step {
                    Step::Expiration(validity) => json!({ "expires": unix_now() + validity }),
                    Step::Rename => json!({ "description": format!("{} renamed", row.case) }),
                };
                let answer = update_key(&router, api_id, &key_id, body).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected UpdateApiKey 200, got {answer:?}"
                );
                let updated = answer.body["apiKey"].clone();
                assert_retention_span(&case, "UpdateApiKey", &updated);
                if matches!(step, Step::Rename) {
                    // A write naming no expiration moves neither time member.
                    assert_eq!(
                        updated["expires"], last["expires"],
                        "case {case:?}: expected the stored expiration to stand, got {updated:?}"
                    );
                    assert_eq!(
                        updated["deletes"], last["deletes"],
                        "case {case:?}: expected the stored deletion time to stand, \
                         got {updated:?}"
                    );
                }
                last = updated;
            }

            written.push(Written {
                case,
                api_index,
                key_id,
                last,
            });
        }
    }

    // Once every row's keys are in place, each listing still reports each key's own
    // derivation rather than a neighbour's.
    for (api_index, api_id) in apis.iter().enumerate() {
        let case = format!("the whole listing of api {api_index}");
        let answer = call(&router, "GET", &format!("/v1/apis/{api_id}/apikeys"), None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListApiKeys 200, got {answer:?}"
        );
        let listed = answer.body["apiKeys"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let expected: Vec<&Written> = written
            .iter()
            .filter(|key| key.api_index == api_index)
            .collect();
        assert_eq!(
            listed.len(),
            expected.len(),
            "case {case:?}: expected the {} keys minted under this api, got {listed:?}",
            expected.len()
        );
        for key in expected {
            let found = listed
                .iter()
                .find(|candidate| candidate["id"].as_str() == Some(key.key_id.as_str()))
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected the listing to hold {}, got {listed:?}",
                        key.case, key.key_id
                    )
                });
            assert_retention_span(&key.case, "ListApiKeys", found);
            assert_eq!(
                found, &key.last,
                "case {:?}: expected the listed key to echo the value its last write \
                 reported, got {found:?}",
                key.case
            );
        }

        // And it holds its OWN keys only: a key minted under the other API is not here.
        for other in written.iter().filter(|key| key.api_index != api_index) {
            assert!(
                !listed
                    .iter()
                    .any(|listed| listed["id"].as_str() == Some(other.key_id.as_str())),
                "case {case:?}: expected the key {} of api {} to be absent, got {listed:?}",
                other.key_id,
                other.api_index
            );
        }
    }
}
