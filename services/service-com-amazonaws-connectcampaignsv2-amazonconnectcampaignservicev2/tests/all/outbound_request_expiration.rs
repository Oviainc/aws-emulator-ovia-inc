// SPDX-License-Identifier: CC0-1.0

//! An outbound request's `expirationTime` is the instant after which it cannot be
//! dialled, and the campaign's buffer holds only the requests still pending.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handler together. The rules under test:
//!
//! * a request whose `expirationTime` the clock has already passed when the batch
//!   arrives is reported on the per-request failure channel carrying the modelled
//!   `FailureCode` `InvalidInput`, under no dial request id, and the requests beside it
//!   in the same batch are taken as usual (the batch itself is still HTTP 200);
//! * a request still inside its `expirationTime` is taken under its own minted
//!   `DialRequestId`, and answers its `clientToken` idempotently while it is pending;
//! * once a taken request has passed its `expirationTime` it is no longer pending: it
//!   releases its buffer slot and its `clientToken`, so re-submitting that token mints a
//!   new request instead of answering with the stale dial id.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, router, router_with_engine};

/// Now, as seconds since the Unix epoch.
fn unix_secs() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("a test clock after the Unix epoch")
        .as_secs_f64()
}

/// `now + offset_secs` as the RFC 3339 instant `OutboundRequest$expirationTime` carries
/// on the wire: it targets `TimeStamp`, which the model marks
/// `@timestampFormat("date-time")`.
fn instant(offset_secs: f64) -> String {
    DateTime::from_secs_f64(unix_secs() + offset_secs)
        .fmt(Format::DateTime)
        .expect("a formattable instant")
}

/// A minimal telephony `channelSubtypeConfig`: `outboundMode` and
/// `defaultOutboundConfig` are both required by the model.
fn telephony_config() -> Value {
    json!({
        "telephony": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
        }
    })
}

/// One outbound request body expiring `offset_secs` from now: the members
/// `OutboundRequest` requires, for telephony.
fn request_expiring_in(client_token: &str, offset_secs: f64) -> Value {
    json!({
        "clientToken": client_token,
        "expirationTime": instant(offset_secs),
        "channelSubtypeParameters": {
            "telephony": {"destinationPhoneNumber": "+15555550100", "attributes": {}}
        }
    })
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

/// A running telephony campaign on a freshly onboarded instance, returning its id.
async fn running_campaign(router: &MultiServiceRouter, region: &str, instance: &str) -> String {
    onboard(router, region, instance).await;
    let (status, created) = call(
        router,
        region,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": format!("dialler-for-{instance}"),
            "connectInstanceId": instance,
            "channelSubtypeConfig": telephony_config(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the campaign to be created, got {status} {created}"
    );
    let id = created
        .get("id")
        .and_then(Value::as_str)
        .expect("a minted campaign id")
        .to_string();
    let (status, body) = call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/start"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the campaign to start, got {status} {body}"
    );
    id
}

/// Submit a batch and return `(clientToken, dial request id)` for every successful
/// request and `(clientToken, failureCode, id)` for every failed one.
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
        &format!("/v2/campaigns/{campaign}/outbound-requests"),
        &json!({"outboundRequests": requests}),
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

/// The `clientToken`s the campaign's buffer holds, straight out of the store — read
/// from the region the campaign was created in, which is the only region that holds
/// it.
fn buffered_tokens(
    engine: &engine::InMemoryEngine,
    region: &str,
    campaign: &str,
) -> BTreeSet<String> {
    engine
        .state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>()
        .lock()
        .campaign(region, campaign)
        .expect("the campaign under test")
        .outbound_requests
        .keys()
        .cloned()
        .collect()
}

/// The requests the campaign still has pending at this instant, as the store derives
/// it from each record's stored `expirationTime`.
fn pending_count(engine: &engine::InMemoryEngine, region: &str, campaign: &str) -> usize {
    engine
        .state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>()
        .lock()
        .campaign(region, campaign)
        .expect("the campaign under test")
        .pending_outbound_requests(SystemTime::now())
}

/// Whether a request is dialable is decided by its `expirationTime` against the clock,
/// over a TABLE spanning what that comparison generalises over: how far the instant is
/// from now, on both sides of it, and the expiration instant itself.
///
/// Every row is submitted beside a control request that expires far in the future, so
/// each case also asserts the per-request channel: one request's outcome never decides
/// another's, and the batch is HTTP 200 either way.
#[tokio::test]
async fn an_expiration_already_passed_fails_that_request_while_the_batch_goes_through() {
    let region = "eu-central-1";
    let router = router();
    let campaign = running_campaign(&router, region, "instance-1").await;

    // (case, the offset from now the row's expirationTime carries, dialable)
    let cases: &[(&str, f64, bool)] = &[
        ("a hundred years ago", -3_155_760_000.0, false),
        ("thirty days ago", -2_592_000.0, false),
        ("an hour ago", -3_600.0, false),
        ("a second ago", -1.0, false),
        ("the expiration instant itself", 0.0, false),
        ("ten seconds ahead", 10.0, true),
        ("an hour ahead", 3_600.0, true),
        ("thirty days ahead", 2_592_000.0, true),
        ("a hundred years ahead", 3_155_760_000.0, true),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (row, (case, offset, dialable)) in cases.iter().enumerate() {
        let token = format!("row-{row}");
        let control = format!("control-{row}");
        let (successful, failed) = submit(
            &router,
            region,
            &campaign,
            vec![
                request_expiring_in(&token, *offset),
                request_expiring_in(&control, 3_600.0),
            ],
            case,
        )
        .await;

        let taken: Vec<&str> = successful
            .iter()
            .map(|(token, _)| token.as_str())
            .collect();
        let expected_taken: Vec<&str> = if *dialable {
            vec![token.as_str(), control.as_str()]
        } else {
            vec![control.as_str()]
        };
        assert_eq!(
            taken, expected_taken,
            "case {case:?}: expected {expected_taken:?} taken, got {successful:?} / {failed:?}"
        );
        let expected_failed: Vec<(String, String, Option<String>)> = if *dialable {
            Vec::new()
        } else {
            vec![(token.clone(), "InvalidInput".to_string(), None)]
        };
        assert_eq!(
            failed, expected_failed,
            "case {case:?}: a request that cannot be dialled is reported on the failure \
             channel with the modelled code and no dial id, got {failed:?}"
        );

        for (token, id) in &successful {
            assert!(
                !id.is_empty() && id != "<absent>",
                "case {case:?}: a taken request carries a minted dial id, got {token}={id:?}"
            );
            assert!(
                minted.insert(id.clone()),
                "case {case:?}: every taken request needs its own dial id, but {id:?} was \
                 minted twice ({successful:?})"
            );
        }
    }
}

/// A request that has passed its `expirationTime` is no longer pending: it releases
/// both its buffer slot and its `clientToken`, while a request still inside its
/// expiration keeps answering that token with the id it was first given.
///
/// The two rows differ along the one thing the rule turns on — how long the request
/// lives: one expires during the test, the other long after it.
#[tokio::test]
async fn an_expired_request_releases_its_buffer_slot_and_its_client_token() {
    let region = "us-west-2";
    let (router, engine) = router_with_engine();
    let campaign = running_campaign(&router, region, "instance-1").await;

    let short_lived_secs = 2.0;
    let (successful, failed) = submit(
        &router,
        region,
        &campaign,
        vec![
            request_expiring_in("short-lived", short_lived_secs),
            request_expiring_in("long-lived", 3_155_760_000.0),
        ],
        "a short-lived and a long-lived request",
    )
    .await;
    assert!(
        failed.is_empty(),
        "case {:?}: both requests are inside their expiration, got {successful:?} / {failed:?}",
        "a short-lived and a long-lived request"
    );
    let id_of = |tokens: &Vec<(String, String)>, wanted: &str| -> String {
        tokens
            .iter()
            .find(|(token, _)| token == wanted)
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| panic!("case {wanted:?}: expected it taken, got {tokens:?}"))
    };
    let short_lived_id = id_of(&successful, "short-lived");
    let long_lived_id = id_of(&successful, "long-lived");
    assert_eq!(
        pending_count(&engine, region, &campaign),
        2,
        "case {:?}: both requests are pending while inside their expiration",
        "a short-lived and a long-lived request"
    );

    // While it is still pending, the token answers idempotently with the id it was
    // first given.
    let (repeat, failed) = submit(
        &router,
        region,
        &campaign,
        vec![request_expiring_in("short-lived", short_lived_secs)],
        "the short-lived token repeated while pending",
    )
    .await;
    assert_eq!(
        (id_of(&repeat, "short-lived"), failed.len()),
        (short_lived_id.clone(), 0),
        "case {:?}: a pending request answers its token with its own id, got {repeat:?}",
        "the short-lived token repeated while pending"
    );

    // Nothing sleeps inside the emulator for an expiration — only a caller that wants
    // to see the state past it.
    tokio::time::sleep(Duration::from_secs_f64(short_lived_secs) + Duration::from_millis(400))
        .await;
    assert_eq!(
        pending_count(&engine, region, &campaign),
        1,
        "case {:?}: the short-lived request is past its expiration, so only the long-lived \
         one is still pending",
        "after the short-lived request's expiration"
    );

    // A later batch materialises the expiry the clock already owed: the expired record
    // is out of the buffer, so it holds no slot.
    let (successful, failed) = submit(
        &router,
        region,
        &campaign,
        vec![request_expiring_in("fresh", 3_600.0)],
        "a batch submitted after the expiration",
    )
    .await;
    assert!(
        failed.is_empty(),
        "case {:?}: a fresh request is taken, got {successful:?} / {failed:?}",
        "a batch submitted after the expiration"
    );
    assert_eq!(
        buffered_tokens(&engine, region, &campaign),
        ["fresh".to_string(), "long-lived".to_string()]
            .into_iter()
            .collect::<BTreeSet<String>>(),
        "case {:?}: the expired request must have left the buffer",
        "a batch submitted after the expiration"
    );

    // And its token is free: re-submitting it mints a new request rather than handing
    // back the dial id of a request that has expired.
    let (reused, failed) = submit(
        &router,
        region,
        &campaign,
        vec![
            request_expiring_in("short-lived", 3_600.0),
            request_expiring_in("long-lived", 3_155_760_000.0),
        ],
        "the expired token re-submitted",
    )
    .await;
    assert!(
        failed.is_empty(),
        "case {:?}: both requests are inside their expiration, got {reused:?} / {failed:?}",
        "the expired token re-submitted"
    );
    let reused_id = id_of(&reused, "short-lived");
    assert_ne!(
        reused_id, short_lived_id,
        "case {:?}: the token of an expired request must mint a new dial id, got \
         {reused_id:?} for a request that expired under {short_lived_id:?}",
        "the expired token re-submitted"
    );
    assert_eq!(
        id_of(&reused, "long-lived"),
        long_lived_id,
        "case {:?}: the still-pending request keeps the id it was first given, got {reused:?}",
        "the expired token re-submitted"
    );
}
