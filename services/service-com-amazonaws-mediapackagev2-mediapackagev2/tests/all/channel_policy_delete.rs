// SPDX-License-Identifier: CC0-1.0

//! `DeleteChannelPolicy`: the policy it detaches is the addressed channel's and no other.
//!
//! The broad channel-policy table — what a put accepts, what a read reports, the delete's
//! idempotence and the channel record's immutability — is `tests/all/channel_policy.rs`,
//! whose RULE 3 pins that ATTACHING a policy touches one address only. This suite pins
//! the same property for the DETACH, which that rule does not exercise: a delete removes
//! the policy of the addressed (channel group, channel) pair and leaves every other
//! address's policy exactly as it was, including the namesakes that collide on one
//! component.
//!
//! Both the emulator and the error-envelope reader are the crate's single client, from
//! `tests/all/channel_lifecycle.rs`.

use serde_json::{json, Value};

use crate::channel_lifecycle::{error_type, Emulator};

/// The policy sub-resource of one channel, addressed as the model binds it:
/// `/channelGroup/{ChannelGroupName}/channel/{ChannelName}/policy`.
fn policy_path(group: &str, channel: &str) -> String {
    format!("/channelGroup/{group}/channel/{channel}/policy")
}

/// A policy document naming the address it is attached to, so a read that reports the
/// wrong channel's policy is visible in the assertion rather than merely unequal.
fn policy_for(group: &str, channel: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Sid": format!("{group}-{channel}"), "Effect": "Allow", "Action": "*"}]
    })
    .to_string()
}

/// A delete detaches one address's policy and no other's.
#[tokio::test]
async fn a_delete_detaches_only_the_addressed_channels_policy() {
    // The same channel name in two groups, plus a sibling in the first: three distinct
    // addresses, two of which collide with the deleted one on one component each.
    let addresses = [
        ("group-a", "channel-1"),
        ("group-a", "channel-2"),
        ("group-b", "channel-1"),
    ];

    // Each row deletes a different one of them, so every address is the deleted one once
    // and a bystander twice.
    for &(deleted_group, deleted_channel) in &addresses {
        let case = format!("deleting {deleted_group}/{deleted_channel}");
        let emulator = Emulator::new();
        for group in ["group-a", "group-b"] {
            let (status, body) = emulator.create_group(group, json!({})).await;
            assert_eq!(status, 200, "case {case:?}: creating {group} got {body}");
        }
        for (group, channel) in addresses {
            let (status, body) = emulator.create_channel(group, channel, json!({})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: creating {group}/{channel} got {body}"
            );
            let (status, body) = emulator
                .call(
                    "PUT",
                    &policy_path(group, channel),
                    Some(json!({"Policy": policy_for(group, channel)})),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: attaching the policy of {group}/{channel} got {body}"
            );
        }

        let (status, body) = emulator
            .call("DELETE", &policy_path(deleted_group, deleted_channel), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the delete, got {status} {body}"
        );

        for (group, channel) in addresses {
            let (status, body) = emulator
                .call("GET", &policy_path(group, channel), None)
                .await;
            if (group, channel) == (deleted_group, deleted_channel) {
                assert_eq!(
                    status, 404,
                    "case {case:?}: the deleted policy must be gone, got {status} {body}"
                );
                assert_eq!(
                    error_type(&body),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {body}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: {group}/{channel} must keep its policy, got {status} {body}"
                );
                assert_eq!(
                    body.get("Policy").and_then(Value::as_str),
                    Some(policy_for(group, channel).as_str()),
                    "case {case:?}: {group}/{channel} must report its OWN policy, got {body}"
                );
                assert_eq!(
                    body.get("ChannelName").and_then(Value::as_str),
                    Some(channel),
                    "case {case:?}: the read must name the address it was asked for, got {body}"
                );
            }
        }
    }
}
