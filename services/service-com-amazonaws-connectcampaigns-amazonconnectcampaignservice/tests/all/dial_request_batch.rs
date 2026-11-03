// SPDX-License-Identifier: CC0-1.0

//! The dial plane of AmazonConnectCampaignService — `PutDialRequestBatch` — end to end
//! over HTTP, plus the state-conflict refusal the whole campaign plane shares.
//!
//! The rules under test, each over a table whose rows differ along what the rule
//! generalises over rather than along the one request that exposed the defect:
//!
//! * A `date-time` timestamp member travels as an RFC3339 STRING. `DialRequest`'s
//!   `expirationTime` targets `TimeStamp`, which carries `@timestampFormat("date-time")`
//!   (`smithy_query(query="shape", shape="DialRequest,TimeStamp")`), so the string an AWS
//!   SDK serializes a datetime to for that member is read as the instant it names —
//!   whatever its sub-second precision and whichever field of the instant it steps — and
//!   never as an epoch number.
//! * A campaign takes dial requests only while it is `Running`. A campaign in any other
//!   run state is refused HTTP 409 `ConflictException` with the sentence this service
//!   words every state conflict with, and an id naming no stored campaign is refused HTTP
//!   404 `ResourceNotFoundException` in that operation class's own words.
//! * The operation is idempotent per `clientToken`: a token the campaign's buffer already
//!   holds is answered with the `DialRequestId` it was given the first time and stores
//!   nothing, whether the repeat arrives in a later batch or twice inside one.
//! * Each request of an accepted batch is reported ON ITS OWN — a request this campaign
//!   cannot dial, because its destination is outside the shape the model declares for it
//!   or its `expirationTime` has already passed, goes to `failedRequests` with a code from
//!   the model's own catalog while the rest of the batch is taken. Only a fault in the
//!   BATCH itself — `dialRequests` outside its modelled `@length` — refuses the call.

use std::collections::BTreeMap;

use aws_smithy_schema::traits::TimestampFormat;
use serde_json::{json, Value};
use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::ops::types::{
    DialRequest, FAILURE_CODE_VALUES,
};
use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::state::{
    service_state, StoredDialRequest,
};

use super::common::{call, router, Answer, Harness};

/// The region every case here signs for.
const REGION: &str = "us-east-1";

/// The Connect instance every campaign below is created for.
const INSTANCE: &str = "dial-instance";

/// The failure code a request that can never be dialled is reported with. One of the
/// three values `FailureCode` carries (`smithy_query(query="shape", shape="FailureCode")`:
/// `InvalidInput | RequestThrottled | UnknownError`), written out here so this suite pins
/// the wire value rather than reading it back out of the implementation.
const INVALID_INPUT: &str = "InvalidInput";

/// The sentence this service answers a campaign-state conflict with, as fixed text around
/// the id the request carried and the state the campaign is in. Written out here, not read
/// back out of the implementation, so this suite pins the wording; `{id}` and `{state}`
/// are where the two variable parts go (see [`worded`]).
const STATE_CONFLICT_WORDING: &str = "Campaign: {id} is in invalid state: {state}";

/// The sentence the operations addressed to a campaign's RUN STATE and its dial plane
/// answer an id that resolves to nothing with. Written out for the same reason.
const RUN_STATE_NOT_FOUND_WORDING: &str = "Campaign: {id} doesn't exist";

/// `wording` with the campaign id and run state it names filled in.
fn worded(wording: &str, id: &str, state: &str) -> String {
    wording.replace("{id}", id).replace("{state}", state)
}

/// A minimal `CreateCampaign` body: every member the model marks required, and nothing
/// else.
fn create_body(name: &str) -> Value {
    json!({
        "name": name,
        "connectInstanceId": INSTANCE,
        "dialerConfig": {"agentlessDialerConfig": {}},
        "outboundCallConfig": {"connectContactFlowId": "flow-1"},
    })
}

/// One dial request body: the four members `DialRequest` marks required.
fn dial_request(client_token: &str, phone_number: &str, expiration_time: &str) -> Value {
    json!({
        "clientToken": client_token,
        "phoneNumber": phone_number,
        "expirationTime": expiration_time,
        "attributes": {"campaign": "nightly"},
    })
}

/// Create a campaign for [`INSTANCE`] and drive it into `target` through the lifecycle
/// operations, returning its id. Only the paths the state machine allows are used.
async fn a_campaign_in(router: &Harness, name: &str, target: &str) -> String {
    router.register_connect_instance(REGION, INSTANCE);
    let created = call(router, REGION, "PUT", "/campaigns", &create_body(name)).await;
    assert_eq!(
        created.status, 200,
        "case \"create the {target} campaign\": expected HTTP 200, got {} {}",
        created.status, created.body
    );
    let id = created.body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"create the {target} campaign\": no id in {}", created.body))
        .to_string();

    let path: &[&str] = match target {
        "Initialized" => &[],
        "Running" => &["start"],
        "Paused" => &["start", "pause"],
        "Stopped" => &["start", "stop"],
        other => panic!("no lifecycle path to {other}"),
    };
    for action in path {
        let moved = call(
            router,
            REGION,
            "POST",
            &format!("/campaigns/{id}/{action}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            moved.status, 200,
            "case \"drive to {target}\": expected {action} to answer HTTP 200, got {} {}",
            moved.status, moved.body
        );
    }
    let state = call(
        router,
        REGION,
        "GET",
        &format!("/campaigns/{id}/state"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        state.body["state"].as_str(),
        Some(target),
        "case \"drive to {target}\": the campaign ended in {}",
        state.body
    );
    id
}

/// Submit `dial_requests` for campaign `id`.
async fn put_batch(router: &Harness, id: &str, dial_requests: Vec<Value>) -> Answer {
    call(
        router,
        REGION,
        "PUT",
        &format!("/campaigns/{id}/dial-requests"),
        &json!({"dialRequests": dial_requests}),
    )
    .await
}

/// The dial requests campaign `id` has in its buffer, keyed by the `clientToken` they were
/// submitted under.
///
/// Read out of the store through the engine the harness was built over: no modelled
/// operation of this service reports a pending dial request, so this is the only way to
/// see what the handler persisted.
fn buffered(router: &Harness, id: &str) -> BTreeMap<String, StoredDialRequest> {
    let state =
        service_state(router.engine()).expect("the shared engine carries this service's store");
    let store = state.lock();
    store
        .campaigns
        .get(id)
        .map(|campaign| campaign.dial_requests.clone())
        .unwrap_or_default()
}

/// The `(clientToken, id)` pairs one answer reports as successful, in the order it reports
/// them.
fn successful(answer: &Answer) -> Vec<(String, String)> {
    answer.body["successfulRequests"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|entry| {
            (
                entry["clientToken"].as_str().unwrap_or_default().to_string(),
                entry["id"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `(clientToken, failureCode)` pairs one answer reports as failed.
fn failed(answer: &Answer) -> Vec<(String, String)> {
    answer.body["failedRequests"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|entry| {
            (
                entry["clientToken"].as_str().unwrap_or_default().to_string(),
                entry["failureCode"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// RFC3339 `expirationTime` values a client can write a future instant with, as
/// `(case, the wire value, seconds after the first row's instant, sub-second
/// nanoseconds)`.
///
/// The rows differ along the two things the form varies in: sub-second precision (none,
/// milli, micro, nano) and which field of the instant a step lands in (second, minute,
/// hour, day, month, year) — so a value read in the wrong unit, or as an epoch number,
/// cannot pass. The first row is the base every other row's offset is measured from, so
/// the table pins the parse without naming an epoch number of its own. Every row is
/// written in UTC: the vendored RFC-3339 parser accepts the `Z` designator only
/// (`aws_smithy_types::DateTime::from_str` answers "Smithy does not support timezone
/// offsets in RFC-3339 date times"), which is also the form every AWS SDK writes.
fn date_time_forms() -> Vec<(&'static str, &'static str, i64, u32)> {
    vec![
        ("whole seconds in UTC", "2099-01-01T00:00:00Z", 0, 0),
        ("milliseconds", "2099-01-01T00:00:00.123Z", 0, 123_000_000),
        ("microseconds", "2099-01-01T00:00:00.123456Z", 0, 123_456_000),
        (
            "nanoseconds",
            "2099-01-01T00:00:00.123456789Z",
            0,
            123_456_789,
        ),
        ("one second later", "2099-01-01T00:00:01Z", 1, 0),
        ("one minute later", "2099-01-01T00:01:00Z", 60, 0),
        ("one hour later", "2099-01-01T01:00:00Z", 3_600, 0),
        ("the next day", "2099-01-02T00:00:00Z", 86_400, 0),
        ("the next month", "2099-02-01T00:00:00Z", 31 * 86_400, 0),
        // 2099 is not a leap year, so the year that follows this base is 365 days long.
        ("the next year", "2100-01-01T00:00:00Z", 365 * 86_400, 0),
    ]
}

#[tokio::test]
async fn a_date_time_expiration_travels_as_the_rfc3339_string_the_model_declares() {
    let router = router();
    let id = a_campaign_in(&router, "dialing", "Running").await;

    // The one thing the restJson1 codec reads to decide a timestamp's wire form is the
    // member schema's own format, so the schema is pinned beside the wire behaviour.
    let format = DialRequest::M_EXPIRATION_TIME
        .timestamp_format()
        .map(|carried| carried.format());
    assert!(
        matches!(format, Some(TimestampFormat::DateTime)),
        "case \"DialRequest$expirationTime's schema\": expected the date-time format the \
         model declares, got {format:?}"
    );

    let mut base_secs: Option<i64> = None;
    for (index, (case, wire, delta_secs, nanos)) in date_time_forms().into_iter().enumerate() {
        // Built from the row's position, not its prose, so the token stays inside
        // `ClientToken`'s own pattern whatever a row is called.
        let token = format!("tok-{index}");
        let accepted = put_batch(
            &router,
            &id,
            vec![dial_request(&token, "+15555550100", wire)],
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected HTTP 200 for the date-time string {wire:?}, got {} {}",
            accepted.status, accepted.body
        );
        let taken = successful(&accepted);
        assert_eq!(
            taken.len(),
            1,
            "case {case:?}: expected the one request to be taken, got {taken:?} and failures \
             {:?}",
            failed(&accepted)
        );
        assert_eq!(
            taken[0].0, token,
            "case {case:?}: expected the submitted clientToken back, got {:?}",
            taken[0]
        );
        assert!(
            !taken[0].1.is_empty(),
            "case {case:?}: expected a minted DialRequestId, got {:?}",
            taken[0]
        );

        let stored = buffered(&router, &id);
        let request = stored
            .get(&token)
            .unwrap_or_else(|| panic!("case {case:?}: nothing buffered under {token:?}"));
        let expires = request.expires_at;
        let base = *base_secs.get_or_insert(expires.secs());
        assert_eq!(
            (expires.secs(), expires.subsec_nanos()),
            (base + delta_secs, nanos),
            "case {case:?}: expected {wire:?} to be read as the instant it names, got \
             ({}, {})",
            expires.secs(),
            expires.subsec_nanos()
        );
    }
}

/// Every run state a campaign can be in that takes NO dial request, and the upper-case
/// spelling the refusal names it with. `CampaignState` is
/// `Initialized | Running | Paused | Stopped | Failed`
/// (`smithy_query(query="shape", shape="CampaignState")`); `Running` is the one state that
/// does take requests, and no operation of this service puts a campaign in `Failed`, so
/// there is no path to it to drive.
fn undialable_states() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Initialized", "INITIALIZED"),
        ("Paused", "PAUSED"),
        ("Stopped", "STOPPED"),
    ]
}

#[tokio::test]
async fn a_campaign_that_is_not_running_refuses_the_batch_as_the_state_conflict_it_is() {
    let router = router();

    for (state, upper) in undialable_states() {
        let case = format!("a {state} campaign");
        let id = a_campaign_in(&router, &format!("not-{state}"), state).await;
        let refused = put_batch(
            &router,
            &id,
            vec![dial_request("tok-1", "+15555550100", "2099-01-01T00:00:00Z")],
        )
        .await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected HTTP 409 for a campaign that is not Running, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ConflictException"),
            "case {case:?}: expected ConflictException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            worded(STATE_CONFLICT_WORDING, &id, upper),
            "case {case:?}: expected the state-conflict sentence around the id and the \
             upper-case state, got {}",
            refused.body
        );
        assert!(
            buffered(&router, &id).is_empty(),
            "case {case:?}: a refused batch buffers nothing, got {:?}",
            buffered(&router, &id).keys().collect::<Vec<_>>()
        );
    }
}

/// Campaign ids no campaign carries, spanning `CampaignId` (len 0..256, pattern `^[\S]*$`,
/// from `smithy_query(query="shape", shape="CampaignId")`): the uuid form, the shortest id
/// a path label can carry, punctuation the pattern admits, the longest id the bound
/// allows, and the 32-hex form this emulator mints with one digit changed.
fn unknown_ids() -> Vec<(&'static str, String)> {
    vec![
        (
            "the uuid form",
            "3f1c90ab-7d44-4e52-8a16-90dc5b2e4711".to_string(),
        ),
        ("one character", "z".to_string()),
        ("punctuation the pattern admits", "a:b-D9._e".to_string()),
        ("256 characters (the length maximum)", "q".repeat(256)),
        (
            "the minted id form",
            "fedcba9876543210fedcba9876543210".to_string(),
        ),
    ]
}

#[tokio::test]
async fn an_id_naming_no_campaign_is_refused_before_the_state_gate() {
    let router = router();
    // A campaign that IS dialable, so the refusals below are the refusal under test and
    // not the operation being broken.
    let control = a_campaign_in(&router, "control", "Running").await;

    let mut rows = unknown_ids();
    // A campaign the store HELD and has since dropped is as absent as one it never had.
    let deleted = a_campaign_in(&router, "doomed", "Running").await;
    let dropped = call(
        &router,
        REGION,
        "DELETE",
        &format!("/campaigns/{deleted}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        dropped.status, 200,
        "case \"delete the campaign\": expected HTTP 200, got {} {}",
        dropped.status, dropped.body
    );
    rows.push(("a campaign the store has dropped", deleted));

    for (case, id) in rows {
        let refused = put_batch(
            &router,
            &id,
            vec![dial_request("tok-1", "+15555550100", "2099-01-01T00:00:00Z")],
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404 for an id no campaign carries, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            worded(RUN_STATE_NOT_FOUND_WORDING, &id, ""),
            "case {case:?}: expected this operation class's own not-found wording around \
             the id the request carried, got {}",
            refused.body
        );
    }

    // None of those refusals touched the control campaign's buffer.
    assert!(
        buffered(&router, &control).is_empty(),
        "case \"the control campaign afterwards\": expected an untouched buffer, got {:?}",
        buffered(&router, &control).keys().collect::<Vec<_>>()
    );
}

/// Batches whose `clientToken`s repeat, as `(case, the batches to submit in order, the
/// tokens the campaign must end up holding)`.
///
/// The rows differ along WHERE the repeat is: the same batch sent twice, a later batch
/// re-sending one token of an earlier one, the same token twice inside one batch, and a
/// batch whose other members differ while the token does not.
#[allow(clippy::type_complexity)]
fn repeated_token_batches() -> Vec<(&'static str, Vec<Vec<Value>>, Vec<&'static str>)> {
    let expires = "2099-06-01T12:00:00Z";
    vec![
        (
            "the same batch sent twice",
            vec![
                vec![dial_request("tok-a", "+15555550101", expires)],
                vec![dial_request("tok-a", "+15555550101", expires)],
            ],
            vec!["tok-a"],
        ),
        (
            "a later batch re-sending one token of an earlier one",
            vec![
                vec![
                    dial_request("tok-b", "+15555550102", expires),
                    dial_request("tok-c", "+15555550103", expires),
                ],
                vec![
                    dial_request("tok-c", "+15555550103", expires),
                    dial_request("tok-d", "+15555550104", expires),
                ],
            ],
            vec!["tok-b", "tok-c", "tok-d"],
        ),
        (
            "the same token twice inside one batch",
            vec![vec![
                dial_request("tok-e", "+15555550105", expires),
                dial_request("tok-e", "+15555550105", expires),
            ]],
            vec!["tok-e"],
        ),
        (
            "a repeat whose other members differ",
            vec![
                vec![dial_request("tok-f", "+15555550106", expires)],
                vec![dial_request("tok-f", "+15555550107", "2099-07-01T12:00:00Z")],
            ],
            vec!["tok-f"],
        ),
    ]
}

#[tokio::test]
async fn a_client_token_is_answered_with_the_id_it_was_first_given() {
    let router = router();

    for (case, batches, expected_tokens) in repeated_token_batches() {
        // A campaign per row, so one row's buffer cannot answer another's token.
        let id = a_campaign_in(&router, case, "Running").await;
        // The id and the destination each token was given the FIRST time it was seen.
        let mut first_ids: BTreeMap<String, String> = BTreeMap::new();
        let mut first_numbers: BTreeMap<String, String> = BTreeMap::new();
        for (index, batch) in batches.iter().enumerate() {
            let sent = batch.len();
            for request in batch {
                let token = request["clientToken"].as_str().unwrap_or_default().to_string();
                let number = request["phoneNumber"].as_str().unwrap_or_default().to_string();
                first_numbers.entry(token).or_insert(number);
            }
            let answer = put_batch(&router, &id, batch.clone()).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} batch {index}: expected HTTP 200, got {} {}",
                answer.status, answer.body
            );
            let taken = successful(&answer);
            assert_eq!(
                taken.len(),
                sent,
                "case {case:?} batch {index}: expected every request of the batch reported \
                 as taken, got {taken:?} and failures {:?}",
                failed(&answer)
            );
            for (token, dial_id) in taken {
                let first = first_ids.entry(token.clone()).or_insert(dial_id.clone());
                assert_eq!(
                    &dial_id, first,
                    "case {case:?} batch {index}: expected {token:?} to keep the id it was \
                     first given ({first:?}), got {dial_id:?}"
                );
            }
        }

        let stored = buffered(&router, &id);
        let held: Vec<&str> = stored.keys().map(String::as_str).collect();
        assert_eq!(
            held, expected_tokens,
            "case {case:?}: expected one buffered request per distinct clientToken, got \
             {held:?}"
        );
        for token in expected_tokens {
            let minted = first_ids
                .get(token)
                .unwrap_or_else(|| panic!("case {case:?}: {token:?} was never reported taken"));
            let request = stored
                .get(token)
                .unwrap_or_else(|| panic!("case {case:?}: nothing buffered under {token:?}"));
            assert_eq!(
                request.id.as_str(),
                minted.as_str(),
                "case {case:?}: expected the buffer to hold the id reported for {token:?}, \
                 got {:?}",
                request.id
            );
            // A repeat stores nothing, so the buffered record is still the one the FIRST
            // submission under this token asked to dial.
            let first_number = first_numbers
                .get(token)
                .map(String::as_str)
                .unwrap_or_default();
            assert_eq!(
                (
                    request.phone_number.as_str(),
                    request.attributes.get("campaign").map(String::as_str)
                ),
                (first_number, Some("nightly")),
                "case {case:?}: expected the buffer to keep what {token:?} first asked to \
                 dial, got {:?} / {:?}",
                request.phone_number,
                request.attributes
            );
        }
    }
}

#[tokio::test]
async fn a_request_that_can_never_be_dialled_is_reported_on_its_own() {
    let router = router();
    let id = a_campaign_in(&router, "mixed", "Running").await;

    // One row per instant already behind the clock when the batch is judged, differing in
    // how far behind it is: the epoch itself, a date long past, and the last instant
    // before the clock's own second.
    let past: Vec<(&str, &str)> = vec![
        ("the epoch", "1970-01-01T00:00:00Z"),
        ("a date long past", "2000-01-01T00:00:00Z"),
        ("one second into the epoch", "1970-01-01T00:00:01.500Z"),
    ];

    for (index, (case, expires)) in past.into_iter().enumerate() {
        let answer = put_batch(
            &router,
            &id,
            vec![
                dial_request("tok-expired", "+15555550108", expires),
                dial_request(
                    &format!("tok-live-{index}"),
                    "+15555550109",
                    "2099-01-01T00:00:00Z",
                ),
            ],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 — an undialable request is a per-request \
             outcome, not a refusal of the call — got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            failed(&answer),
            vec![("tok-expired".to_string(), INVALID_INPUT.to_string())],
            "case {case:?}: expected the expired request reported as failed with the \
             model's own code, got {:?}",
            answer.body
        );
        assert_eq!(
            successful(&answer).len(),
            1,
            "case {case:?}: expected the live request of the batch still taken, got {:?}",
            answer.body
        );
        assert!(
            !buffered(&router, &id).contains_key("tok-expired"),
            "case {case:?}: an expired request occupies no buffer slot, got {:?}",
            buffered(&router, &id).keys().collect::<Vec<_>>()
        );
    }

    assert!(
        FAILURE_CODE_VALUES.contains(&INVALID_INPUT),
        "case \"the failure code reported\": expected {INVALID_INPUT:?} to be one of the \
         model's own FailureCode values, got {FAILURE_CODE_VALUES:?}"
    );
}

/// `phoneNumber` values outside `DestinationPhoneNumber` (len 0..20, pattern
/// `^[\d\-+]*$`, from `smithy_query(query="shape", shape="DestinationPhoneNumber")`), as
/// `(case, the value)`: a letter the pattern does not admit, a space, parentheses a caller
/// might write a number with, a name that is no number at all, and characters past the
/// length maximum.
fn undialable_phone_numbers() -> Vec<(&'static str, String)> {
    vec![
        (
            "a letter the pattern does not admit",
            "+1555555010A".to_string(),
        ),
        ("a space inside the number", "+1 5555550100".to_string()),
        (
            "parentheses around an area code",
            "+1(555)5550100".to_string(),
        ),
        ("no number at all", "not-a-phone-number".to_string()),
        (
            "22 characters (two past the maximum)",
            "+".to_string() + &"1".repeat(21),
        ),
    ]
}

#[tokio::test]
async fn a_destination_outside_its_modelled_shape_is_reported_on_its_own() {
    let router = router();
    let id = a_campaign_in(&router, "constrained", "Running").await;

    for (index, (case, phone_number)) in undialable_phone_numbers().into_iter().enumerate() {
        // One dialable request beside the undialable one, so the row also shows the rest
        // of the batch is served. The token is built from the row's position, not its
        // prose, so it stays inside `ClientToken`'s own pattern.
        let live = format!("tok-live-{index}");
        let answer = put_batch(
            &router,
            &id,
            vec![
                dial_request("tok-undialable", &phone_number, "2099-01-01T00:00:00Z"),
                dial_request(&live, "+15555550100", "2099-01-01T00:00:00Z"),
            ],
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 — a destination the campaign cannot dial is \
             a per-request outcome, not a refusal of the call — got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            failed(&answer),
            vec![("tok-undialable".to_string(), INVALID_INPUT.to_string())],
            "case {case:?}: expected the undialable request reported as failed with the \
             model's own code, got {}",
            answer.body
        );
        assert_eq!(
            successful(&answer)
                .iter()
                .map(|(token, _)| token.clone())
                .collect::<Vec<String>>(),
            vec![live.clone()],
            "case {case:?}: expected the dialable request of the batch still taken, got {}",
            answer.body
        );
        let stored = buffered(&router, &id);
        assert!(
            !stored.contains_key("tok-undialable"),
            "case {case:?}: an undialable request occupies no buffer slot, got {:?}",
            stored.keys().collect::<Vec<_>>()
        );
        assert!(
            !answer.body.to_string().contains(&phone_number),
            "case {case:?}: phoneNumber is @sensitive, so the answer must not echo it, \
             got {}",
            answer.body
        );
    }
}
