// SPDX-License-Identifier: CC0-1.0

//! What one resource's configuration history answers, end to end over the wire.
//!
//! The rule pinned here: `GetResourceConfigHistory` reports the configuration items this
//! account's inventory holds for ONE resource — "each state of the resource during the
//! specified time interval" — bounded to the requested `[earlierTime, laterTime]`
//! window, whose later end defaults to now, newest first by default and oldest first
//! when the request asks for `Forward`, one page of at most
//! `limit` items (ten when the request asks for none, a hundred at most) followed by a
//! `nextToken` that resumes AFTER the last item the page reported — present exactly
//! while items remain. It refuses a resource key the inventory holds no configuration
//! item for with `ResourceNotDiscoveredException` worded as Config words it — the one
//! thing it refuses about, whatever recorders the account holds, since the inventory
//! outlives the recorder that wrote it — a window whose earlier end is not
//! chronologically before its later end with `InvalidTimeRangeException`, and a token it
//! did not issue with `InvalidNextTokenException`. Being a report, it writes nothing: a
//! second read answers what the first one did.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — reusing the
//! `Emulator` harness `config_setup_and_rule_loop` defines. The resource's states are
//! seeded through the store's own write path (`record_discovered_resource`, which the
//! recording pass uses) the way this crate's other inventory suites seed one, because
//! the rows pin members of a configuration item that no write operation takes — the
//! account, the Region and the capture instant a recording pass captures.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, DiscoveredResourceEntry, SERVICE_LINKED_PRINCIPALS,
};

use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

/// The operation under test.
const HISTORY: &str = "GetResourceConfigHistory";

/// Resource types the rows name. Entries of
/// `com.amazonaws.configservice#ResourceType`, from `smithy_query(query="shape",
/// shape="ResourceType")`, deliberately including types no recording in the issue used.
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const DYNAMODB_TABLE: &str = "AWS::DynamoDB::Table";
/// See [`EC2_VOLUME`].
const SQS_QUEUE: &str = "AWS::SQS::Queue";

/// The resource whose history every table reads.
const RESOURCE_ID: &str = "vol-00000000000000001";

/// How many configuration items a page carries when the request asks for no limit: "By
/// default, Config returns a limit of 10 configuration items per page"
/// (`GetResourceConfigHistoryRequest$limit`).
const DEFAULT_PAGE_SIZE: usize = 10;

/// The largest page the operation serves: "You cannot specify a number greater than
/// 100" (`GetResourceConfigHistoryRequest$limit`, whose modelled range is 0..100).
const MAX_PAGE_SIZE: i64 = 100;

/// The instant the FIRST state of the seeded resource was captured: a fixed point in
/// the past, so every state is captured before the call that reads them.
const FIRST_CAPTURE_SECS: u64 = 1_700_000_000;

/// How far apart two successive states of the seeded resource were captured.
const CAPTURE_STEP_SECS: u64 = 3_600;

/// The instant the state `index` of the seeded resource was captured.
fn capture_instant(index: usize) -> SystemTime {
    SystemTime::UNIX_EPOCH
        + Duration::from_secs(FIRST_CAPTURE_SECS + CAPTURE_STEP_SECS * index as u64)
}

/// The same instant as the epoch-seconds timestamp the wire carries.
fn capture_seconds(index: usize) -> f64 {
    (FIRST_CAPTURE_SECS + CAPTURE_STEP_SECS * index as u64) as f64
}

/// Record `states` successive states of one resource through the write path a recording
/// pass uses, each one captured a step after the one before it, and answer their capture
/// instants oldest first.
///
/// Each state records a DIFFERENT configuration document, which is what makes the
/// inventory keep the one it replaces as a state the resource LEFT rather than
/// overwriting it.
fn record_states(
    emulator: &Emulator,
    resource_type: &str,
    resource_id: &str,
    states: usize,
) -> Vec<f64> {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let mut store = state.lock();
    for index in 0..states {
        store.record_discovered_resource(DiscoveredResourceEntry {
            resource_type: resource_type.to_string(),
            resource_id: resource_id.to_string(),
            resource_name: Some(resource_id.to_string()),
            schema_version_id: Some("1.0".to_string()),
            configuration: Some(json!({ "state": index }).to_string()),
            first_recorded_at: Some(capture_instant(0)),
            last_recorded_at: Some(capture_instant(index)),
            ..DiscoveredResourceEntry::default()
        });
    }
    (0..states).map(capture_seconds).collect()
}

/// An emulator with a customer managed configuration recorder — which a
/// configuration-item read needs — holding `states` recorded states of [`RESOURCE_ID`],
/// plus the capture instants of those states, newest first.
async fn emulator_holding(states: usize) -> (Emulator, Vec<f64>) {
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
        .await;
    let mut captured = record_states(&emulator, EC2_VOLUME, RESOURCE_ID, states);
    captured.reverse();
    (emulator, captured)
}

/// The request body for one history read: the resource key, plus the `extra` members
/// (an order, a window bound, a limit, a token) the case adds.
fn request(resource_type: &str, resource_id: &str, extra: Value) -> Value {
    let mut request = json!({ "resourceType": resource_type, "resourceId": resource_id });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    request
}

/// The instant each configuration item in a response was captured, in the order the
/// response carried them — which is what identifies the state an item records.
fn captured_at(case: &str, body: &Value) -> Vec<f64> {
    body["configurationItems"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected configurationItems, got {body}"))
        .iter()
        .map(|item| {
            item["configurationItemCaptureTime"]
                .as_f64()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected an item capture time, got {item}")
                })
        })
        .collect()
}

/// Call `operation`, require a 400 carrying the error code `code`, and answer the
/// message the service worded the refusal with.
async fn refusal(case: &str, emulator: &Emulator, body: &Value, code: &str) -> String {
    let (status, parsed) = emulator.call(HISTORY, body).await;
    let actual = parsed["__type"]
        .as_str()
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
        .unwrap_or("<none>");
    assert_eq!(
        (status, actual),
        (400, code),
        "case {case:?}: expected HTTP 400 {code}, got {status} {actual:?} in {parsed}"
    );
    parsed["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {parsed}"))
        .to_string()
}

/// How many states the inventory holds for one resource, read the way a handler reaches
/// it: a report must leave this alone.
fn held_states(emulator: &Emulator, resource_type: &str, resource_id: &str) -> usize {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let store = state.lock();
    store
        .discovered_resources
        .get(&(resource_type.to_string(), resource_id.to_string()))
        .map(|entry| entry.history.len() + 1)
        .unwrap_or_default()
}

#[tokio::test]
async fn the_history_reports_the_states_inside_the_window_in_the_order_asked_for() {
    // Four recorded states, so a window can name one end of the resource's history, the
    // other, a span in the middle, the whole of it, or no state at all.
    let (emulator, newest_first) = emulator_holding(4).await;
    let oldest_first: Vec<f64> = newest_first.iter().rev().copied().collect();
    let at = capture_seconds;

    // Rows differ along what the rule generalises over: the order asked for, which ends
    // of the window are given, and how much of the history the window spans.
    let cases: [(&str, Value, Vec<f64>); 10] = [
        ("no order and no window", json!({}), newest_first.clone()),
        (
            "the default order, named explicitly",
            json!({ "chronologicalOrder": "Reverse" }),
            newest_first.clone(),
        ),
        (
            "oldest first",
            json!({ "chronologicalOrder": "Forward" }),
            oldest_first.clone(),
        ),
        (
            "a window spanning the two middle states",
            json!({ "earlierTime": at(1), "laterTime": at(2) }),
            vec![at(2), at(1)],
        ),
        (
            "the same window, oldest first",
            json!({
                "earlierTime": at(1),
                "laterTime": at(2),
                "chronologicalOrder": "Forward",
            }),
            vec![at(1), at(2)],
        ),
        (
            "a window whose ends are the first and the last state, both included",
            json!({ "earlierTime": at(0), "laterTime": at(3) }),
            newest_first.clone(),
        ),
        (
            "only a later end, which bounds the newer states out",
            json!({ "laterTime": at(1) }),
            vec![at(1), at(0)],
        ),
        (
            "only an earlier end, which bounds the older states out",
            json!({ "earlierTime": at(2) }),
            vec![at(3), at(2)],
        ),
        (
            "a window that ends before the first state was captured",
            json!({ "laterTime": at(0) - 1.0 }),
            Vec::new(),
        ),
        (
            "a window that starts after the last state was captured",
            json!({ "earlierTime": at(3) + 1.0 }),
            Vec::new(),
        ),
    ];

    for (case, extra, expected) in cases {
        let body = emulator
            .ok(HISTORY, &request(EC2_VOLUME, RESOURCE_ID, extra))
            .await;
        let reported = captured_at(case, &body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the states captured at {expected:?}, got {reported:?} \
             in {body}"
        );
        // One page holds every state the window named, so nothing is left to resume
        // from.
        assert_eq!(
            body["nextToken"].as_str(),
            None,
            "case {case:?}: a page that reported every state in the window carries no \
             token, got {body}"
        );
    }

    // Each item names the resource it records, in this account and the Region the
    // request was signed for.
    let body = emulator
        .ok(HISTORY, &request(EC2_VOLUME, RESOURCE_ID, json!({})))
        .await;
    let items = body["configurationItems"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        items.len(),
        newest_first.len(),
        "the whole history is what the members below are checked over, got {body}"
    );
    for (position, item) in items.iter().enumerate() {
        // The OLDEST item of the resource is the one that discovered it; every later one
        // records a configuration that has been updated since
        // (`ConfigurationItem$configurationItemStatus`).
        let expected_status = if position + 1 == items.len() {
            "ResourceDiscovered"
        } else {
            "OK"
        };
        assert_eq!(
            (
                item["resourceType"].as_str(),
                item["resourceId"].as_str(),
                item["resourceName"].as_str(),
                item["awsRegion"].as_str(),
                item["accountId"].as_str(),
                item["configurationItemStatus"].as_str(),
            ),
            (
                Some(EC2_VOLUME),
                Some(RESOURCE_ID),
                Some(RESOURCE_ID),
                Some(emulator.region.as_str()),
                Some(emulator_server::emulator_account_id()),
                Some(expected_status),
            ),
            "every item must name the recorded resource, got {item}"
        );
    }
}

#[tokio::test]
async fn a_page_carries_at_most_the_limit_and_the_token_resumes_after_it() {
    // More states than one default page holds, so a request that asks for no limit
    // pages too.
    const STATES: usize = 12;
    let (emulator, newest_first) = emulator_holding(STATES).await;

    // Rows differ along the page size: the smallest page, sizes that divide the history
    // and sizes that do not, the default, exactly as many items as there are, more than
    // there are, and the published maximum.
    let cases: [(&str, i64, usize); 8] = [
        ("one item a page", 1, 1),
        ("two items a page", 2, 2),
        ("five items a page, the last page short", 5, 5),
        ("the default page size, asked for as zero", 0, DEFAULT_PAGE_SIZE),
        ("one item fewer than there are", (STATES - 1) as i64, STATES - 1),
        ("exactly as many items as there are", STATES as i64, STATES),
        ("more items than there are", (STATES + 5) as i64, STATES + 5),
        ("the largest page the operation serves", MAX_PAGE_SIZE, MAX_PAGE_SIZE as usize),
    ];

    for (case, limit, page_size) in cases {
        let mut seen: Vec<f64> = Vec::new();
        let mut token: Option<String> = None;
        // One page per state at worst: a walk that has not finished by then is not
        // paging.
        for _ in 0..=STATES {
            let mut extra = json!({ "limit": limit });
            if let Some(held) = &token {
                extra["nextToken"] = json!(held);
            }
            let body = emulator
                .ok(HISTORY, &request(EC2_VOLUME, RESOURCE_ID, extra))
                .await;
            let page = captured_at(case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page carries at most {page_size} items, got {} in {body}",
                page.len()
            );
            seen.extend(page.iter().copied());
            match body["nextToken"].as_str() {
                // A token comes back exactly while items remain, so the page that
                // handed one out was a FULL page.
                Some(next) => {
                    assert_eq!(
                        page.len(),
                        page_size,
                        "case {case:?}: a page followed by a token is full, got {} items \
                         in {body}",
                        page.len()
                    );
                    token = Some(next.to_string());
                }
                None => {
                    token = None;
                    break;
                }
            }
        }
        assert_eq!(
            token, None,
            "case {case:?}: paging at {page_size} items a page must finish, got {seen:?}"
        );
        assert_eq!(
            seen, newest_first,
            "case {case:?}: the pages together report every state once, newest first, \
             got {seen:?}"
        );
    }

    // A limit outside the range the operation accepts is refused: the modelled range
    // answers for it before the handler's own `InvalidLimitException` can, so either
    // code is the contract.
    for (case, limit) in [
        ("one past the maximum", MAX_PAGE_SIZE + 1),
        ("far past the maximum", MAX_PAGE_SIZE * 10),
        ("a negative page size", -1),
    ] {
        let (status, code) = emulator
            .refused(
                HISTORY,
                &request(EC2_VOLUME, RESOURCE_ID, json!({ "limit": limit })),
            )
            .await;
        assert!(
            status == 400 && ["InvalidLimitException", "ValidationException"].contains(&code.as_str()),
            "case {case:?}: a limit of {limit} is outside 0..{MAX_PAGE_SIZE} and must be \
             refused 400 InvalidLimitException or ValidationException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_resource_key_the_inventory_holds_nothing_for_is_refused_as_config_words_it() {
    let (emulator, _) = emulator_holding(2).await;
    record_states(&emulator, DYNAMODB_TABLE, "orders", 1);

    // Rows differ along WHICH half of the key the inventory holds nothing for: the id,
    // the type, both, and a pair whose id was recorded under another type. The sentence
    // is the same template for all of them, with the request's own id and type in it.
    let cases: [(&str, &str, &str); 5] = [
        ("an id nothing was recorded for", EC2_VOLUME, "vol-00000000000000404"),
        ("a type nothing was recorded for", SQS_QUEUE, "a-queue"),
        (
            "a queue URL the inventory does not hold",
            SQS_QUEUE,
            "https://sqs.us-east-1.amazonaws.com/000000000000/never-created",
        ),
        ("an id recorded under another type", DYNAMODB_TABLE, RESOURCE_ID),
        ("a type recorded under another id", EC2_VOLUME, "orders"),
    ];

    for (case, resource_type, resource_id) in cases {
        let message = refusal(
            case,
            &emulator,
            &request(resource_type, resource_id, json!({})),
            "ResourceNotDiscoveredException",
        )
        .await;
        let expected = format!(
            "Resource {resource_id} of resourceType:{resource_type} is unknown or has \
             not been discovered"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // The resource the inventory DOES hold is still served, so the refusals above are
    // about the key asked for and not about the read.
    let body = emulator
        .ok(HISTORY, &request(EC2_VOLUME, RESOURCE_ID, json!({})))
        .await;
    assert_eq!(
        captured_at("the recorded resource", &body).len(),
        2,
        "the recorded resource's two states are still reported, got {body}"
    );
}

#[tokio::test]
async fn a_history_read_answers_about_the_key_asked_for_whatever_recorders_exist() {
    // Config keeps the configuration items it captured for a resource — deleting a
    // recorder deletes no history — so what this read answers is decided by the KEY the
    // request named and not by the recorders the account holds when it is called.
    //
    // Rows differ along the key asked for, and each row is read in three account states
    // in turn: no configuration recorder at all, a service-linked recorder only (which
    // records for the service it is linked to), and the account's customer managed
    // recorder.
    let cases: [(&str, &str, &str, bool); 3] = [
        ("a resource the inventory holds", EC2_VOLUME, RESOURCE_ID, true),
        (
            "a resource the inventory does not hold",
            EC2_VOLUME,
            "vol-00000000000000404",
            false,
        ),
        ("a type nothing was recorded for", SQS_QUEUE, "a-queue", false),
    ];

    for (case, resource_type, resource_id, held) in cases {
        let emulator = Emulator::new();
        record_states(&emulator, EC2_VOLUME, RESOURCE_ID, 2);
        let body = request(resource_type, resource_id, json!({}));

        for account_state in ["no recorder", "a service-linked recorder", "a customer managed recorder"]
        {
            match account_state {
                "a service-linked recorder" => {
                    emulator
                        .ok(
                            "PutServiceLinkedConfigurationRecorder",
                            &json!({ "ServicePrincipal": SERVICE_LINKED_PRINCIPALS[0] }),
                        )
                        .await;
                }
                "a customer managed recorder" => {
                    emulator
                        .ok(
                            "PutConfigurationRecorder",
                            &recorder_request(json!({ "roleARN": ROLE_ARN })),
                        )
                        .await;
                }
                _ => {}
            }

            let row = format!("{case} with {account_state}");
            if held {
                let reported = emulator.ok(HISTORY, &body).await;
                assert_eq!(
                    captured_at(&row, &reported).len(),
                    2,
                    "case {row:?}: the two recorded states are reported, got {reported}"
                );
            } else {
                let message =
                    refusal(&row, &emulator, &body, "ResourceNotDiscoveredException").await;
                let expected = format!(
                    "Resource {resource_id} of resourceType:{resource_type} is unknown or \
                     has not been discovered"
                );
                assert_eq!(
                    message, expected,
                    "case {row:?}: expected the message {expected:?}, got {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_window_the_wrong_way_round_and_a_token_it_did_not_issue_are_refused() {
    let (emulator, _) = emulator_holding(3).await;
    let at = capture_seconds;

    // A range whose earlier end is not chronologically before its later end names no
    // interval at all; rows differ along how far apart the two ends are.
    for (case, earlier, later) in [
        ("one second the wrong way round", at(1) + 1.0, at(1)),
        ("a whole step the wrong way round", at(2), at(1)),
        ("the newest state against the oldest", at(2), at(0)),
    ] {
        let message = refusal(
            case,
            &emulator,
            &request(
                EC2_VOLUME,
                RESOURCE_ID,
                json!({ "earlierTime": earlier, "laterTime": later }),
            ),
            "InvalidTimeRangeException",
        )
        .await;
        assert!(
            !message.is_empty(),
            "case {case:?}: the refusal must say what is wrong with the range, got \
             {message:?}"
        );
    }

    // The token is opaque: anything this operation did not issue is malformed, whatever
    // it looks like. Rows differ along the shape of the value sent.
    for (case, token) in [
        ("a word", json!("next-please")),
        ("an empty token", json!("")),
        ("a page of another ordering", json!("eyJvZmZzZXQiOiAxfQ")),
        (
            "a token of this operation's with a character changed",
            json!("MTcwMDAwMDAwMDAwMDAwMDAwMA"),
        ),
    ] {
        let message = refusal(
            case,
            &emulator,
            &request(EC2_VOLUME, RESOURCE_ID, json!({ "nextToken": token })),
            "InvalidNextTokenException",
        )
        .await;
        let quotes_the_token = token
            .as_str()
            .is_some_and(|sent| !sent.is_empty() && message.contains(sent));
        assert!(
            !quotes_the_token,
            "case {case:?}: an opaque token is not echoed back, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_history_read_reports_without_writing() {
    let (emulator, newest_first) = emulator_holding(3).await;
    let held = held_states(&emulator, EC2_VOLUME, RESOURCE_ID);

    // Rows differ along the read performed: a whole history, one page of it, a window
    // and an order.
    for (case, extra) in [
        ("the whole history", json!({})),
        ("one page of it", json!({ "limit": 1 })),
        ("a window", json!({ "earlierTime": capture_seconds(1) })),
        ("oldest first", json!({ "chronologicalOrder": "Forward" })),
    ] {
        let body = request(EC2_VOLUME, RESOURCE_ID, extra);
        let first = emulator.ok(HISTORY, &body).await;
        let again = emulator.ok(HISTORY, &body).await;
        assert_eq!(
            again, first,
            "case {case:?}: a second read must report what the first one did, got {again}"
        );
        assert_eq!(
            held_states(&emulator, EC2_VOLUME, RESOURCE_ID),
            held,
            "case {case:?}: a report must leave the resource at {held} recorded states"
        );
    }

    // And the states themselves are untouched: the whole history still reads back.
    let body = emulator
        .ok(HISTORY, &request(EC2_VOLUME, RESOURCE_ID, json!({})))
        .await;
    assert_eq!(
        captured_at("after the reads", &body),
        newest_first,
        "every state must still be reported after the reads, got {body}"
    );
}
