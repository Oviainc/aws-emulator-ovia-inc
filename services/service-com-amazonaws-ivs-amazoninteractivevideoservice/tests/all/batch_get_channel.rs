// SPDX-License-Identifier: CC0-1.0

//! `BatchGetChannel`, driven over the wire through `MultiServiceRouter::dispatch` on
//! the harness `channel_lifecycle` already owns, exactly as a signed
//! `aws ivs batch-get-channel` call arrives.
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//! `BatchGetChannel` answers EVERY requested ARN individually, in the order the
//! request listed them — a persisted channel in `channels` (the same record the
//! single-ARN read answers), an ARN this account holds no channel for as a
//! `BatchError` in `errors` naming that ARN — and the call itself succeeds with HTTP
//! 200 whatever mixture of the two the request holds. Only the modelled constraints
//! on the list (`length(1..50)`) and on its member (the `ChannelArn` pattern) refuse
//! the call outright, with a 400 `ValidationException`. It is a read: it changes
//! nothing a later caller sees.

use serde_json::{json, Value};

use super::channel_lifecycle::{Ivs, CHANNEL_ARN_PREFIX};

/// The `BatchError$code` the handler reports for an ARN naming no channel: the name
/// of the error shape the single-ARN `GetChannel` raises for the same ARN.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The modelled maximum of `ChannelArnList` (`length(min: 1, max: 50)`), from
/// `smithy_query(query="shape", shape="com.amazonaws.ivs#ChannelArnList")`.
const MAX_ARNS: usize = 50;

/// Create a channel called `name` on the shared harness and return its ARN: the
/// batch read is about ARNs, so a row only ever needs the ARN back.
async fn channel_named(ivs: &Ivs, name: &str) -> String {
    ivs.create_arn(json!({ "name": name })).await
}

/// The `arn` of each item of a response list, in the order the response listed them.
/// An absent list reads as empty, which is what a client sees for an omitted member.
fn arns(body: &Value, key: &str) -> Vec<String> {
    body.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("arn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the table: the case name, the ARNs the request lists, and the `arn` of
/// each entry the response's `channels` and `errors` lists must hold, in order.
type Case = (&'static str, Vec<String>, Vec<String>, Vec<String>);

/// The rule, over a table whose rows differ in how many ARNs are asked for, how many
/// of them exist, where the absent ones sit in the request, and the order relative to
/// the store's own ordering.
#[tokio::test]
async fn batch_get_channel_answers_every_requested_arn_individually() {
    let ivs = Ivs::new();

    // Four live channels, plus well-formed ARNs naming none: one in this region, one
    // in another region, and one under a different account.
    let mut live: Vec<String> = Vec::new();
    for index in 0..4 {
        live.push(channel_named(&ivs, &format!("batch-{index}")).await);
    }
    let absent_here = format!("{CHANNEL_ARN_PREFIX}absentchannel");
    let absent_region = "arn:aws:ivs:eu-west-1:000000000000:channel/absentchannel".to_string();
    let absent_account = "arn:aws:ivs:us-east-1:123456789012:channel/absentchannel".to_string();

    // The 50-ARN row: the modelled maximum, alternating present and absent.
    let mut big_request: Vec<String> = Vec::new();
    let mut big_present: Vec<String> = Vec::new();
    let mut big_missing: Vec<String> = Vec::new();
    for index in 0..MAX_ARNS / 2 {
        let present = channel_named(&ivs, &format!("bulk-{index}")).await;
        let missing = format!("{CHANNEL_ARN_PREFIX}bulkabsent{index}");
        big_request.push(present.clone());
        big_request.push(missing.clone());
        big_present.push(present);
        big_missing.push(missing);
    }

    let cases: Vec<Case> = vec![
        (
            "one ARN, present",
            vec![live[0].clone()],
            vec![live[0].clone()],
            vec![],
        ),
        (
            "one ARN, absent",
            vec![absent_here.clone()],
            vec![],
            vec![absent_here.clone()],
        ),
        (
            "present, absent, present — the partial failure",
            vec![live[0].clone(), absent_here.clone(), live[1].clone()],
            vec![live[0].clone(), live[1].clone()],
            vec![absent_here.clone()],
        ),
        (
            "absent first, then present",
            vec![absent_here.clone(), live[2].clone()],
            vec![live[2].clone()],
            vec![absent_here.clone()],
        ),
        (
            "every ARN absent, in three flavours",
            vec![
                absent_here.clone(),
                absent_region.clone(),
                absent_account.clone(),
            ],
            vec![],
            vec![
                absent_here.clone(),
                absent_region.clone(),
                absent_account.clone(),
            ],
        ),
        (
            "every ARN present, requested against the store's ordering",
            {
                let mut reversed = live.clone();
                reversed.sort();
                reversed.reverse();
                reversed
            },
            {
                let mut reversed = live.clone();
                reversed.sort();
                reversed.reverse();
                reversed
            },
            vec![],
        ),
        (
            "the same present ARN twice, and one absent ARN twice",
            vec![
                live[3].clone(),
                live[3].clone(),
                absent_here.clone(),
                absent_here.clone(),
            ],
            vec![live[3].clone(), live[3].clone()],
            vec![absent_here.clone(), absent_here.clone()],
        ),
        (
            "the full 50-ARN batch, alternating present and absent",
            big_request.clone(),
            big_present.clone(),
            big_missing.clone(),
        ),
    ];

    for (case, requested, expect_channels, expect_errors) in cases {
        let reply = ivs
            .call("BatchGetChannel", &json!({ "arns": requested }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a batch read succeeds whatever mixture it holds, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            arns(&reply.body, "channels"),
            expect_channels,
            "case {case:?}: expected channels {expect_channels:?}, got {:?}",
            arns(&reply.body, "channels")
        );
        assert_eq!(
            arns(&reply.body, "errors"),
            expect_errors,
            "case {case:?}: expected errors {expect_errors:?}, got {:?}",
            arns(&reply.body, "errors")
        );
        // Every requested ARN is accounted for exactly once, by one list or the other.
        assert_eq!(
            arns(&reply.body, "channels").len() + arns(&reply.body, "errors").len(),
            requested.len(),
            "case {case:?}: the two lists must account for all {} requested ARNs, got {}",
            requested.len(),
            reply.body
        );

        // Each error names its own ARN, the not-found code, and a message about it.
        let errors = reply.body["errors"].as_array().cloned().unwrap_or_default();
        for error in &errors {
            let arn = error["arn"].as_str().unwrap_or_default();
            assert_eq!(
                error["code"].as_str().unwrap_or_default(),
                NOT_FOUND_CODE,
                "case {case:?}: expected code {NOT_FOUND_CODE} for {arn}, got {error}"
            );
            assert!(
                error["message"]
                    .as_str()
                    .is_some_and(|message| message.contains(arn)),
                "case {case:?}: expected a message naming {arn}, got {error}"
            );
        }

        // A channel the batch answers is the very record the single-ARN read answers.
        for channel in reply.body["channels"].as_array().cloned().unwrap_or_default() {
            let arn = channel["arn"].as_str().unwrap_or_default().to_string();
            let single = ivs.ok("GetChannel", json!({ "arn": arn })).await;
            assert_eq!(
                single["channel"], channel,
                "case {case:?}: BatchGetChannel and GetChannel must answer one record for {arn}, \
                 got {} and {channel}",
                single["channel"]
            );
        }
    }
}

/// The two modelled constraints on the input are refused outright — the list bounds
/// (`length(min: 1, max: 50)`) at each published bound and one past it, and a member
/// outside the `ChannelArn` pattern — rather than answered as a batch of errors.
#[tokio::test]
async fn batch_get_channel_refuses_input_outside_the_modelled_constraints() {
    let ivs = Ivs::new();
    let live = channel_named(&ivs, "bounds").await;

    // Each published bound of the list length is accepted...
    for count in [1usize, MAX_ARNS] {
        let requested: Vec<String> = (0..count)
            .map(|index| format!("{CHANNEL_ARN_PREFIX}bound{index}"))
            .collect();
        let reply = ivs
            .call("BatchGetChannel", &json!({ "arns": requested }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {count} ARNs (inside length(1..{MAX_ARNS})): expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        // Every ARN of the boundary row is accounted for: none of them exists, so
        // each lands in `errors` and `channels` stays empty.
        assert_eq!(
            (
                arns(&reply.body, "errors").len(),
                arns(&reply.body, "channels").len()
            ),
            (count, 0),
            "case {count} ARNs (inside length(1..{MAX_ARNS})): expected {count} errors and no \
             channels, got {}",
            reply.body
        );
    }

    // ...and one past each of them is a 400, as is a member outside the ARN pattern
    // and an absent required member.
    let too_many: Vec<String> = (0..=MAX_ARNS)
        .map(|index| format!("{CHANNEL_ARN_PREFIX}over{index}"))
        .collect();
    let refused: Vec<(&str, Value)> = vec![
        ("zero ARNs, one below the minimum", json!({ "arns": [] })),
        (
            "51 ARNs, one past the maximum",
            json!({ "arns": too_many }),
        ),
        ("no arns member at all", json!({})),
        (
            "a member that is not an ARN",
            json!({ "arns": ["not-an-arn"] }),
        ),
        (
            "a member naming another service's resource",
            json!({ "arns": [format!("{CHANNEL_ARN_PREFIX}ok"), "arn:aws:ivs:us-east-1:000000000000:stream-key/abc123"] }),
        ),
        (
            "a member with an underscore in its id",
            json!({ "arns": [format!("{CHANNEL_ARN_PREFIX}has_underscore")] }),
        ),
    ];
    for (case, request) in refused {
        let reply = ivs.call("BatchGetChannel", &request).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    // None of the refusals disturbed the channel that does exist.
    let reply = ivs
        .call("BatchGetChannel", &json!({ "arns": [live.clone()] }))
        .await;
    assert_eq!(
        arns(&reply.body, "channels"),
        vec![live.clone()],
        "case after the refusals: expected the live channel {live}, got {}",
        reply.body
    );
}

/// A reporting operation: repeating it answers identically, and it changes nothing a
/// later `ListChannels` sees.
#[tokio::test]
async fn batch_get_channel_changes_nothing_a_later_caller_sees() {
    let ivs = Ivs::new();
    let first = channel_named(&ivs, "read-only-a").await;
    let second = channel_named(&ivs, "read-only-b").await;
    let before = ivs.ok("ListChannels", json!({})).await;

    let requested = json!({ "arns": [first, format!("{CHANNEL_ARN_PREFIX}gone"), second] });
    let once = ivs.ok("BatchGetChannel", requested.clone()).await;
    let twice = ivs.ok("BatchGetChannel", requested.clone()).await;
    assert_eq!(
        once, twice,
        "case repeated BatchGetChannel: a read answers identically, got {once} then {twice}"
    );

    let after = ivs.ok("ListChannels", json!({})).await;
    assert_eq!(
        before, after,
        "case ListChannels around BatchGetChannel: a read persists nothing, got {before} then {after}"
    );
}
