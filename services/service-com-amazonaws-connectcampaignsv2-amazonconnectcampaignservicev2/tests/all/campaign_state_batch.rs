// SPDX-License-Identifier: CC0-1.0

//! `GetCampaignStateBatch`: the per-id state report, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handler together. The rules under test:
//!
//! * a batch read answers for EVERY id the request carried, in the order it carried
//!   them: an id the request's region holds a campaign under is reported in
//!   `successfulRequests` with the state that campaign is at — the same state the
//!   single-id reader `GetCampaignState` reports for it — and an id it holds none
//!   under is reported in `failedRequests` with the `ResourceNotFound` failure code,
//!   never as an error on the call;
//! * the read WRITES NOTHING: repeating it answers the same, and the single-id reader
//!   agrees afterwards;
//! * every id is resolved in the region the request was signed for, so a campaign of
//!   another region's endpoint is an id this one holds nothing under;
//! * an id outside the id shape's modelled bounds, and a list outside the list's own,
//!   are refused as HTTP 400 `ValidationException` with no per-id report at all.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// One harness per crate: the routing header, the router under test and the
// request/response plumbing live in `tests/all/common.rs`.
use super::common::{call, campaign_state, create_campaign, drive_lifecycle, error_code, router};

/// The `GetCampaignStateBatchFailureCode` a requested id that names no campaign is
/// reported with. Source: the model's own `GetCampaignStateBatchFailureCode` catalog
/// (`smithy_query query="shape" shape="GetCampaignStateBatchFailureCode"`), whose two
/// values are this one and `UnknownError`.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFound";

/// `CampaignId`'s modelled `@length` maximum, and `CampaignIdList`'s own: source
/// `smithy_query query="closure" operation="GetCampaignStateBatch"`, which gives
/// `CampaignId` `@length` 1..256 with `@pattern ^[-:/a-zA-Z0-9]+$`, and
/// `campaignIds` `@length` 1..25.
const CAMPAIGN_ID_MAX_LENGTH: usize = 256;
const MAX_IDS_PER_BATCH: usize = 25;

/// One id of a batch request: either a campaign this suite created (named by its
/// fixture key) or a literal id the store is not expected to hold.
#[derive(Clone, Copy, Debug)]
enum Requested {
    Known(&'static str),
    Unknown(&'static str),
}

/// What a campaign fixture is: the lifecycle actions driven on it, whether its run
/// window has already closed, and the state it is therefore at.
struct Fixture {
    key: &'static str,
    window_closed: bool,
    actions: &'static [&'static str],
    state: &'static str,
}

/// The fixtures every batch row draws its known ids from: one campaign per state of
/// the model's `CampaignState` catalog that a caller can reach through this service's
/// own operations (`Initialized`, `Running`, `Paused`, `Stopped`, and the `Completed`
/// the clock owes a run whose window has closed).
fn fixtures() -> &'static [Fixture] {
    &[
        Fixture {
            key: "initialized",
            window_closed: false,
            actions: &[],
            state: "Initialized",
        },
        Fixture {
            key: "running",
            window_closed: false,
            actions: &["start"],
            state: "Running",
        },
        Fixture {
            key: "paused",
            window_closed: false,
            actions: &["start", "pause"],
            state: "Paused",
        },
        Fixture {
            key: "stopped",
            window_closed: false,
            actions: &["start", "stop"],
            state: "Stopped",
        },
        Fixture {
            key: "completed",
            window_closed: true,
            actions: &["start"],
            state: "Completed",
        },
    ]
}

/// A run window that closed long before the call, as the `Schedule` members carry it
/// on the wire (`@timestampFormat("date-time")`).
fn closed_window() -> Value {
    json!({"startTime": "2020-01-01T00:00:00Z", "endTime": "2020-01-02T00:00:00Z"})
}

/// Create every fixture campaign on `instance` and drive it to its state, returning
/// the minted id of each by fixture key.
async fn create_fixtures(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
) -> BTreeMap<&'static str, String> {
    let mut ids = BTreeMap::new();
    for fixture in fixtures() {
        let schedule = if fixture.window_closed {
            Some(closed_window())
        } else {
            None
        };
        let id = create_campaign(router, region, instance, fixture.key, schedule).await;
        drive_lifecycle(router, region, &id, fixture.actions, fixture.key).await;
        let reported = campaign_state(router, region, &id, fixture.key).await;
        assert_eq!(
            reported, fixture.state,
            "fixture {:?}: expected GetCampaignState to report {:?}, got {reported:?}",
            fixture.key, fixture.state
        );
        ids.insert(fixture.key, id);
    }
    ids
}

/// The state the fixture named `key` is at.
fn fixture_state(key: &str) -> &'static str {
    fixtures()
        .iter()
        .find(|fixture| fixture.key == key)
        .map(|fixture| fixture.state)
        .unwrap_or_else(|| panic!("no fixture named {key:?}"))
}

/// Drive one `GetCampaignStateBatch` for `ids`.
async fn get_state_batch(
    router: &MultiServiceRouter,
    region: &str,
    ids: &[String],
) -> (u16, Value) {
    call(
        router,
        region,
        "POST",
        "/v2/campaigns-state",
        &json!({"campaignIds": ids}),
    )
    .await
}

/// The `(campaignId, state)` pairs a response's `successfulRequests` carries, in the
/// order it carries them.
fn successful(response: &Value) -> Vec<(String, String)> {
    entries(response, "successfulRequests", "state")
}

/// The `(campaignId, failureCode)` pairs a response's `failedRequests` carries, in the
/// order it carries them.
fn failed(response: &Value) -> Vec<(String, String)> {
    entries(response, "failedRequests", "failureCode")
}

/// The `(campaignId, <member>)` pairs of one response list, in order.
fn entries(response: &Value, list: &str, member: &str) -> Vec<(String, String)> {
    response
        .get(list)
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    (
                        row.get("campaignId")
                            .and_then(Value::as_str)
                            .unwrap_or("<no campaignId>")
                            .to_string(),
                        row.get(member)
                            .and_then(Value::as_str)
                            .unwrap_or("<missing>")
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A batch reports the state of every id the request's region holds a campaign under
/// and reports every other id as a per-id `ResourceNotFound` failure, over a TABLE
/// whose rows differ along what the rule generalises over: how many ids the request
/// carries (one, a few, the published maximum), which states those campaigns are at,
/// whether an id is held or not, how known and unknown ids are interleaved, the order
/// they are sent in, and whether an id is repeated.
#[tokio::test]
async fn a_batch_reports_the_state_of_every_id_it_holds_and_fails_the_rest() {
    let region = "us-west-2";
    let router = router();
    let instance = "state-batch-instance";
    let ids = create_fixtures(&router, region, instance).await;

    // 25 ids — `campaignIds`' published maximum — built from every fixture in turn
    // plus the unknown ids between them.
    let mut at_the_maximum: Vec<Requested> = Vec::new();
    while at_the_maximum.len() < MAX_IDS_PER_BATCH {
        for fixture in fixtures() {
            if at_the_maximum.len() == MAX_IDS_PER_BATCH {
                break;
            }
            at_the_maximum.push(Requested::Known(fixture.key));
        }
        if at_the_maximum.len() < MAX_IDS_PER_BATCH {
            at_the_maximum.push(Requested::Unknown("999999999999"));
        }
    }

    let cases: Vec<(&str, Vec<Requested>)> = vec![
        ("one id, held", vec![Requested::Known("running")]),
        (
            "one id, not held",
            vec![Requested::Unknown("999999999999")],
        ),
        (
            "every state the fixtures span",
            fixtures()
                .iter()
                .map(|fixture| Requested::Known(fixture.key))
                .collect(),
        ),
        (
            "the same ids in reverse order",
            fixtures()
                .iter()
                .rev()
                .map(|fixture| Requested::Known(fixture.key))
                .collect(),
        ),
        (
            "held and not held, interleaved",
            vec![
                Requested::Known("initialized"),
                Requested::Unknown("111111111111"),
                Requested::Known("completed"),
                Requested::Unknown("arn:aws:connect-campaigns:us-west-2:000000000000:campaign/x"),
            ],
        ),
        (
            "none of them held",
            vec![
                Requested::Unknown("888888888888"),
                Requested::Unknown("a-b/c:d"),
                Requested::Unknown("ZZZ999"),
            ],
        ),
        (
            "one id repeated, held and not held",
            vec![
                Requested::Known("paused"),
                Requested::Known("paused"),
                Requested::Unknown("999999999999"),
                Requested::Unknown("999999999999"),
            ],
        ),
        ("at the published maximum", at_the_maximum),
    ];

    for (case, requested) in cases {
        let sent: Vec<String> = requested
            .iter()
            .map(|one| match one {
                Requested::Known(key) => ids[key].clone(),
                Requested::Unknown(id) => (*id).to_string(),
            })
            .collect();
        let expected_successful: Vec<(String, String)> = requested
            .iter()
            .filter_map(|one| match one {
                Requested::Known(key) => {
                    Some((ids[key].clone(), fixture_state(key).to_string()))
                }
                Requested::Unknown(_) => None,
            })
            .collect();
        let expected_failed: Vec<(String, String)> = requested
            .iter()
            .filter_map(|one| match one {
                Requested::Known(_) => None,
                Requested::Unknown(id) => {
                    Some(((*id).to_string(), RESOURCE_NOT_FOUND.to_string()))
                }
            })
            .collect();

        let (status, body) = get_state_batch(&router, region, &sent).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            successful(&body),
            expected_successful,
            "case {case:?}: expected successfulRequests {expected_successful:?}, got {:?}",
            successful(&body)
        );
        assert_eq!(
            failed(&body),
            expected_failed,
            "case {case:?}: expected failedRequests {expected_failed:?}, got {:?}",
            failed(&body)
        );

        // The batch agrees with the single-id reader, id for id.
        for (id, state) in &expected_successful {
            let single = campaign_state(&router, region, id, case).await;
            assert_eq!(
                &single, state,
                "case {case:?}: GetCampaignState reported {single:?} for {id}, the batch \
                 reported {state:?}"
            );
        }

        // Reporting states changes nothing a later caller sees: the same request
        // answers the same, and the single-id reader still agrees.
        let (status, again) = get_state_batch(&router, region, &sent).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated read to answer 200, got {status} {again}"
        );
        assert_eq!(
            successful(&again),
            expected_successful,
            "case {case:?}: expected the repeated read to report {expected_successful:?}, got {:?}",
            successful(&again)
        );
        assert_eq!(
            failed(&again),
            expected_failed,
            "case {case:?}: expected the repeated read to fail {expected_failed:?}, got {:?}",
            failed(&again)
        );
    }
}

/// Every id is resolved in the region the request was signed for: a campaign reachable
/// through one region's endpoint is an id the other holds nothing under, so each
/// region reports its own campaign's state and fails the other's id.
#[tokio::test]
async fn a_batch_resolves_every_id_in_the_region_the_request_was_signed_for() {
    let router = router();
    let instance = "state-batch-regional-instance";
    // The same instance name in two regions, so only the region tells the two
    // campaigns apart.
    let east = create_campaign(&router, "us-east-1", instance, "eastern", None).await;
    let west = create_campaign(&router, "eu-west-1", instance, "western", None).await;

    for (region, mine, theirs) in [
        ("us-east-1", &east, &west),
        ("eu-west-1", &west, &east),
    ] {
        let sent = vec![mine.clone(), theirs.clone()];
        let (status, body) = get_state_batch(&router, region, &sent).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            successful(&body),
            vec![(mine.clone(), "Initialized".to_string())],
            "case {region:?}: expected only this region's campaign to be reported, got {:?}",
            successful(&body)
        );
        // Two regions can mint the same id, in which case the "other" id resolves
        // here too — that is one id naming two resources, so only assert the foreign
        // one failed when it is not also this region's own.
        if theirs != mine {
            assert_eq!(
                failed(&body),
                vec![(theirs.clone(), RESOURCE_NOT_FOUND.to_string())],
                "case {region:?}: expected the other region's id to fail, got {:?}",
                failed(&body)
            );
        }
    }
}

/// A request outside the modelled bounds is refused as a bad request, over a TABLE
/// whose rows differ along which bound is reached: the id shape's `@pattern`, the id
/// shape's `@length` at both published ends and one past each, and the list's own
/// `@length` at both ends and one past each. An accepted id names no campaign here, so
/// an accepted row is a 200 whose `failedRequests` carries it.
#[tokio::test]
async fn the_batch_holds_every_id_to_the_id_shape_and_the_list_to_its_length() {
    let region = "ap-southeast-2";
    let router = router();

    let at_max_length = "a".repeat(CAMPAIGN_ID_MAX_LENGTH);
    let past_max_length = "a".repeat(CAMPAIGN_ID_MAX_LENGTH + 1);

    // (case, the ids the request carries, whether the shape admits them)
    let cases: Vec<(String, Vec<String>, bool)> = vec![
        // Admitted by `@pattern ^[-:/a-zA-Z0-9]+$`: ASCII letters, digits, `-`, `:`
        // and `/`.
        ("digits".to_string(), vec!["000000000000".to_string()], true),
        (
            "letters, digits and every punctuation character the class admits"
                .to_string(),
            vec!["Campaign-1:a/b".to_string()],
            true,
        ),
        (
            "one character, the length minimum".to_string(),
            vec!["a".to_string()],
            true,
        ),
        (
            "at the length maximum".to_string(),
            vec![at_max_length.clone()],
            true,
        ),
        // Refused.
        (
            "one past the length maximum".to_string(),
            vec![past_max_length],
            false,
        ),
        ("the empty id".to_string(), vec![String::new()], false),
        (
            "a space in the id".to_string(),
            vec!["has space".to_string()],
            false,
        ),
        (
            "an underscore, which the class does not admit".to_string(),
            vec!["has_underscore".to_string()],
            false,
        ),
        (
            "a dot, which the class does not admit".to_string(),
            vec!["has.dot".to_string()],
            false,
        ),
        (
            "a non-ASCII letter".to_string(),
            vec!["héllo".to_string()],
            false,
        ),
        (
            "one bad id among good ones".to_string(),
            vec![
                "000000000000".to_string(),
                "bang!".to_string(),
                "111111111111".to_string(),
            ],
            false,
        ),
        // The list's own `@length` 1..25.
        (
            "the list at its minimum".to_string(),
            vec!["000000000001".to_string()],
            true,
        ),
        (
            "the list at its maximum".to_string(),
            (0..MAX_IDS_PER_BATCH)
                .map(|index| format!("{index:012}"))
                .collect(),
            true,
        ),
        ("the empty list".to_string(), Vec::new(), false),
        (
            "one past the list maximum".to_string(),
            (0..=MAX_IDS_PER_BATCH)
                .map(|index| format!("{index:012}"))
                .collect(),
            false,
        ),
    ];

    for (case, sent, admitted) in cases {
        let (status, body) = get_state_batch(&router, region, &sent).await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body}"
            );
            // Nothing was created in this region, so every admitted id is reported as
            // a per-id failure rather than as an error on the call.
            let expected: Vec<(String, String)> = sent
                .iter()
                .map(|id| (id.clone(), RESOURCE_NOT_FOUND.to_string()))
                .collect();
            assert_eq!(
                failed(&body),
                expected,
                "case {case:?}: expected every id to be reported not found, got {:?}",
                failed(&body)
            );
            assert!(
                successful(&body).is_empty(),
                "case {case:?}: expected no successfulRequests, got {:?}",
                successful(&body)
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case:?}: expected ValidationException, got {} {body}",
                error_code(&body)
            );
            assert!(
                successful(&body).is_empty() && failed(&body).is_empty(),
                "case {case:?}: expected a refusal to report no ids, got {:?} / {:?}",
                successful(&body),
                failed(&body)
            );
        }
    }
}
