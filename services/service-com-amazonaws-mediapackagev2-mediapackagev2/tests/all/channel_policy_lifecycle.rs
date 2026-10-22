// SPDX-License-Identifier: CC0-1.0

//! `DeleteChannelPolicy` against an address that does not resolve: it reports success and
//! materialises nothing.
//!
//! `tests/all/channel_policy.rs` RULE 8 pins that the delete answers 200 for an unknown
//! channel or channel group — the operation is `@idempotent` and its model declares no
//! `ResourceNotFoundException`, so "there is no policy here" is the state it promises.
//! This suite pins the other half of that rule, which no other suite asserts: answering
//! success must not CREATE the address it was handed. A delete is a mutating call, so the
//! channel group, the channel and the policy it did not find must still be absent
//! afterwards, and a channel that was there must still have no policy.
//!
//! The emulator and the error-envelope reader are the crate's single client, from
//! `tests/all/channel_lifecycle.rs`.

use serde_json::json;

use crate::channel_lifecycle::{error_type, Emulator};

/// A policy document that is accepted by the put, so the "channel exists" row can be
/// distinguished from a row whose policy was never attachable.
const POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*"}]}"#;

/// The delete answers 200 for every unresolvable address and creates nothing.
#[tokio::test]
async fn a_delete_against_an_unresolvable_address_creates_nothing() {
    // label, create the group, create the channel, group addressed, channel addressed.
    // The rows walk the chain: both levels present, only the group present, neither.
    let rows: &[(&str, bool, bool, &str, &str)] = &[
        ("the channel exists, with no policy", true, true, "grp", "chan"),
        ("the channel is absent", true, false, "grp", "ghost-channel"),
        ("the channel group is absent", false, false, "ghost-group", "chan"),
    ];

    for &(label, make_group, make_channel, group, channel) in rows {
        let emulator = Emulator::new();
        if make_group {
            let (status, body) = emulator.create_group("grp", json!({})).await;
            assert_eq!(status, 200, "case {label:?}: creating the group got {body}");
        }
        if make_channel {
            let (status, body) = emulator.create_channel("grp", "chan", json!({})).await;
            assert_eq!(status, 200, "case {label:?}: creating the channel got {body}");
        }

        let path = format!("/channelGroup/{group}/channel/{channel}/policy");
        let (status, body) = emulator.call("DELETE", &path, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from the delete, got {status} {body}"
        );

        // Nothing the delete did not find may exist now: not the group, not the channel,
        // not a policy.
        let (status, body) = emulator
            .call("GET", &format!("/channelGroup/{group}"), None)
            .await;
        let expected = if make_group { 200 } else { 404 };
        assert_eq!(
            status, expected,
            "case {label:?}: the delete must not change whether the channel group exists, \
             expected {expected}, got {status} {body}"
        );
        let (status, body) = emulator.get_channel(group, channel).await;
        let expected = if make_channel { 200 } else { 404 };
        assert_eq!(
            status, expected,
            "case {label:?}: the delete must not change whether the channel exists, \
             expected {expected}, got {status} {body}"
        );
        let (status, body) = emulator.call("GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {label:?}: no policy may exist after the delete, got {status} {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );

        // And the address the delete answered for is still usable: where the channel is
        // there, a policy can be attached and read afterwards.
        if make_channel {
            let (status, body) = emulator
                .call("PUT", &path, Some(json!({"Policy": POLICY})))
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: attaching after the delete got {status} {body}"
            );
            let (status, body) = emulator.call("GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case {label:?}: the attached policy must read back, got {status} {body}"
            );
        }
    }
}
