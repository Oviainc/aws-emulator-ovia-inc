// SPDX-License-Identifier: CC0-1.0

//! The ONE "a campaign takes outbound requests only while it is Running" rule both
//! batch operations judge a call by, driven end to end over the wire through each of
//! them.
//!
//! `PutOutboundRequestBatch` and `PutProfileOutboundRequestBatch` resolve and judge a
//! campaign through the same entry point on this service's store, so these cases hold
//! both of them to that rule rather than to one copy of it:
//!
//! * a campaign the clock has in any state other than `Running` is refused with HTTP
//!   409 `InvalidCampaignStateException` naming that state, with the same sentence from
//!   either operation, and the campaign's buffer is left empty;
//! * a campaign that IS `Running` at the instant of the call takes the batch, however
//!   it got there;
//! * an id the request's region holds no campaign under is HTTP 404
//!   `ResourceNotFoundException` from either operation — the one part of this the two
//!   deliberately word differently, since the dial batch's recorded refusal names the
//!   campaign asked for.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State, CAMPAIGN_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{
    call, campaign_state, create_campaign_with, drive_lifecycle, error_code, instant, router,
    router_with_engine, Harness,
};

/// One batch operation: the request the model binds it to under `/v2/campaigns/{id}`,
/// the list member it takes its requests in, how one well-formed request of its own
/// shape is built, the sentence it words a not-found refusal with, and the buffer of
/// the campaign it writes to.
#[derive(Debug, Clone, Copy)]
struct Batch {
    operation: &'static str,
    suffix: &'static str,
    member: &'static str,
    request: fn(&str) -> Value,
    not_found_message: fn(&str) -> String,
    pending: fn(&engine::InMemoryEngine, &str, &str) -> usize,
}

/// One telephony dial request: the members `OutboundRequest` requires, with an
/// expiration far enough ahead that the clock cannot pass it mid-case.
fn dial_request(client_token: &str) -> Value {
    json!({
        "clientToken": client_token,
        "expirationTime": instant(3_600.0),
        "channelSubtypeParameters": {
            "telephony": {"destinationPhoneNumber": "+15555550100", "attributes": {}}
        }
    })
}

/// One profile outbound request: `clientToken` and `profileId` are the members
/// `ProfileOutboundRequest` requires, and the `profileId` is within that member's
/// modelled `^[a-f0-9]{32}$` pattern.
fn profile_request(client_token: &str) -> Value {
    json!({
        "clientToken": client_token,
        "profileId": "0123456789abcdef0123456789abcdef",
        "expirationTime": instant(3_600.0)
    })
}

/// The dial requests `campaign` still has pending, read off the store the handlers
/// wrote to.
fn pending_dial(engine: &engine::InMemoryEngine, region: &str, campaign: &str) -> usize {
    engine
        .state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>()
        .lock()
        .campaign(region, campaign)
        .expect("the campaign under test")
        .pending_outbound_requests(SystemTime::now())
}

/// The profile outbound requests `campaign` still has pending, from the same store.
fn pending_profile(engine: &engine::InMemoryEngine, region: &str, campaign: &str) -> usize {
    engine
        .state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>()
        .lock()
        .campaign(region, campaign)
        .expect("the campaign under test")
        .pending_profile_outbound_requests(SystemTime::now())
}

/// Both operations that take a batch of outbound requests into a campaign's buffer —
/// the two whose model declares `InvalidCampaignStateException` beside
/// `ResourceNotFoundException` for a batch.
const BATCH_OPERATIONS: &[Batch] = &[
    Batch {
        operation: "PutOutboundRequestBatch",
        suffix: "/outbound-requests",
        member: "outboundRequests",
        request: dial_request,
        not_found_message: dial_not_found_message,
        pending: pending_dial,
    },
    Batch {
        operation: "PutProfileOutboundRequestBatch",
        suffix: "/profile-outbound-requests",
        member: "profileOutboundRequests",
        request: profile_request,
        not_found_message: profile_not_found_message,
        pending: pending_profile,
    },
];

/// The not-found sentence the dial batch answers with, which names the campaign the
/// caller asked for.
fn dial_not_found_message(id: &str) -> String {
    format!("Campaign: {id} doesn't exist")
}

/// The not-found sentence the profile batch answers with: the fixed text every other
/// campaign-id operation of this service shares.
fn profile_not_found_message(_id: &str) -> String {
    CAMPAIGN_NOT_FOUND_MESSAGE.to_string()
}

/// Submit a one-request batch of `op`'s shape to `id` under `client_token`.
async fn submit(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    op: &Batch,
    client_token: &str,
) -> (u16, Value) {
    let mut body = serde_json::Map::new();
    body.insert(
        op.member.to_string(),
        json!([(op.request)(client_token)]),
    );
    call(
        router,
        region,
        "PUT",
        &format!("/v2/campaigns/{id}{}", op.suffix),
        &Value::Object(body),
    )
    .await
}

/// A state a campaign can be at when a batch arrives, and how a case gets it there:
/// the run window it is created with (`None` for no schedule at all), the lifecycle
/// actions driven over it, and whether a campaign at that state takes the batch.
#[derive(Debug, Clone, Copy)]
struct Row {
    state: &'static str,
    window: Option<(f64, f64)>,
    setup: &'static [&'static str],
    takes_requests: bool,
}

/// Every `CampaignState` value this slice of the API can put a campaign at (source: the
/// model's `CampaignState` enum, as `CAMPAIGN_STATE_VALUES` in this crate's `ops::types`
/// renders it) — including `Running` reached three different ways and `Completed`
/// reached by the CLOCK alone, with no call having written it. `Failed` is the one value
/// no call here reaches.
const ROWS: &[Row] = &[
    Row {
        state: "Initialized",
        window: None,
        setup: &[],
        takes_requests: false,
    },
    Row {
        state: "Running",
        window: None,
        setup: &["start"],
        takes_requests: true,
    },
    Row {
        state: "Paused",
        window: None,
        setup: &["start", "pause"],
        takes_requests: false,
    },
    Row {
        state: "Running",
        window: None,
        setup: &["start", "pause", "resume"],
        takes_requests: true,
    },
    Row {
        state: "Stopped",
        window: None,
        setup: &["start", "stop"],
        takes_requests: false,
    },
    Row {
        state: "Running",
        window: None,
        setup: &["start", "stop", "start"],
        takes_requests: true,
    },
    // Started inside a run window that is still OPEN: the clock has not finished this
    // campaign, so it is dialling.
    Row {
        state: "Running",
        window: Some((-60.0, 7_200.0)),
        setup: &["start"],
        takes_requests: true,
    },
    // Started inside a window that has since CLOSED: the clock, and no call, has this
    // campaign at the terminal `Completed`.
    Row {
        state: "Completed",
        window: Some((-7_200.0, -3_600.0)),
        setup: &["start"],
        takes_requests: false,
    },
];

/// A campaign on `instance`, created with `window` as its schedule when given and put
/// through `setup`. Hands back the minted id.
async fn campaign_at(
    router: &Harness,
    region: &str,
    instance: &str,
    name: &str,
    row: &Row,
    case: &str,
) -> String {
    let extra = match row.window {
        Some((start, end)) => json!({"schedule": {
            "startTime": instant(start),
            "endTime": instant(end)
        }}),
        None => json!({}),
    };
    let id = create_campaign_with(router, region, instance, name, extra).await;
    drive_lifecycle(router, region, &id, row.setup, case).await;
    id
}

/// Both batch operations judge a campaign by the one dialability rule: a campaign the
/// clock has anywhere but `Running` is refused with one status, one code, one reported
/// state and one sentence and buffers nothing, and a campaign that IS `Running` takes
/// the batch however it got there.
#[tokio::test]
async fn both_batch_operations_judge_a_campaign_by_one_rule() {
    let region = "us-west-2";
    let instance = "outbound-batch-one-rule";
    let (router, engine) = router_with_engine();

    for op in BATCH_OPERATIONS {
        for (index, row) in ROWS.iter().enumerate() {
            let case = format!("{} on a {} campaign (row {index})", op.operation, row.state);
            let id = campaign_at(
                &router,
                region,
                instance,
                &format!("one-rule-{}-{index}", op.operation),
                row,
                &case,
            )
            .await;
            let state = campaign_state(&router, region, &id, &case).await;
            assert_eq!(
                state, row.state,
                "case {case:?}: expected the setup to leave the campaign at {:?}, got {state:?}",
                row.state
            );

            let (status, body) = submit(&router, region, &id, op, "token-1").await;
            if row.takes_requests {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a Running campaign takes the batch, got {status} {body}"
                );
                let taken = body
                    .get("successfulRequests")
                    .and_then(Value::as_array)
                    .map(|entries| entries.len())
                    .unwrap_or_default();
                assert_eq!(
                    taken, 1,
                    "case {case:?}: expected the request to be reported as taken, got {body}"
                );
                let buffered = (op.pending)(&engine, region, &id);
                assert_eq!(
                    buffered, 1,
                    "case {case:?}: expected 1 pending request in the campaign's buffer, \
                     got {buffered}"
                );
                continue;
            }

            assert_eq!(
                status, 409,
                "case {case:?}: expected HTTP 409, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("InvalidCampaignStateException"),
                "case {case:?}: expected InvalidCampaignStateException, got {}",
                error_code(&body)
            );
            assert_eq!(
                body["state"].as_str(),
                Some(row.state),
                "case {case:?}: expected the refusal to name state {:?}, got {:?}",
                row.state,
                body["state"]
            );
            let expected_message = format!(
                "Campaign {id} is {} and takes outbound requests only while Running",
                row.state
            );
            assert_eq!(
                body["message"].as_str(),
                Some(expected_message.as_str()),
                "case {case:?}: expected {expected_message:?}, got {:?}",
                body["message"]
            );
            let buffered = (op.pending)(&engine, region, &id);
            assert_eq!(
                buffered, 0,
                "case {case:?}: a refused batch buffers nothing, got {buffered}"
            );
        }
    }
}

/// An id no campaign of the request's region answers to is HTTP 404
/// `ResourceNotFoundException` from either batch operation, over several well-formed
/// ids — so the refusal is "no such campaign" rather than anything about one id's shape,
/// and a campaign of another region is exactly that: absent here. Each operation words
/// that one refusal as its own recorded responses do.
#[tokio::test]
async fn both_batch_operations_refuse_an_unknown_id_each_in_its_own_words() {
    let home = "eu-west-1";
    let away = "ap-south-1";
    let instance = "outbound-batch-one-rule-404";
    let router = router();
    let kept = create_campaign_with(&router, home, instance, "kept", json!({})).await;
    drive_lifecycle(&router, home, &kept, &["start"], "the kept campaign").await;

    // Each row is within `CampaignId`'s pattern `^[-:/a-zA-Z0-9]+$` and its 1..256
    // length bound (source: the operations' `id` member), carries no `/` — the id
    // travels as one path label — and none of them is the id the create above minted.
    let unknown = [
        ("a minted-shaped id", "000000009999".to_string()),
        ("a word", "no-such-campaign".to_string()),
        ("a single character", "x".to_string()),
        ("a colon-shaped id", "campaign:42".to_string()),
        ("the longest id the model takes", "a".repeat(256)),
        // The campaign the home region holds: absent from the region asked here.
        ("a campaign of another region", kept.clone()),
    ];

    for op in BATCH_OPERATIONS {
        for (id_case, id) in &unknown {
            let case = format!("{} on {id_case}", op.operation);
            let region = if id_case == &"a campaign of another region" {
                away
            } else {
                home
            };
            let (status, body) = submit(&router, region, id, op, "token-1").await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {}",
                error_code(&body)
            );
            let expected_message = (op.not_found_message)(id);
            assert_eq!(
                body["message"].as_str(),
                Some(expected_message.as_str()),
                "case {case:?}: expected {expected_message:?}, got {:?}",
                body["message"]
            );
        }
    }
}
