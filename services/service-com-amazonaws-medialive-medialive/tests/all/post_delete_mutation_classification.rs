// SPDX-License-Identifier: CC0-1.0

//! How a MediaLive mutation sent AFTER a delete is classified, end to end over
//! the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! Two rules, one per resource, and the point of both is that a client can tell
//! "this resource is gone, recreate it" from "this resource is busy, retry":
//!
//! 1. CHANNEL. An operation that changes a channel resolves its `ChannelId`
//!    FIRST, before it looks at any channel's state: an id the account holds no
//!    channel for and an id whose delete this service has accepted are the same
//!    refusal, the operation's modelled 404, and the state conflict (409)
//!    belongs only to a channel that still exists. The issue's recorded case
//!    (`StartChannel` on a channel driven to the terminal `DELETED`) is ONE row:
//!    the table spans the channel mutations and both phases of an accepted
//!    delete.
//!
//! 2. INPUT SECURITY GROUP. Its delete is a SOFT delete: the group stays
//!    addressable through `DescribeInputSecurityGroup` in the terminal `DELETED`
//!    state and stays MUTABLE, so `UpdateInputSecurityGroup` on the tombstone
//!    replaces its whitelist and answers 200, and the only thing the delete
//!    takes away is the group's place in `ListInputSecurityGroups`. The one
//!    refusal that resolution has left — an id this account never held — is the
//!    group suite's own table (`input_security_group`:
//!    `an_update_refuses_an_id_no_group_has`), not restated here.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::input_security_group::whitelist;
use crate::support::percent_encoded;
use crate::wire::{call, error_code, error_message, medialive};

/// One operation that CHANGES the channel its `ChannelId` names: the method, the
/// URI built from that id, and the body a client sends.
struct ChannelMutation {
    /// The operation, as the model names it.
    op: &'static str,
    /// Its request: `(method, uri, body)` from the channel id.
    request: fn(&str) -> (&'static str, String, Option<Value>),
    /// The error code this operation's model gives a dead id. Every channel
    /// mutation here models `NotFoundException`; `UpdateChannel` models no 404
    /// at all and answers its `BadRequestException` instead, so that row lives
    /// in the channel update suite (`channel_update`).
    code: &'static str,
    /// The status that code travels with.
    status: u16,
}

/// Every channel mutation this rule covers, with the request each sends.
const CHANNEL_MUTATIONS: &[ChannelMutation] = &[
    ChannelMutation {
        op: "StartChannel",
        request: |id| ("POST", format!("/prod/channels/{id}/start"), None),
        code: "NotFoundException",
        status: 404,
    },
    ChannelMutation {
        op: "StopChannel",
        request: |id| ("POST", format!("/prod/channels/{id}/stop"), None),
        code: "NotFoundException",
        status: 404,
    },
    ChannelMutation {
        op: "DeleteChannel",
        request: |id| ("DELETE", format!("/prod/channels/{id}"), None),
        code: "NotFoundException",
        status: 404,
    },
    ChannelMutation {
        op: "RestartChannelPipelines",
        request: |id| {
            (
                "POST",
                format!("/prod/channels/{id}/restartChannelPipelines"),
                None,
            )
        },
        code: "NotFoundException",
        status: 404,
    },
    ChannelMutation {
        op: "UpdateChannelClass",
        request: |id| {
            (
                "PUT",
                format!("/prod/channels/{id}/channelClass"),
                Some(json!({ "channelClass": "SINGLE_PIPELINE" })),
            )
        },
        code: "NotFoundException",
        status: 404,
    },
    ChannelMutation {
        op: "DeleteSchedule",
        request: |id| ("DELETE", format!("/prod/channels/{id}/schedule"), None),
        code: "NotFoundException",
        status: 404,
    },
];

/// Rule: every channel mutation resolves its `ChannelId` before it looks at the
/// channel's state, so an id whose delete this service has accepted is answered
/// with the operation's modelled not-found error — never with the state conflict
/// a channel that still exists gets — and its message says the channel was
/// already deleted, while an id the account never held says it was not found.
///
/// The rows span what the rule generalises over: the OPERATION doing the
/// mutating, and the PHASE the accepted delete is in — the transient `DELETING`
/// the clock still owes the terminal state, the terminal `DELETED` the record
/// then keeps (the issue's recorded row), and an id no channel ever had, which
/// is the same refusal with the other message.
#[tokio::test]
async fn a_channel_mutation_after_the_delete_is_not_found_rather_than_a_conflict() {
    // (case, the phase the channel is in, whether the account holds its record)
    let phases: [(&str, &str, bool); 3] = [
        ("while DELETING", "DELETING", true),
        ("once DELETED", "DELETED", true),
        ("an id no channel ever had", "absent", false),
    ];

    for (case, phase, recorded) in phases {
        for mutation in CHANNEL_MUTATIONS {
            let router = medialive();
            let id = if recorded {
                let id = idle_channel(&router, case).await;
                let (status, deleting) =
                    call(&router, "DELETE", &format!("/prod/channels/{id}"), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}/{}: DeleteChannel: expected 200, got {status}: {deleting}",
                    mutation.op
                );
                if phase == "DELETED" {
                    wait_for_the_clock();
                }
                // The phase the mutation under test is sent in, read back out of
                // the store through the reader a deleted record stays visible in.
                let state = described_state(&router, case, &id).await;
                assert_eq!(
                    state, phase,
                    "case {case:?}/{}: the test did not reach {phase}, it reached {state:?}",
                    mutation.op
                );
                id
            } else {
                // Numeric and never minted: this store holds no channel for it.
                "9999999".to_string()
            };

            let (method, uri, body) = (mutation.request)(&id);
            let (status, refusal) = call(&router, method, &uri, body).await;
            assert_eq!(
                status, mutation.status,
                "case {case:?}/{}: expected {}, got {status}: {refusal}",
                mutation.op, mutation.status
            );
            assert_eq!(
                error_code(status, &refusal),
                mutation.code,
                "case {case:?}/{}: expected {}, got {refusal}",
                mutation.op,
                mutation.code
            );
            let expected = if recorded {
                format!("Channel {id} already deleted")
            } else {
                format!("Channel {id} not found")
            };
            assert_eq!(
                error_message(&refusal),
                expected,
                "case {case:?}/{}: expected the message {expected:?}, got {:?}",
                mutation.op,
                error_message(&refusal)
            );
        }
    }
}

/// Rule: the state conflict (409 `ConflictException`) is reserved for a channel
/// that STILL EXISTS — the same mutation that is answered 404 on a tombstone is
/// answered 409 on a live channel whose state does not allow it — so the two
/// answers mean different things to a retrying client.
///
/// The rows are the live refusals of the same operations the table above sends
/// at a tombstone: a start of a channel that is already running, a delete of one
/// that is running, and a stop of one that is idle.
#[tokio::test]
async fn the_same_mutation_on_a_live_channel_is_still_a_conflict() {
    // (case, method, uri suffix, whether the channel must be running first)
    let table: [(&str, &str, &str, bool); 4] = [
        ("start a RUNNING channel", "POST", "/start", true),
        ("delete a RUNNING channel", "DELETE", "", true),
        ("stop an IDLE channel", "POST", "/stop", false),
        ("restart the pipelines of an IDLE channel", "POST", "/restartChannelPipelines", false),
    ];

    for (case, method, suffix, running) in table {
        let router = medialive();
        let id = idle_channel(&router, case).await;
        if running {
            let (status, started) =
                call(&router, "POST", &format!("/prod/channels/{id}/start"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: StartChannel: expected 200, got {status}: {started}"
            );
            wait_for_the_clock();
        }

        let (status, refusal) =
            call(&router, method, &format!("/prod/channels/{id}{suffix}"), None).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409 on a channel that still exists, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refusal}"
        );
    }
}

/// Rule: the batch forms report the same classification per entry as the
/// single-resource operation they batch, so a channel whose delete has been
/// accepted is reported with the NOT-FOUND code and the already-deleted message
/// rather than with a state conflict, and the entry carries the record's ARN.
///
/// The rows span the three batch channel operations.
#[tokio::test]
async fn a_batch_entry_for_a_deleted_channel_reports_not_found() {
    // (case, the batch URI; the channel ids travel in `channelIds` in each)
    let table: [(&str, &str); 3] = [
        ("BatchStart", "/prod/batch/start"),
        ("BatchStop", "/prod/batch/stop"),
        ("BatchDelete", "/prod/batch/delete"),
    ];

    for (case, uri) in table {
        let router = medialive();
        let id = idle_channel(&router, case).await;
        let (status, created) = call(&router, "GET", &format!("/prod/channels/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeChannel: expected 200, got {status}: {created}"
        );
        let arn = created["arn"].clone();

        let (status, deleting) = call(&router, "DELETE", &format!("/prod/channels/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteChannel: expected 200, got {status}: {deleting}"
        );
        wait_for_the_clock();

        let (status, batched) =
            call(&router, "POST", uri, Some(json!({ "channelIds": [id.clone()] }))).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the batch itself to succeed, got {status}: {batched}"
        );
        assert_eq!(
            batched["successful"],
            json!([]),
            "case {case:?}: expected no accepted entry, got {batched}"
        );
        let failed = batched["failed"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: expected exactly one failed entry, got {batched}"
        );
        assert_eq!(
            failed[0],
            json!({
                "id": id,
                "arn": arn,
                "code": "NotFoundException",
                "message": format!("Channel {id} already deleted"),
            }),
            "case {case:?}: expected the entry to report the id, the ARN, the not-found code and \
             the already-deleted message, got {:?}",
            failed[0]
        );
    }
}

/// Rule: an input security group's delete is a SOFT delete — the group stays
/// addressable in the terminal `DELETED` state — so `UpdateInputSecurityGroup`
/// on that tombstone succeeds (200), REPLACES its whitelist rules rather than
/// merging them, and the following `DescribeInputSecurityGroup` reports exactly
/// the rules the update carried while the group stays `DELETED`.
///
/// The rows differ along what the rule generalises over: how many rules the
/// update carries (none at all, one, several), their prefix lengths — including
/// both published IPv4 bounds, /0 and /32, which `InputWhitelistRuleCidr$Cidr`
/// ("The IPv4 CIDR to whitelist") admits — and their order, which the group
/// keeps.
#[tokio::test]
async fn a_deleted_input_security_group_is_still_mutable() {
    // (case, the CIDRs the update carries, in request order)
    let table: [(&str, &[&str]); 5] = [
        ("one replacing cidr", &["192.0.2.0/24"]),
        ("two cidrs", &["198.51.100.0/24", "203.0.113.128/25"]),
        ("both published prefix bounds", &["0.0.0.0/0", "192.0.2.7/32"]),
        ("the same two, reordered", &["203.0.113.128/25", "198.51.100.0/24"]),
        ("the whitelist cleared", &[]),
    ];

    for (case, cidrs) in table {
        let router = medialive();
        // Created with rules the update must REPLACE, not be merged with.
        let (status, created) = call(
            &router,
            "POST",
            "/prod/inputSecurityGroups",
            Some(json!({
                "whitelistRules": whitelist(&["10.0.0.0/16", "172.16.0.0/12"]),
                "tags": { "team": "video" },
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateInputSecurityGroup: expected 201, got {status}: {created}"
        );
        let group = created["securityGroup"].clone();
        let id = group["id"].as_str().unwrap_or_default().to_string();
        let arn = group["arn"].as_str().unwrap_or_default().to_string();

        let (status, deleted) = call(
            &router,
            "DELETE",
            &format!("/prod/inputSecurityGroups/{id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteInputSecurityGroup: expected 200, got {status}: {deleted}"
        );
        assert_eq!(
            described_group(&router, case, &id).await["state"],
            json!("DELETED"),
            "case {case:?}: expected the soft-deleted group to read back as DELETED"
        );

        // The mutation the hard-delete contract would refuse: it succeeds here,
        // because the group this account still answers reads for is a group it
        // still answers writes for.
        let (status, updated) = call(
            &router,
            "PUT",
            &format!("/prod/inputSecurityGroups/{id}"),
            Some(json!({ "whitelistRules": whitelist(cidrs) })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update of the DELETED group to succeed, got {status}: {updated}"
        );
        assert_eq!(
            updated["securityGroup"]["whitelistRules"],
            whitelist(cidrs),
            "case {case:?}: expected the update to report the rules it applied, got {:?}",
            updated["securityGroup"]["whitelistRules"]
        );

        // Read back through the group's own reader: the rules were REPLACED, in
        // the order the request listed them, and the group is still a tombstone.
        let described = described_group(&router, case, &id).await;
        assert_eq!(
            described["whitelistRules"],
            whitelist(cidrs),
            "case {case:?}: expected the describe to report the replaced rules, got {:?}",
            described["whitelistRules"]
        );
        assert_eq!(
            described["state"],
            json!("DELETED"),
            "case {case:?}: expected the updated group to stay DELETED, got {:?}",
            described["state"]
        );

        // A tag write reaches the same tombstone, for the same reason.
        let (status, tagged) = call(
            &router,
            "POST",
            &format!("/prod/tags/{}", percent_encoded(&arn)),
            Some(json!({ "tags": { "phase": "after" } })),
        )
        .await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected CreateTags on the tombstone to succeed, got {status}: {tagged}"
        );
        assert_eq!(
            described_group(&router, case, &id).await["tags"],
            json!({ "team": "video", "phase": "after" }),
            "case {case:?}: expected the tag written on the tombstone to read back"
        );

        // The one thing the delete did take away: the group's place in the
        // collection. An update does not put it back.
        let (status, listed) = call(&router, "GET", "/prod/inputSecurityGroups", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListInputSecurityGroups: expected 200, got {status}: {listed}"
        );
        let listed_ids: Vec<String> = listed["inputSecurityGroups"]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .map(|group| group["id"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !listed_ids.contains(&id),
            "case {case:?}: expected the deleted group to stay out of the listing, got {listed_ids:?}"
        );
    }
}

/// One channel, created and settled into `IDLE` — the state from which this
/// service accepts a delete.
async fn idle_channel(router: &MultiServiceRouter, case: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": format!("post-delete-{case}") })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateChannel: expected 201, got {status}: {created}"
    );
    let id = created["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty(),
        "case {case:?}: expected the create to report an id, got {created}"
    );
    wait_for_the_clock();
    id
}

/// The `State` `DescribeChannel` reports for a channel right now. A deleted
/// channel's record stays readable, which is what makes the phase assertions
/// above possible.
async fn described_state(router: &MultiServiceRouter, case: &str, id: &str) -> String {
    let (status, described) = call(router, "GET", &format!("/prod/channels/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeChannel({id}): expected 200, got {status}: {described}"
    );
    described["state"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One input security group, read back through `DescribeInputSecurityGroup`.
async fn described_group(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, described) = call(
        router,
        "GET",
        &format!("/prod/inputSecurityGroups/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInputSecurityGroup({id}): expected 200, got {status}: {described}"
    );
    described
}
