// SPDX-License-Identifier: CC0-1.0

//! `PutProfileOutboundRequestBatch` takes profile outbound requests into a campaign's
//! profile buffer and reports every request of the batch on its own.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handler together. The rules under test:
//!
//! * a batch is taken only by a campaign the store holds (an unknown id is
//!   `ResourceNotFoundException`, HTTP 404) that is `Running` at the instant it is
//!   served (any other state is `InvalidCampaignStateException`, HTTP 409, carrying
//!   that state);
//! * a request whose `expirationTime` the clock has already passed is reported on the
//!   per-request failure channel under the modelled
//!   `ProfileOutboundRequestFailureCode` `InvalidInput` and no request id, while the
//!   requests beside it in the same batch are taken as usual (the batch is still HTTP
//!   200), and a request carrying NO `expirationTime` — the member is optional — never
//!   expires;
//! * the call is idempotent: a `clientToken` whose request is still pending is answered
//!   with the `ProfileOutboundRequestId` it was first given and stores nothing further,
//!   and a request that has passed its expiration releases both its buffer slot and its
//!   token.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router, router_with_engine};

/// Now, as seconds since the Unix epoch.
fn unix_secs() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("a test clock after the Unix epoch")
        .as_secs_f64()
}

/// `now + offset_secs` as the RFC 3339 instant `ProfileOutboundRequest$expirationTime`
/// carries on the wire: it targets `TimeStamp`, which the model marks
/// `@timestampFormat("date-time")`.
fn instant(offset_secs: f64) -> String {
    DateTime::from_secs_f64(unix_secs() + offset_secs)
        .fmt(Format::DateTime)
        .expect("a formattable instant")
}

/// A `profileId` within the model's `ProfileId` pattern `^[a-f0-9]{32}$`: 32 lowercase
/// hex digits, one per `seed`.
fn profile_id(seed: u128) -> String {
    format!("{seed:032x}")
}

/// One profile outbound request body: `clientToken` and `profileId` are the two members
/// `ProfileOutboundRequest` requires, and `expirationTime` is carried only when
/// `offset_secs` names one (the member is optional).
fn profile_request(client_token: &str, seed: u128, offset_secs: Option<f64>) -> Value {
    let mut request = json!({
        "clientToken": client_token,
        "profileId": profile_id(seed),
        "eventTriggerContext": {
            "sourceEvent": "Campaign-WebNotification",
            "channelContext": {
                "webNotificationContext": {"sessionId": "s-1", "browserId": "b-1"}
            }
        }
    });
    if let Some(offset) = offset_secs {
        request["expirationTime"] = Value::String(instant(offset));
    }
    request
}

/// Onboard `instance` and wait out the span the clock owes its onboarding job, so the
/// instance is a campaign target by the time this returns.
async fn onboard(router: &MultiServiceRouter, region: &str, instance: &str) {
    let (status, body) = call(
        router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {instance:?}: expected onboarding to start, got {status} {body}"
    );
    tokio::time::sleep(ONBOARDING_JOB_DURATION + Duration::from_millis(150)).await;
}

/// A campaign on a freshly onboarded instance, returning its id. The campaign is left
/// in `Initialized` — the state `CreateCampaign` mints it in.
async fn campaign(router: &MultiServiceRouter, region: &str, instance: &str) -> String {
    onboard(router, region, instance).await;
    let (status, created) = call(
        router,
        region,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": format!("profile-campaign-for-{instance}"),
            "connectInstanceId": instance,
            "channelSubtypeConfig": {
                "telephony": {
                    "outboundMode": {"agentless": {}},
                    "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
                }
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the campaign to be created, got {status} {created}"
    );
    created
        .get("id")
        .and_then(Value::as_str)
        .expect("a minted campaign id")
        .to_string()
}

/// Drive one lifecycle call on `id` and assert it went through.
async fn lifecycle(router: &MultiServiceRouter, region: &str, id: &str, action: &str) {
    let (status, body) = call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/{action}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {action:?}: expected the campaign to {action}, got {status} {body}"
    );
}

/// A running campaign on a freshly onboarded instance, returning its id.
async fn running_campaign(router: &MultiServiceRouter, region: &str, instance: &str) -> String {
    let id = campaign(router, region, instance).await;
    lifecycle(router, region, &id, "start").await;
    id
}

/// Submit a batch and return `(clientToken, request id)` for every successful request
/// and `(clientToken, failureCode, id)` for every failed one.
#[allow(clippy::type_complexity)]
async fn submit(
    router: &MultiServiceRouter,
    region: &str,
    campaign: &str,
    requests: Vec<Value>,
    case: &str,
) -> (
    Vec<(String, String)>,
    Vec<(String, String, Option<String>)>,
) {
    let (status, body) = call(
        router,
        region,
        "PUT",
        &format!("/v2/campaigns/{campaign}/profile-outbound-requests"),
        &json!({"profileOutboundRequests": requests}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a batch reporting each request on its own is still HTTP 200, got \
         {status} {body}"
    );
    let string_at = |entry: &Value, member: &str| -> String {
        entry
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or("<absent>")
            .to_string()
    };
    let entries = |member: &str| -> Vec<Value> {
        body.get(member)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let successful = entries("successfulRequests")
        .iter()
        .map(|entry| (string_at(entry, "clientToken"), string_at(entry, "id")))
        .collect();
    let failed = entries("failedRequests")
        .iter()
        .map(|entry| {
            (
                string_at(entry, "clientToken"),
                string_at(entry, "failureCode"),
                entry
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
            )
        })
        .collect();
    (successful, failed)
}

/// The `clientToken`s the campaign's profile buffer holds, straight out of the store.
fn buffered_tokens(engine: &engine::InMemoryEngine, campaign: &str) -> BTreeSet<String> {
    engine
        .state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>()
        .lock()
        .campaign(campaign)
        .expect("the campaign under test")
        .profile_outbound_requests
        .keys()
        .cloned()
        .collect()
}

/// What the store holds for one taken request: `(id, profileId, sourceEvent)`.
fn stored_request(
    engine: &engine::InMemoryEngine,
    campaign: &str,
    client_token: &str,
) -> Option<(String, String, Option<String>)> {
    engine
        .state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>()
        .lock()
        .campaign(campaign)
        .expect("the campaign under test")
        .profile_outbound_requests
        .get(client_token)
        .map(|stored| {
            (
                stored.id.clone(),
                stored.profile_id.clone(),
                stored.source_event.clone(),
            )
        })
}

/// The profile requests the campaign still has pending at this instant, as the store
/// derives it from each record's stored `expirationTime`.
fn pending_count(engine: &engine::InMemoryEngine, campaign: &str) -> usize {
    engine
        .state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>()
        .lock()
        .campaign(campaign)
        .expect("the campaign under test")
        .pending_profile_outbound_requests(SystemTime::now())
}

/// Whether a profile request can be placed is decided by its `expirationTime` against
/// the clock, over a TABLE spanning what that comparison generalises over: how far the
/// instant is from now, on both sides of it, the expiration instant itself, and the
/// absence of an expiration at all (`ProfileOutboundRequest$expirationTime` is
/// optional, per `smithy_query(query="shape", shape="ProfileOutboundRequest")`).
///
/// Every row is submitted beside a control request that expires far in the future, so
/// each case also asserts the per-request channel: one request's outcome never decides
/// another's, and the batch is HTTP 200 either way.
#[tokio::test]
async fn an_expiration_already_passed_fails_that_profile_request_while_the_batch_goes_through() {
    let region = "eu-central-1";
    let router = router();
    let campaign = running_campaign(&router, region, "instance-1").await;

    // (case, the offset from now the row's expirationTime carries, placeable)
    let cases: &[(&str, Option<f64>, bool)] = &[
        ("a hundred years ago", Some(-3_155_760_000.0), false),
        ("thirty days ago", Some(-2_592_000.0), false),
        ("an hour ago", Some(-3_600.0), false),
        ("a second ago", Some(-1.0), false),
        ("the expiration instant itself", Some(0.0), false),
        ("ten seconds ahead", Some(10.0), true),
        ("an hour ahead", Some(3_600.0), true),
        ("thirty days ahead", Some(2_592_000.0), true),
        ("a hundred years ahead", Some(3_155_760_000.0), true),
        ("no expiration at all", None, true),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (row, (case, offset, placeable)) in cases.iter().enumerate() {
        let token = format!("row-{row}");
        let control = format!("control-{row}");
        let (successful, failed) = submit(
            &router,
            region,
            &campaign,
            vec![
                profile_request(&token, row as u128, *offset),
                profile_request(&control, 1_000 + row as u128, Some(3_600.0)),
            ],
            case,
        )
        .await;

        let taken: Vec<&str> = successful.iter().map(|(token, _)| token.as_str()).collect();
        let expected_taken: Vec<&str> = if *placeable {
            vec![token.as_str(), control.as_str()]
        } else {
            vec![control.as_str()]
        };
        assert_eq!(
            taken, expected_taken,
            "case {case:?}: expected {expected_taken:?} taken, got {successful:?} / {failed:?}"
        );
        let expected_failed: Vec<(String, String, Option<String>)> = if *placeable {
            Vec::new()
        } else {
            vec![(token.clone(), "InvalidInput".to_string(), None)]
        };
        assert_eq!(
            failed, expected_failed,
            "case {case:?}: a request that can no longer be placed is reported on the \
             failure channel with the modelled code and no request id, got {failed:?}"
        );

        for (token, id) in &successful {
            assert!(
                !id.is_empty() && id != "<absent>",
                "case {case:?}: a taken request carries a minted request id, got {token}={id:?}"
            );
            assert!(
                minted.insert(id.clone()),
                "case {case:?}: every taken request needs its own request id, but {id:?} was \
                 minted twice ({successful:?})"
            );
        }
    }
}

/// The call is idempotent while a request is pending, and the store holds what was
/// submitted: the `profileId` and the triggering event of every taken request.
///
/// The two rows differ along the one thing the rule turns on — how long the request
/// lives: one expires during the test, the other carries no expiration at all.
#[tokio::test]
async fn a_pending_token_answers_with_its_own_id_and_an_expired_one_frees_its_slot() {
    let region = "us-west-2";
    let (router, engine) = router_with_engine();
    let campaign = running_campaign(&router, region, "instance-1").await;

    let short_lived_secs = 2.0;
    let case = "a short-lived and an unexpiring request";
    let (successful, failed) = submit(
        &router,
        region,
        &campaign,
        vec![
            profile_request("short-lived", 7, Some(short_lived_secs)),
            profile_request("unexpiring", 8, None),
        ],
        case,
    )
    .await;
    assert!(
        failed.is_empty(),
        "case {case:?}: neither request has passed an expiration, got {successful:?} / \
         {failed:?}"
    );
    let id_of = |tokens: &Vec<(String, String)>, wanted: &str| -> String {
        tokens
            .iter()
            .find(|(token, _)| token == wanted)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("case {wanted:?}: expected it taken, got {tokens:?}"))
    };
    let short_lived_id = id_of(&successful, "short-lived");
    let unexpiring_id = id_of(&successful, "unexpiring");
    assert_eq!(
        pending_count(&engine, &campaign),
        2,
        "case {case:?}: both requests are pending, got {successful:?}"
    );

    // The store holds the request as submitted — the response's id, the profile it
    // names and the event it was triggered by — so a reader of the buffer sees what
    // the caller sent rather than a copy the handler kept.
    for (token, seed, id) in [
        ("short-lived", 7u128, &short_lived_id),
        ("unexpiring", 8u128, &unexpiring_id),
    ] {
        let stored = stored_request(&engine, &campaign, token);
        let expected = Some((
            id.clone(),
            profile_id(seed),
            Some("Campaign-WebNotification".to_string()),
        ));
        assert_eq!(
            stored, expected,
            "case {token:?}: expected the store to hold {expected:?}, got {stored:?}"
        );
    }

    // While it is still pending, the token answers idempotently with the id it was
    // first given, and nothing further is stored.
    let repeat_case = "the short-lived token repeated while pending";
    let (repeat, failed) = submit(
        &router,
        region,
        &campaign,
        vec![profile_request("short-lived", 7, Some(short_lived_secs))],
        repeat_case,
    )
    .await;
    assert_eq!(
        (id_of(&repeat, "short-lived"), failed.len()),
        (short_lived_id.clone(), 0),
        "case {repeat_case:?}: a pending request answers its token with its own id, got \
         {repeat:?}"
    );
    assert_eq!(
        pending_count(&engine, &campaign),
        2,
        "case {repeat_case:?}: a repeated token stores no second request"
    );

    // Nothing sleeps inside the emulator for an expiration — only a caller that wants
    // to see the state past it.
    tokio::time::sleep(Duration::from_secs_f64(short_lived_secs) + Duration::from_millis(400))
        .await;
    let after_case = "after the short-lived request's expiration";
    assert_eq!(
        pending_count(&engine, &campaign),
        1,
        "case {after_case:?}: the short-lived request is past its expiration, so only the \
         unexpiring one is still pending"
    );

    // A later batch materialises the expiry the clock already owed: the expired record
    // is out of the buffer, so it holds no slot, and its token is free — re-submitting
    // it mints a new request rather than handing back the stale id.
    let reuse_case = "the expired token re-submitted";
    let (reused, failed) = submit(
        &router,
        region,
        &campaign,
        vec![
            profile_request("short-lived", 7, Some(3_600.0)),
            profile_request("unexpiring", 8, None),
        ],
        reuse_case,
    )
    .await;
    assert!(
        failed.is_empty(),
        "case {reuse_case:?}: neither request has passed an expiration, got {reused:?} / \
         {failed:?}"
    );
    let reused_id = id_of(&reused, "short-lived");
    assert_ne!(
        reused_id, short_lived_id,
        "case {reuse_case:?}: the token of an expired request must mint a new request id, \
         got {reused_id:?} for a request that expired under {short_lived_id:?}"
    );
    assert_eq!(
        id_of(&reused, "unexpiring"),
        unexpiring_id,
        "case {reuse_case:?}: the still-pending request keeps the id it was first given, \
         got {reused:?}"
    );
    assert_eq!(
        buffered_tokens(&engine, &campaign),
        ["short-lived".to_string(), "unexpiring".to_string()]
            .into_iter()
            .collect::<BTreeSet<String>>(),
        "case {reuse_case:?}: the buffer holds exactly the two pending requests"
    );
}

/// A batch is taken only by a campaign the store holds that is `Running` at the instant
/// it is served, over a TABLE spanning the states a campaign reaches through its
/// lifecycle calls (source: the model's `CampaignState` enum) plus an id naming no
/// campaign at all.
#[tokio::test]
async fn only_a_running_campaign_takes_a_profile_batch() {
    let region = "ap-southeast-2";
    let (router, engine) = router_with_engine();

    // (case, the lifecycle calls that lead to the state, the state reported, status)
    let cases: &[(&str, &[&str], &str, u16)] = &[
        ("freshly created", &[], "Initialized", 409),
        ("started", &["start"], "Running", 200),
        ("paused", &["start", "pause"], "Paused", 409),
        ("resumed after a pause", &["start", "pause", "resume"], "Running", 200),
        ("stopped", &["start", "stop"], "Stopped", 409),
    ];

    for (row, (case, actions, state, expected_status)) in cases.iter().enumerate() {
        let instance = format!("instance-{row}");
        let id = campaign(&router, region, &instance).await;
        for action in *actions {
            lifecycle(&router, region, &id, action).await;
        }
        let (status, body) = call(
            &router,
            region,
            "PUT",
            &format!("/v2/campaigns/{id}/profile-outbound-requests"),
            &json!({
                "profileOutboundRequests": [profile_request("token-1", row as u128, Some(3_600.0))]
            }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if *expected_status == 200 {
            assert_eq!(
                pending_count(&engine, &id),
                1,
                "case {case:?}: a running campaign takes the request, got {body}"
            );
            continue;
        }
        assert_eq!(
            error_code(&body),
            "InvalidCampaignStateException",
            "case {case:?}: expected the modelled campaign-state refusal, got {body}"
        );
        let reported = body.get("state").and_then(Value::as_str);
        assert_eq!(
            reported,
            Some(*state),
            "case {case:?}: the refusal names the state the campaign is in, got {reported:?}"
        );
        assert_eq!(
            pending_count(&engine, &id),
            0,
            "case {case:?}: a refused batch stores nothing, got {body}"
        );
    }

    // An id the store holds no campaign under is the modelled not-found refusal, and it
    // is decided before any state question.
    let (status, body) = call(
        &router,
        region,
        "PUT",
        "/v2/campaigns/999999999999/profile-outbound-requests",
        &json!({"profileOutboundRequests": [profile_request("token-1", 99, Some(3_600.0))]}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException"),
        "case {:?}: expected the modelled not-found refusal, got {status} {body}",
        "an id naming no campaign"
    );
}
