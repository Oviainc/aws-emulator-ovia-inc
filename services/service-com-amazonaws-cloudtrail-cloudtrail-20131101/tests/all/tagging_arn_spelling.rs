// SPDX-License-Identifier: CC0-1.0

//! How a tagging call reads the ARN it is handed, end to end over the wire.
//!
//! RULE: a tagging `ResourceId` is an ARN, and a CloudTrail resource ARN is spelled
//! only from the characters the model's ARN types admit — `ChannelArn`,
//! `EventDataStoreArn` and `DashboardArn` are each `^[a-zA-Z0-9._/\-:]+$`
//! (`smithy_query(query="shape", shape="ChannelArn,EventDataStoreArn,DashboardArn")`) —
//! so a value carrying any other character is not an ARN at all, whatever resource
//! type is written inside it: it is refused with `CloudTrailARNInvalidException`
//! (HTTP 400), naming the value, rather than looked up and reported as a resource that
//! does not exist. A tagging `ResourceId` targets a plain `String`, carrying no
//! `@pattern` of its own, so nothing else in the request walk applies that set.
//!
//! The rows differ in WHICH character is outside the set (punctuation, whitespace,
//! non-ASCII), in WHERE in the value it sits (the resource id, the Region field), and
//! in the resource TYPE the value names — and each runs against every tagging
//! operation, because the reading is the `ResourceId`'s, not one operation's. A
//! control row per resource type then takes the offending character OUT and shows the
//! same value resolving by kind again, so the refusal is the spelling and not the kind.

use serde_json::json;

use super::emulator::{Emulator, Reply, HOME_REGION};

/// The account every emulated ARN is minted under (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The modelled error a value that is no CloudTrail ARN is refused with, and the
/// status its `@httpError` declares
/// (`smithy_query(query="operation", operation="AddTags")`).
const CLOUDTRAIL_ARN_INVALID: &str = "CloudTrailARNInvalidException";
/// See [`CLOUDTRAIL_ARN_INVALID`].
const CLOUDTRAIL_ARN_INVALID_STATUS: u16 = 400;

/// An ARN of `resource_type` in `region`, with `id` as its resource id.
fn arn(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:cloudtrail:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// The tagging operations, each called with the one `ResourceId` under test: the pair
/// that writes tags and the read that reports them
/// (`AddTags` / `RemoveTags` take `ResourceId`, `ListTags` a `ResourceIdList`).
async fn tagging_calls(emulator: &Emulator, resource_id: &str) -> Vec<(&'static str, Reply)> {
    let tags = json!([{"Key": "team", "Value": "security"}]);
    let mut replies = Vec::new();
    for operation in ["AddTags", "RemoveTags"] {
        replies.push((
            operation,
            emulator
                .call_in(
                    operation,
                    HOME_REGION,
                    &json!({"ResourceId": resource_id, "TagsList": tags}),
                )
                .await,
        ));
    }
    replies.push((
        "ListTags",
        emulator
            .call_in(
                "ListTags",
                HOME_REGION,
                &json!({"ResourceIdList": [resource_id]}),
            )
            .await,
    ));
    replies
}

/// RULE: a `ResourceId` carrying a character no CloudTrail ARN is spelled with is
/// refused as an invalid ARN by every tagging operation, naming the value it refused.
#[tokio::test]
async fn a_resource_id_outside_the_arn_character_set_is_not_an_arn() {
    let emulator = Emulator::new();

    // (case, the ResourceId the request names)
    let rows: Vec<(&str, String)> = vec![
        (
            "exclamation marks around a channel id",
            arn(HOME_REGION, "channel", "!!invalid-id!!"),
        ),
        (
            "a space inside a channel id",
            arn(HOME_REGION, "channel", "two words"),
        ),
        (
            "a hash in an event data store id",
            arn(HOME_REGION, "eventdatastore", "a1b9f92c#fa85"),
        ),
        (
            "a percent sign in a trail name",
            arn(HOME_REGION, "trail", "a%trail"),
        ),
        (
            "an asterisk in a dashboard name",
            arn(HOME_REGION, "dashboard", "a*dashboard"),
        ),
        (
            "a plus sign in an event data store id",
            arn(HOME_REGION, "eventdatastore", "a1b9f92c+fa85"),
        ),
        (
            "a non-ASCII letter in a channel id",
            arn(HOME_REGION, "channel", "canalé"),
        ),
        (
            "a non-ASCII script in a channel id",
            arn(HOME_REGION, "channel", "频道"),
        ),
        (
            // Outside the id, too: the set is the whole value's, not the id's.
            "an exclamation mark in the Region field",
            arn("us-east-1!", "channel", "a-channel"),
        ),
        (
            "a question mark where the ARN prefix belongs",
            format!("arn?aws:cloudtrail:{HOME_REGION}:{ACCOUNT}:channel/a-channel"),
        ),
    ];

    for (case, resource_id) in &rows {
        let expected = format!("The ARN {resource_id} is invalid.");
        for (operation, reply) in tagging_calls(&emulator, resource_id).await {
            let case = format!("{operation}: {case}");
            reply.expect_error(&case, CLOUDTRAIL_ARN_INVALID_STATUS, CLOUDTRAIL_ARN_INVALID);
            assert_eq!(
                reply.message(),
                expected,
                "case {case}: expected the refusal {expected:?}, got {reply:?}"
            );
        }
    }
}

/// RULE (the other side of it): the refusal is the SPELLING, not the resource type —
/// the same ARN written only from the characters an ARN is spelled with resolves in
/// the registry its resource type names, and answers that kind's own refusal.
#[tokio::test]
async fn an_arn_spelled_within_the_character_set_resolves_by_its_kind() {
    let emulator = Emulator::new();

    // (case, the ResourceId, the modelled error each tagging call answers)
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a channel ARN naming no channel",
            arn(HOME_REGION, "channel", "39032a83-30fe-4581-8bd3-351632c2be72"),
            "ChannelNotFoundException",
        ),
        (
            "an event data store ARN naming no store",
            arn(
                HOME_REGION,
                "eventdatastore",
                "a1b9f92c-fa85-40de-9198-dd92e0c55524",
            ),
            "EventDataStoreNotFoundException",
        ),
        (
            "a dashboard ARN naming no dashboard",
            arn(HOME_REGION, "dashboard", "no-such-dashboard"),
            "ResourceNotFoundException",
        ),
        (
            // A colon-separated resource segment is inside the set as well, which is
            // why the set is read off the whole value rather than off a `/` split.
            "a channel ARN written with a colon before its id",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{ACCOUNT}:channel:a-channel"),
            "ChannelNotFoundException",
        ),
    ];

    for (case, resource_id, expected_code) in &rows {
        for (operation, reply) in tagging_calls(&emulator, resource_id).await {
            let case = format!("{operation}: {case}");
            assert_eq!(
                (reply.status, reply.error_code()),
                (400, *expected_code),
                "case {case}: expected HTTP 400 {expected_code}, got {reply:?}"
            );
        }
    }
}
