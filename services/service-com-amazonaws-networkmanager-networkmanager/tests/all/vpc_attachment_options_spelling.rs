// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of how the VPC-attachment operations read an `Options` set, driven
//! over HTTP through the real router (the harness lives in
//! `crate::global_network_lifecycle`; the create, the update and the reader are covered in
//! their own right by `crate::attachment_lifecycle` and
//! `crate::vpc_attachment_lifecycle`).
//!
//! The rule under test: every member of `VpcOptions` carries a modelled `@default` of
//! `false` (`smithy_query shape=VpcOptions`), so an `Options` set is the four flags it asks
//! for — the default for every member it leaves out — and an absent set asks for all four
//! defaults; two spellings asking for the same four flags are therefore the SAME request,
//! whichever of them the request and the stored attachment use.
//!
//! The tables below span that class: the spellings differ in which side is absent, in the
//! empty object against every member written out, in a partial set naming one member only,
//! and in whether the flags asked for are the defaults or not; and each is driven through
//! both operations that compare an options set — the update's no-change rule and the
//! create's `ClientToken` replay. Nothing reaches into `crate::state`: every attachment is
//! read back through `GetVpcAttachment`.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{attachment_id, core_network_at, subnet_arn, vpc_arn};
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::vpc_attachment_lifecycle::{options_body, options_of, subnets_of};

/// The Region the core networks here configure an edge location in, and so the Region the
/// attached VPCs and their subnets live in.
const REGION: &str = "us-west-2";

/// One spelling of an `Options` set: what goes on the wire (`None` is a request carrying no
/// `Options` member at all), and the four flags it asks for.
struct Spelling {
    /// Names the spelling in every assertion message.
    name: &'static str,
    /// The `Options` member, or `None` for a request that carries none.
    body: Option<Value>,
    /// The flags it asks for, in the order `options_of` reports them.
    flags: [bool; 4],
}

/// The empty object: an `Options` set naming no member, so every flag is its modelled
/// default.
fn empty_options() -> Value {
    json!({})
}

/// Pairs of spellings that ask for the SAME four flags. One row per way two spellings of one
/// request can differ.
fn equivalent_pairs() -> Vec<(&'static str, Spelling, Spelling)> {
    vec![
        (
            "absent against the empty object",
            Spelling {
                name: "no Options member",
                body: None,
                flags: [false, false, false, false],
            },
            Spelling {
                name: "the empty object",
                body: Some(empty_options()),
                flags: [false, false, false, false],
            },
        ),
        (
            "absent against every default written out",
            Spelling {
                name: "no Options member",
                body: None,
                flags: [false, false, false, false],
            },
            Spelling {
                name: "all four flags false",
                body: Some(options_body([false, false, false, false])),
                flags: [false, false, false, false],
            },
        ),
        (
            "every default written out against absent",
            Spelling {
                name: "all four flags false",
                body: Some(options_body([false, false, false, false])),
                flags: [false, false, false, false],
            },
            Spelling {
                name: "no Options member",
                body: None,
                flags: [false, false, false, false],
            },
        ),
        (
            "absent against one member naming its default",
            Spelling {
                name: "no Options member",
                body: None,
                flags: [false, false, false, false],
            },
            Spelling {
                name: "DnsSupport false alone",
                body: Some(json!({ "DnsSupport": false })),
                flags: [false, false, false, false],
            },
        ),
        (
            "flags on, written out against the members that are on",
            Spelling {
                name: "Ipv6Support and DnsSupport on",
                body: Some(options_body([true, false, true, false])),
                flags: [true, false, true, false],
            },
            Spelling {
                name: "only the two on members named",
                body: Some(json!({ "Ipv6Support": true, "DnsSupport": true })),
                flags: [true, false, true, false],
            },
        ),
        (
            "every flag on, written out twice over",
            Spelling {
                name: "all four flags true",
                body: Some(options_body([true, true, true, true])),
                flags: [true, true, true, true],
            },
            Spelling {
                name: "all four flags true again",
                body: Some(options_body([true, true, true, true])),
                flags: [true, true, true, true],
            },
        ),
    ]
}

/// A `CreateVpcAttachment` request attaching the VPC `index` at [`REGION`] with `subnets`
/// many subnets out of that index's block, carrying `options`' spelling.
fn create_request(core_network: &str, index: usize, subnets: usize, options: &Spelling) -> Value {
    let mut request = json!({
        "CoreNetworkId": core_network,
        "VpcArn": vpc_arn(REGION, index),
        "SubnetArns": (0..subnets)
            .map(|offset| subnet_arn(REGION, index * 10 + offset))
            .collect::<Vec<String>>(),
    });
    if let Some(body) = &options.body {
        request["Options"] = body.clone();
    }
    request
}

/// The `Options` member of a spelling, placed on an otherwise empty update request.
fn update_request(options: &Spelling) -> Value {
    let mut request = json!({});
    if let Some(body) = &options.body {
        request["Options"] = body.clone();
    }
    request
}

#[tokio::test]
async fn an_update_whose_options_spell_the_flags_the_attachment_has_changes_nothing() {
    let emulator = Emulator::new();
    let pairs = equivalent_pairs();
    // Each entry: the row, the attachment created under the row's stored spelling, and the
    // VPC index it attaches.
    let mut created: Vec<(usize, String)> = Vec::new();

    for (row, (case, stored, asked)) in pairs.iter().enumerate() {
        let index = row + 1;
        let core_network = core_network_in_region(&emulator, case).await;
        let (status, body) = emulator
            .post(
                "/vpc-attachments",
                create_request(&core_network, index, 2, stored),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateVpcAttachment with {:?} expected 200, got {status} with {body}",
            stored.name
        );
        let id = attachment_id(case, &body["VpcAttachment"]);
        assert_eq!(
            options_of(&body["VpcAttachment"]),
            stored.flags,
            "case {case:?}: the create must store the flags {:?} asks for, got {body}",
            stored.name
        );
        // The spellings are only read as equivalent, never confused: the asked-for set has
        // to name the same flags the stored one does.
        assert_eq!(
            asked.flags, stored.flags,
            "case {case:?}: the table's two spellings must ask for the same flags"
        );
        created.push((index, id));
    }

    tokio::time::sleep(PAST_SETTLE).await;

    for (row, (case, stored, asked)) in pairs.iter().enumerate() {
        let (index, id) = &created[row];
        let (status, settled) = emulator.get(&format!("/vpc-attachments/{id}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetVpcAttachment expected 200, got {status} with {settled}"
        );
        assert_eq!(
            settled["VpcAttachment"]["Attachment"]["State"],
            json!("AVAILABLE"),
            "case {case:?}: past the settle span the attachment is AVAILABLE, got {settled}"
        );

        // The update asks for the flags the attachment already has, spelled the other way:
        // there is nothing to apply, so the attachment is not moved to UPDATING, `UpdatedAt`
        // does not advance and the options it reports keep the spelling it was stored with.
        let (status, updated) = emulator
            .patch(&format!("/vpc-attachments/{id}"), update_request(asked))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateVpcAttachment with {:?} expected 200, got {status} with {updated}",
            asked.name
        );
        assert_eq!(
            updated["VpcAttachment"], settled["VpcAttachment"],
            "case {case:?}: {:?} asks for the flags {:?} already holds, so the answer must be the settled attachment, got {updated}",
            asked.name, stored.name
        );
        let (_, read) = emulator.get(&format!("/vpc-attachments/{id}")).await;
        assert_eq!(
            read["VpcAttachment"], settled["VpcAttachment"],
            "case {case:?}: an update that changes nothing must store nothing, got {read}"
        );

        // The control: a set asking for OTHER flags is a change, so the very same update
        // path does move the attachment. Without it every row above would pass on a handler
        // that applies nothing at all.
        let inverted = stored.flags.map(|flag| !flag);
        let (status, changed) = emulator
            .patch(
                &format!("/vpc-attachments/{id}"),
                json!({ "Options": options_body(inverted) }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an update to the inverted flags expected 200, got {status} with {changed}"
        );
        assert_eq!(
            options_of(&changed["VpcAttachment"]),
            inverted,
            "case {case:?}: expected the flags {inverted:?} the update asks for, got {changed}"
        );
        assert_eq!(
            changed["VpcAttachment"]["Attachment"]["State"],
            json!("UPDATING"),
            "case {case:?}: a change to other flags leaves the attachment UPDATING, got {changed}"
        );
        let created_at = settled["VpcAttachment"]["Attachment"]["CreatedAt"].as_f64();
        assert!(
            changed["VpcAttachment"]["Attachment"]["UpdatedAt"]
                .as_f64()
                .zip(created_at)
                .is_some_and(|(updated_at, created_at)| updated_at > created_at),
            "case {case:?}: a change advances UpdatedAt past CreatedAt, got {changed}"
        );
        // The attachment the control changed is left out of the rest of the suite; the
        // subnets it holds are the ones the create gave it, untouched by either call.
        assert_eq!(
            subnets_of(&changed["VpcAttachment"]),
            vec![
                subnet_arn(REGION, index * 10),
                subnet_arn(REGION, index * 10 + 1)
            ],
            "case {case:?}: an options-only update must not touch the subnets, got {changed}"
        );
    }
}

#[tokio::test]
async fn an_applied_change_keeps_the_options_the_request_asks_for_as_stored() {
    let emulator = Emulator::new();

    // Each row: the spelling the attachment is created with, and the equivalent spelling the
    // subnet-changing update carries. The update applies its SUBNET change, and the options
    // it names ask for the flags already held, so the attachment reports the same options
    // before and after.
    for (row, (case, stored, asked)) in equivalent_pairs().into_iter().enumerate() {
        let index = 21 + row;
        let core_network = core_network_in_region(&emulator, case).await;
        let (status, body) = emulator
            .post(
                "/vpc-attachments",
                create_request(&core_network, index, 1, &stored),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateVpcAttachment expected 200, got {status} with {body}"
        );
        let id = attachment_id(case, &body["VpcAttachment"]);
        tokio::time::sleep(PAST_SETTLE).await;
        let (_, settled) = emulator.get(&format!("/vpc-attachments/{id}")).await;

        let added = subnet_arn(REGION, index * 10 + 1);
        let mut request = update_request(&asked);
        request["AddSubnetArns"] = json!([added]);
        let (status, updated) = emulator
            .patch(&format!("/vpc-attachments/{id}"), request)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateVpcAttachment expected 200, got {status} with {updated}"
        );
        assert_eq!(
            subnets_of(&updated["VpcAttachment"]),
            vec![subnet_arn(REGION, index * 10), added],
            "case {case:?}: the update must add the subnet it names, got {updated}"
        );
        assert_eq!(
            updated["VpcAttachment"]["Options"], settled["VpcAttachment"]["Options"],
            "case {case:?}: {:?} asks for the flags {:?} already holds, so the reported options are untouched, got {updated}",
            asked.name, stored.name
        );
        assert_eq!(
            options_of(&updated["VpcAttachment"]),
            stored.flags,
            "case {case:?}: expected the flags the attachment already had, got {updated}"
        );
    }
}

#[tokio::test]
async fn a_retry_whose_options_spell_the_same_flags_replays_the_attachment() {
    let emulator = Emulator::new();

    for (row, (case, first, retried)) in equivalent_pairs().into_iter().enumerate() {
        let index = 41 + row;
        let core_network = core_network_in_region(&emulator, case).await;
        let token = format!("token-{row}");
        let with_token = |options: &Spelling| {
            let mut request = create_request(&core_network, index, 2, options);
            request["ClientToken"] = json!(token);
            request
        };

        let (status, created) = emulator.post("/vpc-attachments", with_token(&first)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateVpcAttachment expected 200, got {status} with {created}"
        );
        let id = attachment_id(case, &created["VpcAttachment"]);

        // The retry asks for the same four flags, spelled the other way, so it is the same
        // request: the attachment the first call minted is answered.
        let (status, replayed) = emulator
            .post("/vpc-attachments", with_token(&retried))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a retry spelling the same flags with {:?} expected 200, got {status} with {replayed}",
            retried.name
        );
        assert_eq!(
            replayed["VpcAttachment"]["Attachment"]["AttachmentId"],
            json!(id),
            "case {case:?}: the retry must replay attachment {id}, got {replayed}"
        );
        assert_eq!(
            options_of(&replayed["VpcAttachment"]),
            first.flags,
            "case {case:?}: the replay answers the stored attachment's flags, got {replayed}"
        );

        // The control: the same token asking for OTHER flags is a different request, which
        // is the modelled conflict naming the attachment the token already created.
        let inverted = first.flags.map(|flag| !flag);
        let different = Spelling {
            name: "the inverted flags",
            body: Some(options_body(inverted)),
            flags: inverted,
        };
        let (status, refused) = emulator
            .post("/vpc-attachments", with_token(&different))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: the same token asking for other flags expected 409, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused}"
        );
        let expected = format!(
            "client token {token:?} already created attachment {id}, with a different request"
        );
        // `ConflictException` carries its message in `Message`.
        assert_eq!(
            refused["Message"],
            json!(expected),
            "case {case:?}: expected the refusal to read {expected:?}, got {refused}"
        );
    }
}

/// A core network, in its own global network, whose policy configures one edge location in
/// [`REGION`]; see `crate::attachment_lifecycle::core_network_at`, which owns the setup. The
/// document carries no `attachment-policies`, so no rule claims the attachments created on
/// it and none of them waits for an acceptance.
async fn core_network_in_region(emulator: &Emulator, name: &str) -> String {
    let (_global_network, core_network) = core_network_at(emulator, name, &[REGION]).await;
    core_network
}
