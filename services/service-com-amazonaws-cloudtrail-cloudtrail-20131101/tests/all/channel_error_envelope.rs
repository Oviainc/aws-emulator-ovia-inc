// SPDX-License-Identifier: CC0-1.0

//! The exact wire envelope of a channel refusal — the bytes a client parses.
//!
//! The behavioural suite (`channel_deletion.rs`) reads the status, the error code and
//! the message of every case through one helper. This suite pins the WHOLE body once,
//! for each of the two refusals `DeleteChannel` models: the HTTP status and the error
//! body's exact member set — so an envelope change (a renamed member, a namespaced
//! `__type`, an added member) cannot pass unnoticed by a suite that only reads two
//! fields out of the body.

use serde_json::json;

// The emulator-under-test harness is shared by this crate's suites; see
// `tests/all/emulator.rs`.
use super::emulator::{Emulator, HOME_REGION};

/// RULE: a refusal goes back as the `awsJson1_1` error envelope — the status the handler
/// states, and a body of exactly `__type` (the bare error code a client branches on) and
/// `Message` (the modelled member name).
///
/// One row per refusal `DeleteChannel` models: an identifier naming no channel, and an
/// `arn:`-prefixed identifier that is not a channel ARN.
#[tokio::test]
async fn a_channel_refusal_renders_the_awsjson_error_envelope() {
    let emulator = Emulator::new();
    let absent = format!(
        "arn:aws:cloudtrail:{HOME_REGION}:000000000000:channel/\
         39032a83-30fe-4581-8bd3-351632c2be72"
    );
    let not_an_arn = format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:channelwrong");
    let rows: Vec<(&str, &str, u16, &str, String)> = vec![
        (
            "an identifier naming no channel",
            &absent,
            400,
            "ChannelNotFoundException",
            format!("The channel {absent} was not found."),
        ),
        (
            "an arn: identifier that is not a channel ARN",
            &not_an_arn,
            400,
            "ChannelARNInvalidException",
            format!("The format of the channel ARN {not_an_arn} is not valid."),
        ),
    ];

    for (case, identifier, status, code, message) in rows {
        let reply = emulator
            .call_in("DeleteChannel", HOME_REGION, &json!({"Channel": identifier}))
            .await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {reply:?}"
        );
        assert_eq!(
            reply.body,
            json!({"__type": code, "Message": message}),
            "case {case:?}: expected exactly __type {code} and Message {message:?}, \
             got {reply:?}"
        );
    }
}
