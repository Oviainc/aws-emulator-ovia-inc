// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `PutAttachmentRoutingPolicyLabel`, driven over HTTP through the
//! real router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: the call STORES the label it carries on the attachment it addresses
//! within the core network it addresses — replacing whatever label that attachment held,
//! from its create or from an earlier call — and answers that stored label beside the two
//! ids it was stored under, so `ListAttachmentRoutingPolicyAssociations` then reports it as
//! the attachment's routing policy association. An attachment carries exactly one label,
//! whatever kind of attachment it is and whatever lifecycle state it is in; nothing else
//! about the attachment changes; and an id addressing a resource that is not stored — a core
//! network nothing was minted for, an attachment nothing was minted for, or an attachment
//! of ANOTHER core network — is refused as not found and stores nothing.
//!
//! The table below spans that class: rows differing in attachment kind (VPC, Connect,
//! site-to-site VPN), in edge location, in whether the attachment already carried a label
//! from its create, and in the label applied — a plain one, the empty one the modelled
//! `length 0..256` allows, the 256-character one at that upper bound, one carrying spaces
//! and non-ASCII (which the modelled `^[\s\S]*$` allows), and one that repeats the label
//! already stored. A label one character past the bound is refused by that constraint.
//! Nothing reaches into `crate::state`: every label is applied and read back through an
//! operation.

use serde_json::{json, Value};

use crate::attachment_acceptance::core_network_with;
use crate::attachment_lifecycle::{
    attachment_id, core_network_at, create_connect_attachment, create_vpc_attachment, subnet_arn,
    vpc_arn,
};
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::resource_not_found_message::assert_not_found_answer;

/// The `PutAttachmentRoutingPolicyLabel` route (`POST /routing-policy-label`,
/// `smithy_query query="operation" operation="PutAttachmentRoutingPolicyLabel"`).
const PUT_URI: &str = "/routing-policy-label";

/// The `ListAttachmentRoutingPolicyAssociations` route prefix
/// (`GET /routing-policy-label/core-network/{CoreNetworkId}`), which is how a stored label
/// is read back.
const LIST_PREFIX: &str = "/routing-policy-label/core-network";

/// The upper bound the model puts on a routing policy label: `ConstrainedString`, which
/// `PutAttachmentRoutingPolicyLabelRequest$RoutingPolicyLabel` targets, is `length 0..256`
/// (`smithy_query shape=ConstrainedString`).
const MAX_LABEL_CHARS: usize = 256;

/// The ARN resource types a not-found refusal of this call reports, which are this
/// service's own ARN resource types for the two resources the request addresses.
const CORE_NETWORK: &str = "core-network";
/// See [`CORE_NETWORK`].
const ATTACHMENT: &str = "attachment";

/// The state an attachment claimed by a rule asking for acceptance waits in
/// (`smithy_query shape=AttachmentState`).
const PENDING_ACCEPTANCE: &str = "PENDING_ATTACHMENT_ACCEPTANCE";

/// The edge locations the core network of the table configures, so every row sits at one
/// of them.
const REGIONS: [&str; 3] = ["us-west-2", "eu-central-1", "ap-southeast-4"];

/// A `PutAttachmentRoutingPolicyLabel` request body, carrying `ClientToken` when `token`
/// names one.
fn put_body(core_network: &str, attachment: &str, label: &str, token: Option<&str>) -> Value {
    let mut body = json!({
        "CoreNetworkId": core_network,
        "AttachmentId": attachment,
        "RoutingPolicyLabel": label,
    });
    if let Some(token) = token {
        body["ClientToken"] = json!(token);
    }
    body
}

/// `PutAttachmentRoutingPolicyLabel`.
async fn put(emulator: &Emulator, body: Value) -> (u16, Value) {
    emulator.post(PUT_URI, body).await
}

/// `PutAttachmentRoutingPolicyLabel`, asserting the call succeeded and that its payload
/// reports the two ids it addressed and the label it applied.
async fn put_ok(
    emulator: &Emulator,
    case: &str,
    core_network: &str,
    attachment: &str,
    label: &str,
    token: Option<&str>,
) -> Value {
    let (status, answer) = put(
        emulator,
        put_body(core_network, attachment, label, token),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutAttachmentRoutingPolicyLabel expected 200, got {status} with {answer}"
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
        Some(label),
        "case {case:?}: expected RoutingPolicyLabel {label:?}, got {answer}"
    );
    answer
}

/// The label `ListAttachmentRoutingPolicyAssociations` reports for `attachment`, or `None`
/// when that attachment has no routing policy association at all.
async fn stored_label(
    emulator: &Emulator,
    case: &str,
    core_network: &str,
    attachment: &str,
) -> Option<String> {
    let (status, body) = emulator
        .get(&format!(
            "{LIST_PREFIX}/{core_network}?attachmentId={attachment}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the listing expected 200, got {status} with {body}"
    );
    let listed = body["AttachmentRoutingPolicyAssociations"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected an AttachmentRoutingPolicyAssociations array in {body}")
        });
    assert!(
        listed.len() <= 1,
        "case {case:?}: an attachment carries one routing policy label, got {body}"
    );
    listed.first().map(|summary| {
        assert_eq!(
            summary["AttachmentId"].as_str(),
            Some(attachment),
            "case {case:?}: the summary must name the attachment it belongs to, got {summary}"
        );
        summary["RoutingPolicyLabel"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no RoutingPolicyLabel in {summary}"))
            .to_string()
    })
}

/// Which kind of attachment a row creates. The three kinds share the stored attachment
/// envelope and each takes an optional `RoutingPolicyLabel` on its create (`smithy_query
/// query="operation"
/// operation="CreateVpcAttachment,CreateConnectAttachment,CreateSiteToSiteVpnAttachment"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Vpc,
    Connect,
    Vpn,
}

/// One row of the table: an attachment of one kind at one edge location, the label its
/// create carried (or none), and the label this call applies to it.
struct LabelCase {
    /// Names the case in every assertion message.
    name: &'static str,
    kind: Kind,
    /// The edge location the attachment sits at, which the core network under test
    /// configures an edge for.
    region: &'static str,
    /// The `RoutingPolicyLabel` the create carried, or `None` for a create that carried
    /// none — an attachment the call gives its FIRST label.
    created: Option<&'static str>,
    /// The `RoutingPolicyLabel` this call applies.
    applied: String,
}

/// A VPN connection ARN in `region`, as the modelled `VpnConnectionArn` pattern spells one
/// (`^arn:[^:]{1,63}:ec2:[^:]{0,63}:[^:]{0,63}:vpn-connection\/vpn-[0-9a-f]{8,17}$`,
/// `smithy_query shape=VpnConnectionArn`).
fn vpn_arn(region: &str, index: usize) -> String {
    format!(
        "arn:aws:ec2:{region}:{}:vpn-connection/vpn-{index:08x}",
        engine::emulator_account_id()
    )
}

/// The table: rows differing along everything the rule generalises over — the attachment
/// kind, the edge location, whether a label was already stored, and the label applied.
fn label_cases() -> Vec<LabelCase> {
    vec![
        LabelCase {
            name: "a VPC attachment that carried no label takes a plain one",
            kind: Kind::Vpc,
            region: "us-west-2",
            created: None,
            applied: "north".to_string(),
        },
        LabelCase {
            name: "a VPC attachment's label is replaced by another",
            kind: Kind::Vpc,
            region: "us-west-2",
            created: Some("north"),
            applied: "south".to_string(),
        },
        LabelCase {
            name: "a VPC attachment takes the empty label the modelled length 0..256 allows",
            kind: Kind::Vpc,
            region: "eu-central-1",
            created: Some("north"),
            applied: String::new(),
        },
        LabelCase {
            name: "a VPC attachment takes the 256-character label at the modelled bound",
            kind: Kind::Vpc,
            region: "ap-southeast-4",
            created: None,
            applied: "l".repeat(MAX_LABEL_CHARS),
        },
        LabelCase {
            name: "a VPC attachment takes a label with spaces and non-ASCII",
            kind: Kind::Vpc,
            region: "eu-central-1",
            created: Some("plain"),
            applied: "ingress policy \u{2013} \u{3c0}".to_string(),
        },
        LabelCase {
            name: "a Connect attachment that carried no label takes one",
            kind: Kind::Connect,
            region: "us-west-2",
            created: None,
            applied: "connect-edge".to_string(),
        },
        LabelCase {
            name: "a Connect attachment is given the label it already carries",
            kind: Kind::Connect,
            region: "us-west-2",
            created: Some("keep"),
            applied: "keep".to_string(),
        },
        LabelCase {
            name: "a site-to-site VPN attachment that carried no label takes one",
            kind: Kind::Vpn,
            region: "ap-southeast-4",
            created: None,
            applied: "vpn-south".to_string(),
        },
        LabelCase {
            name: "a site-to-site VPN attachment's label is replaced by another",
            kind: Kind::Vpn,
            region: "eu-central-1",
            created: Some("old"),
            applied: "new".to_string(),
        },
    ]
}

/// What one attachment to create looks like: how it is named in an assertion, its kind,
/// the edge location it sits at, and the label its own create carries.
struct AttachmentSpec<'a> {
    name: &'a str,
    kind: Kind,
    region: &'a str,
    label: Option<&'a str>,
}

/// Create the attachment `spec` describes on `core_network`, answering its minted id.
///
/// `index` keeps each row's VPC / subnet / VPN ARNs distinct, and `transport` is the VPC
/// attachment a Connect attachment is carried over.
async fn create_case(
    emulator: &Emulator,
    core_network: &str,
    index: usize,
    transport: &str,
    spec: &AttachmentSpec<'_>,
) -> String {
    let AttachmentSpec {
        name,
        kind,
        region,
        label,
    } = spec;
    let (kind, region) = (*kind, *region);
    let mut body = match kind {
        Kind::Vpc => json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(region, index),
            "SubnetArns": [subnet_arn(region, index)],
        }),
        Kind::Connect => json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": region,
            "TransportAttachmentId": transport,
            "Options": { "Protocol": "GRE" },
        }),
        Kind::Vpn => json!({
            "CoreNetworkId": core_network,
            "VpnConnectionArn": vpn_arn(region, index),
        }),
    };
    if let Some(label) = label {
        body["RoutingPolicyLabel"] = json!(label);
    }

    let (wrapper, (status, created)) = match kind {
        Kind::Vpc => ("VpcAttachment", create_vpc_attachment(emulator, body).await),
        Kind::Connect => (
            "ConnectAttachment",
            create_connect_attachment(emulator, body).await,
        ),
        Kind::Vpn => (
            "SiteToSiteVpnAttachment",
            emulator.post("/site-to-site-vpn-attachments", body).await,
        ),
    };
    assert_eq!(
        status, 200,
        "case {name:?}: the create expected 200, got {status} with {created}"
    );
    attachment_id(name, &created[wrapper])
}

/// A core network configuring an edge in each of [`REGIONS`], plus one unlabelled VPC
/// attachment to carry the Connect attachments of the table. Answers
/// `(core network id, transport attachment id)`.
///
/// `base` offsets the ARNs this subject's attachments attach, so two subjects of one
/// emulator attach different VPCs and VPN connections: a resource is attached by at most
/// one attachment across every core network.
async fn seeded(emulator: &Emulator, name: &str, base: usize) -> (String, String) {
    let (_, core_network) = core_network_at(emulator, name, &REGIONS).await;
    let transport = create_case(
        emulator,
        &core_network,
        base,
        "",
        &AttachmentSpec {
            name: "the Connect transport",
            kind: Kind::Vpc,
            region: "us-west-2",
            label: None,
        },
    )
    .await;
    (core_network, transport)
}

#[tokio::test]
async fn every_put_stores_its_label_and_the_listing_reports_it() {
    let emulator = Emulator::new();
    let (core_network, transport) = seeded(&emulator, "labels", 0).await;

    let cases = label_cases();
    let mut applied: Vec<(String, String)> = Vec::new();
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

        // What the create stored, so the row's assertions below are about the PUT rather
        // than about the create.
        assert_eq!(
            stored_label(&emulator, case.name, &core_network, &id).await,
            case.created.map(str::to_string),
            "case {:?}: the create must store the label it carried",
            case.name
        );

        let token = format!("token-{index}");
        put_ok(
            &emulator,
            case.name,
            &core_network,
            &id,
            &case.applied,
            Some(&token),
        )
        .await;
        assert_eq!(
            stored_label(&emulator, case.name, &core_network, &id).await,
            Some(case.applied.clone()),
            "case {:?}: the listing must report the label the call stored",
            case.name
        );

        // The same request again, under the same `@idempotencyToken`: one label is stored,
        // the same payload is answered, and nothing is duplicated.
        put_ok(
            &emulator,
            case.name,
            &core_network,
            &id,
            &case.applied,
            Some(&token),
        )
        .await;
        assert_eq!(
            stored_label(&emulator, case.name, &core_network, &id).await,
            Some(case.applied.clone()),
            "case {:?}: a retry of the same call must leave the same single label",
            case.name
        );

        applied.push((id, case.applied.clone()));
    }

    // Every row at once: each attachment carries exactly the label applied to it, so no
    // call reached an attachment other than the one it addressed. The transport carries
    // none, because nothing labelled it.
    for (id, label) in &applied {
        assert_eq!(
            stored_label(&emulator, id, &core_network, id).await,
            Some(label.clone()),
            "case {id:?}: expected the label this attachment was given"
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
async fn a_label_past_the_published_bound_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let (core_network, transport) = seeded(&emulator, "bounds", 0).await;
    let id = create_case(
        &emulator,
        &core_network,
        1,
        &transport,
        &AttachmentSpec {
            name: "the bound",
            kind: Kind::Vpc,
            region: "us-west-2",
            label: Some("kept"),
        },
    )
    .await;

    // Each row: a label and whether the modelled `length 0..256` admits it. The two
    // published bounds are accepted; one character past the upper bound is refused.
    for (case, label, accepted) in [
        ("the empty label at the lower bound", String::new(), true),
        (
            "the label at the upper bound",
            "l".repeat(MAX_LABEL_CHARS),
            true,
        ),
        (
            "one character past the upper bound",
            "l".repeat(MAX_LABEL_CHARS + 1),
            false,
        ),
        (
            "far past the upper bound",
            "l".repeat(MAX_LABEL_CHARS * 4),
            false,
        ),
    ] {
        let before = stored_label(&emulator, case, &core_network, &id).await;
        let (status, answer) = put(
            &emulator,
            put_body(&core_network, &id, &label, Some("bounds-token")),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {answer}"
            );
            assert_eq!(
                stored_label(&emulator, case, &core_network, &id).await,
                Some(label.clone()),
                "case {case:?}: the accepted label must be the one stored"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case:?}: wrong error code in {answer}"
            );
            assert_eq!(
                stored_label(&emulator, case, &core_network, &id).await,
                before,
                "case {case:?}: a refused call must leave the stored label alone"
            );
        }
    }
}

#[tokio::test]
async fn an_id_addressing_nothing_stored_is_refused_and_stores_nothing() {
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
            "POST",
            PUT_URI,
            Some(put_body(
                &core_network_id,
                &attachment,
                "attempted",
                Some(case),
            )),
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
        "case {:?}: a refused call must store nothing",
        "the first attachment"
    );
    assert_eq!(
        stored_label(&emulator, "the other attachment", &other_network, &elsewhere).await,
        Some("theirs".to_string()),
        "case {:?}: a refused call must store nothing",
        "the other attachment"
    );

    // A deleted attachment is an id that names nothing again, so labelling it is the same
    // refusal.
    let (status, deleted) = emulator.delete(&format!("/attachments/{id}")).await;
    assert_eq!(
        status, 200,
        "DeleteAttachment expected 200, got {status} with {deleted}"
    );
    assert_not_found_answer(
        &emulator,
        "a deleted attachment",
        "POST",
        PUT_URI,
        Some(put_body(&core_network, &id, "attempted", None)),
        &id,
        ATTACHMENT,
    )
    .await;
}

#[tokio::test]
async fn labelling_an_attachment_changes_nothing_else_about_it() {
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
            label: None,
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

    put_ok(&emulator, case, &core_network, &id, "edge", None).await;

    let (status, after) = emulator.get(&format!("/vpc-attachments/{id}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetVpcAttachment expected 200, got {status} with {after}"
    );
    assert_eq!(
        after, before,
        "case {case:?}: the attachment's own read must be unchanged by a label, got {after}"
    );
    assert_eq!(
        stored_label(&emulator, case, &core_network, &id).await,
        Some("edge".to_string()),
        "case {case:?}: the label must still have been stored"
    );
}

#[tokio::test]
async fn an_attachment_waiting_for_its_acceptance_can_be_labelled() {
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
            label: None,
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

    put_ok(&emulator, case, &core_network, &id, "waiting", None).await;
    assert_eq!(
        stored_label(&emulator, case, &core_network, &id).await,
        Some("waiting".to_string()),
        "case {case:?}: an attachment takes a label whatever state it is in"
    );

    // The label is not an acceptance and owes no transition: past the span a started
    // attachment settles in, this one is exactly where it was.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, settled) = emulator.get(&format!("/vpc-attachments/{id}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetVpcAttachment expected 200, got {status} with {settled}"
    );
    assert_eq!(
        settled["VpcAttachment"]["Attachment"]["State"].as_str(),
        Some(PENDING_ACCEPTANCE),
        "case {case:?}: labelling must not start an attachment, got {settled}"
    );
}
