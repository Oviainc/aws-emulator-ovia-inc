// SPDX-License-Identifier: CC0-1.0

//! `ProfileOutboundRequest$eventTriggerContext` reaches the record WHOLE, at every
//! depth of the structure it nests.
//!
//! Every case drives real HTTP through the router the production entrypoint serves and
//! then reads the record the call wrote straight out of the store, which is the only
//! reader there is: no modelled operation of this service reports a pending profile
//! request back.
//!
//! The rule under test: a request a campaign takes is recorded with every member it
//! carried, at every depth — so an optional structure a caller sends EMPTY is stored as
//! present-and-empty, and a leaf it fills is stored with the value it sent, whatever
//! depth that leaf sits at. `buffered_requests_keep_what_was_submitted.rs` pins that
//! rule over which members of the context are filled; the table here spans the other
//! thing it generalises over — how DEEP the submitted context reaches, from a context
//! whose nesting stops at `channelContext` to one filled down to
//! `webNotificationContext`'s leaves — plus each published bound of `SessionId`'s and
//! `BrowserId`'s `@length` (1..36) and the `@length` min of `SourceEvent` (1), and what
//! an idempotent repeat does to a context already stored.
//!
//! Member names, nesting, optionality and those bounds are the model's
//! (`smithy_query(query="shape",
//! shape="EventTriggerContext,ChannelContext,WebNotificationContext,SessionId,BrowserId,SourceEvent")`).

use serde_json::{json, Value};

use super::buffered_requests_keep_what_was_submitted::{
    running_campaign, set_if_present, stored_profile_request, take_all,
};
use super::common::{call, error_code, instant, router_with_engine};

/// How far ahead of now every request below expires: far enough that no row is weighed
/// as expired, which is a rule of its own (`outbound_request_expiration.rs`).
const LIVE_FOR_SECS: f64 = 3_600.0;

/// A `profileId` within `ProfileOutboundRequest$profileId`'s pattern `^[a-f0-9]{32}$`:
/// 32 lowercase hex digits, one value per `row`.
fn profile_of(row: usize) -> String {
    format!("{row:032x}")
}

/// One profile outbound request body carrying `context` when the row has one.
fn request_with(row: usize, context: Option<Value>) -> Value {
    let mut request = json!({
        "clientToken": format!("row-{row}"),
        "profileId": profile_of(row),
        "expirationTime": instant(LIVE_FOR_SECS),
    });
    set_if_present(&mut request, "eventTriggerContext", context);
    request
}

/// The whole `eventTriggerContext` a taken request carried is in its record, at every
/// depth the caller filled: a nested structure sent empty is stored present-and-empty,
/// and a leaf sent at either published bound of its `@length` is stored as sent.
#[tokio::test]
async fn an_event_trigger_context_reaches_the_record_at_every_depth_it_nests() {
    let region = "ca-central-1";
    let (router, engine) = router_with_engine();
    let campaign = running_campaign(&router, region, "instance-1").await;

    // `SessionId` and `BrowserId` are both @length 1..36, so these rows reach both
    // published bounds of each.
    let shortest = "a".to_string();
    let longest = "z".repeat(36);
    // `SourceEvent` is @length min 1 with no published maximum, so a long one is as
    // valid as a one-character one.
    let long_source_event = "web-notification-".repeat(20);

    // (case, the eventTriggerContext the row carries, if any)
    let cases: &[(&str, Option<Value>)] = &[
        (
            "nesting stops at the channel context",
            Some(json!({"channelContext": {}})),
        ),
        (
            "nesting stops at the web notification context",
            Some(json!({"channelContext": {"webNotificationContext": {}}})),
        ),
        (
            "a source event beside an empty channel context",
            Some(json!({"sourceEvent": "order-placed", "channelContext": {}})),
        ),
        (
            "a source event beside an empty web notification context",
            Some(json!({
                "sourceEvent": "cart-abandoned",
                "channelContext": {"webNotificationContext": {}}
            })),
        ),
        (
            "session at its length minimum, browser at its maximum",
            Some(json!({"channelContext": {"webNotificationContext": {
                "sessionId": shortest.clone(),
                "browserId": longest.clone()
            }}})),
        ),
        (
            "session at its length maximum, browser at its minimum",
            Some(json!({"channelContext": {"webNotificationContext": {
                "sessionId": longest.clone(),
                "browserId": shortest.clone()
            }}})),
        ),
        (
            "a one-character source event, filled to the leaves",
            Some(json!({
                "sourceEvent": "e",
                "channelContext": {"webNotificationContext": {
                    "sessionId": "9f8e7d6c-5b4a-3928-1716-05f4e3d2c1b0",
                    "browserId": "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9"
                }}
            })),
        ),
        (
            "a long source event, filled to the leaves",
            Some(json!({
                "sourceEvent": long_source_event,
                "channelContext": {"webNotificationContext": {
                    "sessionId": shortest.clone(),
                    "browserId": shortest.clone()
                }}
            })),
        ),
    ];

    let requests: Vec<Value> = cases
        .iter()
        .enumerate()
        .map(|(row, (_, context))| request_with(row, context.clone()))
        .collect();
    let taken = take_all(
        &router,
        region,
        &campaign,
        "profile-outbound-requests",
        "profileOutboundRequests",
        requests,
    )
    .await;

    for (row, (case, context)) in cases.iter().enumerate() {
        let token = format!("row-{row}");
        let reported = taken
            .get(&token)
            .unwrap_or_else(|| panic!("case {case:?}: expected {token} taken, got {taken:?}"));
        let (stored_id, stored_profile, stored_context) =
            stored_profile_request(&engine, region, &campaign, &token).unwrap_or_else(|| {
                panic!("case {case:?}: expected the buffer to hold a record under {token:?}")
            });
        assert_eq!(
            &stored_id, reported,
            "case {case:?}: the record must carry the request id the response reported, got \
             {stored_id:?}"
        );
        assert_eq!(
            stored_profile,
            profile_of(row),
            "case {case:?}: expected the record to keep the profile it is to reach, got \
             {stored_profile:?}"
        );
        assert_eq!(
            stored_context, *context,
            "case {case:?}: expected the record to keep the context as submitted, got \
             {stored_context:?}"
        );
    }
}

/// An idempotent repeat changes nothing about the context already recorded: the
/// `clientToken` is answered with the id it was first given, and the record still holds
/// the context the FIRST submission carried, whole — the store takes nothing further
/// for a token it already holds, as the operation's "This API is idempotent" says.
#[tokio::test]
async fn a_repeated_token_leaves_the_context_it_first_recorded_alone() {
    let region = "sa-east-1";
    let (router, engine) = router_with_engine();
    let campaign = running_campaign(&router, region, "instance-1").await;

    let first = json!({
        "sourceEvent": "Campaign-WebNotification",
        "channelContext": {"webNotificationContext": {
            "sessionId": "11111111-2222-3333-4444-555555555555",
            "browserId": "66666666-7777-8888-9999-aaaaaaaaaaaa"
        }}
    });

    // Each row repeats the SAME token with a context differing from the stored one in
    // one way: a different leaf, a shallower nesting, and no context at all.
    let repeats: &[(&str, Option<Value>)] = &[
        (
            "the same token with a different session and browser",
            Some(json!({
                "sourceEvent": "order-placed",
                "channelContext": {"webNotificationContext": {
                    "sessionId": "ffffffff-eeee-dddd-cccc-bbbbbbbbbbbb",
                    "browserId": "aaaaaaaa-9999-8888-7777-666666666666"
                }}
            })),
        ),
        (
            "the same token with a shallower context",
            Some(json!({"sourceEvent": "cart-abandoned"})),
        ),
        ("the same token with no context at all", None),
    ];

    let taken = take_all(
        &router,
        region,
        &campaign,
        "profile-outbound-requests",
        "profileOutboundRequests",
        vec![request_with(0, Some(first.clone()))],
    )
    .await;
    let first_id = taken
        .get("row-0")
        .unwrap_or_else(|| panic!("expected the first submission taken, got {taken:?}"))
        .clone();

    for (case, context) in repeats {
        let repeated = take_all(
            &router,
            region,
            &campaign,
            "profile-outbound-requests",
            "profileOutboundRequests",
            vec![request_with(0, context.clone())],
        )
        .await;
        let reported = repeated
            .get("row-0")
            .unwrap_or_else(|| panic!("case {case:?}: expected the repeat taken, got {repeated:?}"));
        assert_eq!(
            reported, &first_id,
            "case {case:?}: a pending token answers with the id it was first given, got \
             {reported:?}"
        );
        let (_, _, stored_context) = stored_profile_request(&engine, region, &campaign, "row-0")
            .unwrap_or_else(|| panic!("case {case:?}: expected the buffer to hold the record"));
        assert_eq!(
            stored_context,
            Some(first.clone()),
            "case {case:?}: a repeat stores nothing further, so the record keeps the context \
             first submitted, got {stored_context:?}"
        );
    }
}

/// A context member outside its published `@length` bounds is refused whole, with HTTP
/// 400 `ValidationException`, and the batch stores nothing — a nested member is weighed
/// against the model's constraints exactly as a top-level one is.
#[tokio::test]
async fn a_context_member_outside_its_published_length_is_refused_and_stores_nothing() {
    let region = "ap-south-1";
    let (router, engine) = router_with_engine();
    let campaign = running_campaign(&router, region, "instance-1").await;

    // One past each published bound: `SessionId` and `BrowserId` are @length 1..36 and
    // `SourceEvent` is @length min 1, so an empty string is under every minimum and 37
    // characters is over the two maximums.
    let too_long = "x".repeat(37);
    let cases: &[(&str, Value)] = &[
        (
            "a session one character over its maximum",
            json!({"channelContext": {"webNotificationContext": {"sessionId": too_long}}}),
        ),
        (
            "a browser one character over its maximum",
            json!({"channelContext": {"webNotificationContext": {"browserId": too_long}}}),
        ),
        (
            "a session under its minimum",
            json!({"channelContext": {"webNotificationContext": {"sessionId": ""}}}),
        ),
        (
            "a browser under its minimum",
            json!({"channelContext": {"webNotificationContext": {"browserId": ""}}}),
        ),
        (
            "a source event under its minimum",
            json!({"sourceEvent": ""}),
        ),
    ];

    for (row, (case, context)) in cases.iter().enumerate() {
        let (status, body) = call(
            &router,
            region,
            "PUT",
            &format!("/v2/campaigns/{campaign}/profile-outbound-requests"),
            &json!({"profileOutboundRequests": [request_with(row, Some(context.clone()))]}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled constraint refusal, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {} {body}",
            error_code(&body)
        );
        let stored = stored_profile_request(&engine, region, &campaign, &format!("row-{row}"));
        assert!(
            stored.is_none(),
            "case {case:?}: a refused batch stores nothing, got {stored:?}"
        );
    }
}
