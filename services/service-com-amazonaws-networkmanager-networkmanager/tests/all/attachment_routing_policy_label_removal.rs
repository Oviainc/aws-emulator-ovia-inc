// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `RemoveAttachmentRoutingPolicyLabel`, driven over HTTP through
//! the real router (the harness lives in `crate::global_network_lifecycle`, and the label
//! helpers it shares with the operation that APPLIES a label in
//! `crate::attachment_routing_policy_label`).
//!
//! The rule under test: the call takes the routing policy label OFF the attachment it
//! addresses within the core network it addresses, so that attachment holds none afterwards
//! and `ListAttachmentRoutingPolicyAssociations` no longer reports a routing policy
//! association for it, and answers the label it took off beside the two ids it addressed.
//! An attachment carries at most one label, whatever kind of attachment it is and whatever
//! lifecycle state it is in; nothing else about the attachment changes; an attachment that
//! carried no label has nothing to take off, so the call succeeds and reports no label,
//! which also makes a repeat of the call answer that same success; and an id addressing a
//! resource that is not stored — a core network nothing was minted for, an attachment
//! nothing was minted for, or an attachment of ANOTHER core network — is refused as not
//! found and removes nothing.
//!
//! The table below spans that class: rows differing in attachment kind (VPC, Connect,
//! site-to-site VPN), in edge location, in where the label being removed came from (the
//! create or a later `PutAttachmentRoutingPolicyLabel`), and in the label itself — a plain
//! one, the empty one the modelled `length 0..256` allows, the 256-character one at that
//! upper bound, one carrying spaces and non-ASCII (which the modelled `^[\s\S]*$` allows),
//! and no label at all. Nothing reaches into `crate::state`: every label is applied and
//! read back through an operation.

use serde_json::{json, Value};

use crate::attachment_acceptance::core_network_with;
use crate::attachment_routing_policy_label::{
    create_case, put_ok, seeded, stored_label, AttachmentSpec, Kind, ATTACHMENT, CORE_NETWORK,
    LIST_PREFIX, MAX_LABEL_CHARS, PENDING_ACCEPTANCE,
};
use crate::global_network_lifecycle::{Emulator, PAST_SETTLE};
use crate::resource_not_found_message::assert_not_found_answer;

/// The `RemoveAttachmentRoutingPolicyLabel` route
/// (`DELETE /routing-policy-label/core-network/{CoreNetworkId}/attachment/{AttachmentId}`,
/// `smithy_query query="operation" operation="RemoveAttachmentRoutingPolicyLabel"`).
fn remove_uri(core_network: &str, attachment: &str) -> String {
    format!("/routing-policy-label/core-network/{core_network}/attachment/{attachment}")
}

/// `RemoveAttachmentRoutingPolicyLabel`.
async fn remove(emulator: &Emulator, core_network: &str, attachment: &str) -> (u16, Value) {
    emulator.delete(&remove_uri(core_network, attachment)).await
}

/// `RemoveAttachmentRoutingPolicyLabel`, asserting the call succeeded, that its payload
/// reports the two ids it addressed, and that it reports `expected` as the label it took
/// off — absent where the attachment carried none.
async fn remove_ok(
    emulator: &Emulator,
    case: &str,
    core_network: &str,
    attachment: &str,
    expected: Option<&str>,
) {
    let (status, answer) = remove(emulator, core_network, attachment).await;
    assert_eq!(
        status, 200,
        "case {case:?}: RemoveAttachmentRoutingPolicyLabel expected 200, got {status} with {answer}"
    );
    assert_eq!(
        answer["CoreNetworkId"].as_str(),
        Some(core_network),
        "case {case:?}: expected CoreNetworkId {core_network:?}, got {answer}"
    );
    assert_eq!(
        answer["AttachmentId"].as_str(),
        Some(attachment),
        "case {case:?}: expected AttachmentId {attachment:?}, got {answer}"
    );
    assert_eq!(
        answer["RoutingPolicyLabel"].as_str(),
        expected,
        "case {case:?}: expected the removed label {expected:?}, got {answer}"
    );
    assert_eq!(
        stored_label(emulator, case, core_network, attachment).await,
        None,
        "case {case:?}: the attachment must hold no label after the removal"
    );
}

/// One row of the table: an attachment of one kind at one edge location, the label its
/// create carried (or none), the label a `PutAttachmentRoutingPolicyLabel` then applied (or
/// none), and therefore the label the removal has to take off.
struct RemovalCase {
    /// Names the case in every assertion message.
    name: &'static str,
    kind: Kind,
    /// The edge location the attachment sits at, which the core network under test
    /// configures an edge for.
    region: &'static str,
    /// The `RoutingPolicyLabel` the create carried, or `None` for a create that carried
    /// none.
    created: Option<&'static str>,
    /// The `RoutingPolicyLabel` a later put applied, or `None` where no put ran — so the
    /// label present when the removal runs is this one if there is one, else
    /// [`Self::created`].
    applied: Option<String>,
}

impl RemovalCase {
    /// The label stored on the attachment when the removal runs, which is what the removal
    /// has to report and clear.
    fn stored(&self) -> Option<String> {
        self.applied
            .clone()
            .or_else(|| self.created.map(str::to_string))
    }
}

/// The table: rows differing along everything the rule generalises over — the attachment
/// kind, the edge location, where the stored label came from, and the label itself,
/// including the case of no label at all.
fn removal_cases() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            name: "a VPC attachment loses the label its create carried",
            kind: Kind::Vpc,
            region: "us-west-2",
            created: Some("north"),
            applied: None,
        },
        RemovalCase {
            name: "a VPC attachment loses the label a put applied",
            kind: Kind::Vpc,
            region: "us-west-2",
            created: None,
            applied: Some("south".to_string()),
        },
        RemovalCase {
            name: "a VPC attachment loses the empty label the modelled length 0..256 allows",
            kind: Kind::Vpc,
            region: "eu-central-1",
            created: Some("north"),
            applied: Some(String::new()),
        },
        RemovalCase {
            name: "a VPC attachment loses the 256-character label at the modelled bound",
            kind: Kind::Vpc,
            region: "ap-southeast-4",
            created: None,
            applied: Some("l".repeat(MAX_LABEL_CHARS)),
        },
        RemovalCase {
            name: "a VPC attachment loses a label with spaces and non-ASCII",
            kind: Kind::Vpc,
            region: "eu-central-1",
            created: Some("plain"),
            applied: Some("ingress policy \u{2013} \u{3c0}".to_string()),
        },
        RemovalCase {
            name: "a Connect attachment loses the label its create carried",
            kind: Kind::Connect,
            region: "us-west-2",
            created: Some("connect-edge"),
            applied: None,
        },
        RemovalCase {
            name: "a Connect attachment that never carried a label loses nothing",
            kind: Kind::Connect,
            region: "us-west-2",
            created: None,
            applied: None,
        },
        RemovalCase {
            name: "a site-to-site VPN attachment loses the label its create carried",
            kind: Kind::Vpn,
            region: "ap-southeast-4",
            created: Some("vpn-south"),
            applied: None,
        },
        RemovalCase {
            name: "a site-to-site VPN attachment loses the label a put applied",
            kind: Kind::Vpn,
            region: "eu-central-1",
            created: None,
            applied: Some("new".to_string()),
        },
    ]
}

#[tokio::test]
async fn every_removal_reports_the_stored_label_and_leaves_the_attachment_unlabelled() {
    let emulator = Emulator::new();
    let (core_network, transport) = seeded(&emulator, "removals", 0).await;

    let cases = removal_cases();
    let mut removed: Vec<String> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let id = create_case(
            &emulator,
            &core_network,
            index + 1,
            &transport,
            &AttachmentSpec {
                name: case.name,
                kind: case.kind,
                region: case.region,
                label: case.created,
            },
        )
        .await;
        if let Some(applied) = case.applied.as_deref() {
            put_ok(&emulator, case.name, &core_network, &id, applied, None).await;
        }

        // What is stored when the removal runs, so the row's assertions below are about the
        // removal rather than about the create or the put.
        assert_eq!(
            stored_label(&emulator, case.name, &core_network, &id).await,
            case.stored(),
            "case {:?}: the label to be removed must be the stored one",
            case.name
        );

        remove_ok(
            &emulator,
            case.name,
            &core_network,
            &id,
            case.stored().as_deref(),
        )
        .await;

        // The same call again: there is nothing left to take off, so it succeeds reporting
        // no label and the attachment is still unlabelled.
        remove_ok(&emulator, case.name, &core_network, &id, None).await;

        // The removal cleared the label and nothing else, so the attachment can be labelled
        // again and loses that label the same way.
        put_ok(&emulator, case.name, &core_network, &id, "relabelled", None).await;
        assert_eq!(
            stored_label(&emulator, case.name, &core_network, &id).await,
            Some("relabelled".to_string()),
            "case {:?}: an unlabelled attachment must take a label again",
            case.name
        );
        remove_ok(
            &emulator,
            case.name,
            &core_network,
            &id,
            Some("relabelled"),
        )
        .await;

        removed.push(id);
    }

    // Every row at once: no attachment of this core network carries a routing policy label
    // any more, so no call left a label behind on an attachment other than the one it
    // addressed.
    let (status, listed) = emulator.get(&format!("{LIST_PREFIX}/{core_network}")).await;
    assert_eq!(
        status, 200,
        "the whole listing expected 200, got {status} with {listed}"
    );
    assert_eq!(
        listed["AttachmentRoutingPolicyAssociations"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "case {:?}: every label of this core network was removed, got {listed}",
        "the whole listing"
    );
    for id in &removed {
        assert_eq!(
            stored_label(&emulator, id, &core_network, id).await,
            None,
            "case {id:?}: expected no label on an attachment every label was removed from"
        );
    }
    assert_eq!(
        stored_label(&emulator, "the transport", &core_network, &transport).await,
        None,
        "case {:?}: an attachment nothing labelled has no routing policy association",
        "the transport"
    );
}

#[tokio::test]
async fn an_id_addressing_nothing_stored_is_refused_and_removes_nothing() {
    let emulator = Emulator::new();
    let (core_network, transport) = seeded(&emulator, "refusals", 0).await;
    let id = create_case(
        &emulator,
        &core_network,
        1,
        &transport,
        &AttachmentSpec {
            name: "the labelled attachment",
            kind: Kind::Vpc,
            region: "us-west-2",
            label: Some("kept"),
        },
    )
    .await;
    // A second core network, built the same way (one core network per global network), so
    // one of its attachments is a well-formed id that is not an attachment of the first.
    let (other_network, other_transport) = seeded(&emulator, "elsewhere", 100).await;
    let elsewhere = create_case(
        &emulator,
        &other_network,
        101,
        &other_transport,
        &AttachmentSpec {
            name: "an attachment of another core network",
            kind: Kind::Vpc,
            region: "eu-central-1",
            label: Some("theirs"),
        },
    )
    .await;

    // Each row: a request addressing a resource this account holds nothing for, and the
    // resource its refusal has to identify. Both ids are well formed, so what is refused is
    // what they address, never their shape.
    for (case, core_network_id, attachment, resource_id, resource_type) in [
        (
            "a core network id nothing was minted for",
            "core-network-00000000000000099".to_string(),
            id.clone(),
            "core-network-00000000000000099".to_string(),
            CORE_NETWORK,
        ),
        (
            "an attachment id nothing was minted for",
            core_network.clone(),
            "attachment-00000000000000099".to_string(),
            "attachment-00000000000000099".to_string(),
            ATTACHMENT,
        ),
        (
            "an attachment of another core network",
            core_network.clone(),
            elsewhere.clone(),
            elsewhere.clone(),
            ATTACHMENT,
        ),
        (
            "a core network addressed through an attachment of its own, the other way round",
            other_network.clone(),
            id.clone(),
            id.clone(),
            ATTACHMENT,
        ),
    ] {
        assert_not_found_answer(
            &emulator,
            case,
            "DELETE",
            &remove_uri(&core_network_id, &attachment),
            None,
            &resource_id,
            resource_type,
        )
        .await;
    }

    // Nothing the refusals addressed changed: each attachment still carries the label its
    // create stored, in its own core network.
    assert_eq!(
        stored_label(&emulator, "the first attachment", &core_network, &id).await,
        Some("kept".to_string()),
        "case {:?}: a refused call must remove nothing",
        "the first attachment"
    );
    assert_eq!(
        stored_label(&emulator, "the other attachment", &other_network, &elsewhere).await,
        Some("theirs".to_string()),
        "case {:?}: a refused call must remove nothing",
        "the other attachment"
    );

    // A deleted attachment is an id that names nothing again, so unlabelling it is the same
    // refusal.
    let (status, deleted) = emulator.delete(&format!("/attachments/{id}")).await;
    assert_eq!(
        status, 200,
        "DeleteAttachment expected 200, got {status} with {deleted}"
    );
    assert_not_found_answer(
        &emulator,
        "a deleted attachment",
        "DELETE",
        &remove_uri(&core_network, &id),
        None,
        &id,
        ATTACHMENT,
    )
    .await;
}

#[tokio::test]
async fn removing_a_label_changes_nothing_else_about_the_attachment() {
    let emulator = Emulator::new();
    let (core_network, transport) = seeded(&emulator, "untouched", 0).await;
    let case = "the attachment itself";
    let id = create_case(
        &emulator,
        &core_network,
        1,
        &transport,
        &AttachmentSpec {
            name: case,
            kind: Kind::Vpc,
            region: "us-west-2",
            label: Some("edge"),
        },
    )
    .await;

    // Past the span the create owes, so the attachment has settled and its own read is
    // stable: what follows cannot be the clock's doing.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, before) = emulator.get(&format!("/vpc-attachments/{id}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetVpcAttachment expected 200, got {status} with {before}"
    );

    remove_ok(&emulator, case, &core_network, &id, Some("edge")).await;

    let (status, after) = emulator.get(&format!("/vpc-attachments/{id}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetVpcAttachment expected 200, got {status} with {after}"
    );
    assert_eq!(
        after, before,
        "case {case:?}: the attachment's own read must be unchanged by a removed label, got {after}"
    );
}

#[tokio::test]
async fn an_attachment_waiting_for_its_acceptance_can_be_unlabelled() {
    let emulator = Emulator::new();
    let case = "an attachment awaiting the acceptance its policy asked for";
    let region = "us-west-2";
    // The published Cloud WAN policy schema's own `attachment-policies` spelling, as
    // `crate::attachment_acceptance` builds it: every attachment is claimed, and the
    // claiming rule asks for acceptance.
    let rules = json!([{
        "rule-number": 100,
        "conditions": [{ "type": "any" }],
        "action": {
            "association-method": "constant",
            "segment": "shared",
            "require-attachment-acceptance": true,
        },
    }]);
    let core_network = core_network_with(&emulator, case, region, &rules).await;
    let id = create_case(
        &emulator,
        &core_network,
        1,
        "",
        &AttachmentSpec {
            name: case,
            kind: Kind::Vpc,
            region,
            label: Some("waiting"),
        },
    )
    .await;

    let (status, read) = emulator.get(&format!("/vpc-attachments/{id}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetVpcAttachment expected 200, got {status} with {read}"
    );
    assert_eq!(
        read["VpcAttachment"]["Attachment"]["State"].as_str(),
        Some(PENDING_ACCEPTANCE),
        "case {case:?}: the row needs an attachment waiting for an acceptance, got {read}"
    );

    remove_ok(&emulator, case, &core_network, &id, Some("waiting")).await;

    // Removing the label is not an acceptance and owes no transition: past the span a
    // started attachment settles in, this one is exactly where it was.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, settled) = emulator.get(&format!("/vpc-attachments/{id}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetVpcAttachment expected 200, got {status} with {settled}"
    );
    assert_eq!(
        settled["VpcAttachment"]["Attachment"]["State"].as_str(),
        Some(PENDING_ACCEPTANCE),
        "case {case:?}: unlabelling must not start an attachment, got {settled}"
    );
}
