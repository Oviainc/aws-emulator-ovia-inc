// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `RejectAttachment`, driven over HTTP through the real router (the
//! harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: an attachment request exists only while the attachment is waiting
//! for the acceptance the `attachment-policies` rule that claimed it asked for, so
//! `RejectAttachment` turns that request down — for an attachment of any kind — and the
//! attachment is then `REJECTED` (an `AttachmentState` value, `smithy_query
//! shape=AttachmentState`) for good: the request was never granted, so the attachment is
//! never created and no span of the clock moves it again, while the record stays stored and
//! readable until it is deleted. The decision is taken once, so a second rejection — and an
//! acceptance of an attachment already rejected — has no request left to decide, which is
//! this service's concurrent-state refusal (`ConflictException`, HTTP 409), exactly as an
//! attachment that was never waiting for an acceptance has none to reject. An id that names
//! no stored attachment is `ResourceNotFoundException` (HTTP 404).
//!
//! The tables below span that class: rows differing in attachment kind (VPC, Connect,
//! site-to-site VPN, Direct Connect gateway), in Region and so in edge location, in the
//! lifecycle state the attachment is in when the rejection is issued
//! (`PENDING_ATTACHMENT_ACCEPTANCE`, `CREATING`, `AVAILABLE`), and in whether the attachment
//! carries tags; the not-found rows differ in id length across the span the model's own
//! `^attachment-([0-9a-f]{8,17})$` allows (`smithy_query shape=AttachmentId`). Nothing
//! reaches into `crate::state`: every attachment is created, rejected and read back through
//! an operation.

use serde_json::{json, Value};

use crate::attachment_acceptance::{accept, core_network_with};
use crate::attachment_lifecycle::core_network_at;
use crate::delete_attachment::{create_attachment_of_kind, requires_acceptance, Kind};
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};

/// The ARN resource type an attachment refusal reports in `ResourceType`, which is also the
/// id prefix the model constrains (`^attachment-([0-9a-f]{8,17})$`, `smithy_query
/// shape=AttachmentId`).
const ATTACHMENT: &str = "attachment";

/// The four `AttachmentState` values this suite names (`smithy_query
/// shape=AttachmentState`).
const PENDING_ACCEPTANCE: &str = "PENDING_ATTACHMENT_ACCEPTANCE";
/// See [`PENDING_ACCEPTANCE`].
const CREATING: &str = "CREATING";
/// See [`PENDING_ACCEPTANCE`].
const AVAILABLE: &str = "AVAILABLE";
/// See [`PENDING_ACCEPTANCE`].
const REJECTED: &str = "REJECTED";

/// `RejectAttachment` (`POST /attachments/{AttachmentId}/reject`).
async fn reject(emulator: &Emulator, id: &str) -> (u16, Value) {
    emulator
        .call("POST", &format!("/attachments/{id}/reject"), None)
        .await
}

/// The attachment ids `ListAttachments` reports for `state`, which reads the same store the
/// reader of each kind does.
async fn listed_in_state(emulator: &Emulator, case: &str, state: &str) -> Vec<String> {
    let (status, body) = emulator.get(&format!("/attachments?state={state}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAttachments expected 200, got {status} with {body}"
    );
    body["Attachments"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Attachments list in {body}"))
        .iter()
        .map(|entry| entry["AttachmentId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One row of the rejection table.
struct RejectCase {
    /// Names the case in every assertion message.
    name: &'static str,
    kind: Kind,
    /// The Region of the core network's single edge location, and so of the attachment.
    region: &'static str,
    /// Whether the core network's policy holds every new attachment for an acceptance, which
    /// is what puts the attachment in [`PENDING_ACCEPTANCE`] — the one state a rejection can
    /// act on.
    require_acceptance: bool,
    /// Whether to let the settle span the clock owes elapse before the rejection, which is
    /// what moves a started attachment from [`CREATING`] to [`AVAILABLE`].
    settle: bool,
    /// The `Tags` the create carries (`Value::Null` for no `Tags` member) and the list the
    /// rejected attachment must then still report.
    tags: Value,
    expected_tags: Value,
    /// The state the attachment must be in when the rejection is issued.
    state_before: &'static str,
}

/// The table: rows differ along the attachment kind, the Region (and so the edge location),
/// the state the attachment is in when it is rejected, and whether it carries tags —
/// everything the rule generalises over.
fn reject_cases() -> Vec<RejectCase> {
    vec![
        RejectCase {
            name: "a VPC attachment waiting for its acceptance, no tags",
            kind: Kind::Vpc,
            region: "us-west-2",
            require_acceptance: true,
            settle: false,
            tags: Value::Null,
            expected_tags: json!([]),
            state_before: PENDING_ACCEPTANCE,
        },
        RejectCase {
            name: "a Connect attachment waiting for its acceptance, with two tags",
            kind: Kind::Connect,
            region: "sa-east-1",
            require_acceptance: true,
            settle: false,
            tags: json!([{ "Key": "env", "Value": "prod" }, { "Key": "team", "Value": "net" }]),
            expected_tags: json!([
                { "Key": "env", "Value": "prod" },
                { "Key": "team", "Value": "net" },
            ]),
            state_before: PENDING_ACCEPTANCE,
        },
        RejectCase {
            name: "a site-to-site VPN attachment waiting for its acceptance, with an empty tag value",
            kind: Kind::SiteToSiteVpn,
            region: "eu-west-1",
            require_acceptance: true,
            settle: false,
            tags: json!([{ "Key": "only", "Value": "" }]),
            expected_tags: json!([{ "Key": "only", "Value": "" }]),
            state_before: PENDING_ACCEPTANCE,
        },
        RejectCase {
            name: "a Direct Connect gateway attachment waiting for its acceptance",
            kind: Kind::DirectConnectGateway,
            region: "eu-west-2",
            require_acceptance: true,
            settle: false,
            tags: json!([{ "Key": "tier", "Value": "edge" }]),
            expected_tags: json!([{ "Key": "tier", "Value": "edge" }]),
            state_before: PENDING_ACCEPTANCE,
        },
        RejectCase {
            name: "a VPC attachment still CREATING, which waited for no acceptance",
            kind: Kind::Vpc,
            region: "ap-south-1",
            require_acceptance: false,
            settle: false,
            tags: Value::Null,
            expected_tags: json!([]),
            state_before: CREATING,
        },
        RejectCase {
            name: "a Connect attachment already AVAILABLE",
            kind: Kind::Connect,
            region: "us-east-1",
            require_acceptance: false,
            settle: true,
            tags: json!([{ "Key": "env", "Value": "test" }]),
            expected_tags: json!([{ "Key": "env", "Value": "test" }]),
            state_before: AVAILABLE,
        },
        RejectCase {
            name: "a site-to-site VPN attachment still CREATING",
            kind: Kind::SiteToSiteVpn,
            region: "ca-central-1",
            require_acceptance: false,
            settle: false,
            tags: Value::Null,
            expected_tags: json!([]),
            state_before: CREATING,
        },
        RejectCase {
            name: "a Direct Connect gateway attachment already AVAILABLE",
            kind: Kind::DirectConnectGateway,
            region: "ap-southeast-4",
            require_acceptance: false,
            settle: true,
            tags: json!([{ "Key": "env", "Value": "prod" }]),
            expected_tags: json!([{ "Key": "env", "Value": "prod" }]),
            state_before: AVAILABLE,
        },
    ]
}

/// Create the core network a row needs, answering its id: one whose policy holds every new
/// attachment for an acceptance, or one whose policy carries no `attachment-policies` at all.
async fn core_network_for(emulator: &Emulator, case: &RejectCase) -> String {
    if case.require_acceptance {
        core_network_with(emulator, case.name, case.region, &requires_acceptance()).await
    } else {
        core_network_at(emulator, case.name, &[case.region]).await.1
    }
}

/// The `Attachment` envelope the reader of `kind` answers for `id`, and the status it came
/// with.
async fn read_back(emulator: &Emulator, case: &str, kind: Kind, id: &str) -> (u16, Value) {
    let (status, body) = emulator.get(&kind.get_uri(id)).await;
    let attachment = body[kind.wrapper()]["Attachment"].clone();
    assert_eq!(
        status, 200,
        "case {case:?}: the reader of this kind expected 200, got {status} with {body}"
    );
    (status, attachment)
}

#[tokio::test]
async fn rejecting_an_attachment_turns_down_the_request_it_was_waiting_for() {
    let emulator = Emulator::new();
    // The attachments that were rejected, so the one thing left to show — that no span of
    // the clock moves a rejected attachment — costs ONE wait rather than one per row.
    let mut rejected: Vec<(&'static str, Kind, String)> = Vec::new();

    for (index, case) in reject_cases().iter().enumerate() {
        let name = case.name;
        let core_network = core_network_for(&emulator, case).await;
        let (id, _) = create_attachment_of_kind(
            &emulator,
            name,
            case.kind,
            &core_network,
            case.region,
            index + 1,
            &case.tags,
        )
        .await;
        if case.settle {
            tokio::time::sleep(PAST_SETTLE).await;
        }

        // The state the row says the attachment is in, read back through its own reader, so
        // the rejection below is issued against a known state rather than an assumed one.
        let (_, before) = read_back(&emulator, name, case.kind, &id).await;
        assert_eq!(
            before["State"],
            json!(case.state_before),
            "case {name:?}: expected the attachment to be {} before the rejection, got {before}",
            case.state_before
        );

        let (status, answered) = reject(&emulator, &id).await;

        if case.state_before != PENDING_ACCEPTANCE {
            // Nothing was waiting for a decision, so there is no attachment request to turn
            // down and the stored attachment is left exactly where it was.
            assert_eq!(
                status, 409,
                "case {name:?}: RejectAttachment on an attachment awaiting no acceptance expected 409, got {status} with {answered}"
            );
            assert_eq!(
                error_code(&answered),
                "ConflictException",
                "case {name:?}: expected ConflictException, got {answered}"
            );
            assert_eq!(
                answered["ResourceId"],
                json!(id),
                "case {name:?}: the refusal names the attachment asked for, got {answered}"
            );
            assert_eq!(
                answered["ResourceType"],
                json!(ATTACHMENT),
                "case {name:?}: the refusal names the resource type {ATTACHMENT}, got {answered}"
            );
            let (_, after) = read_back(&emulator, name, case.kind, &id).await;
            assert_eq!(
                after["State"],
                json!(case.state_before),
                "case {name:?}: a refused rejection must change nothing, got {after}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {name:?}: RejectAttachment expected 200, got {status} with {answered}"
        );
        let turned_down = &answered["Attachment"];
        assert_eq!(
            turned_down["AttachmentId"],
            json!(id),
            "case {name:?}: RejectAttachment must answer the attachment asked for, got {answered}"
        );
        assert_eq!(
            turned_down["State"],
            json!(REJECTED),
            "case {name:?}: a turned-down attachment request leaves the attachment {REJECTED}, got {answered}"
        );
        assert_eq!(
            turned_down["AttachmentType"],
            json!(case.kind.attachment_type()),
            "case {name:?}: expected AttachmentType {}, got {answered}",
            case.kind.attachment_type()
        );
        assert_eq!(
            turned_down["CoreNetworkId"], before["CoreNetworkId"],
            "case {name:?}: the rejected attachment keeps the core network it belonged to, got {answered}"
        );
        assert_eq!(
            turned_down["ResourceArn"], before["ResourceArn"],
            "case {name:?}: the rejected attachment keeps the resource it would have attached, got {answered}"
        );
        assert_eq!(
            turned_down["Tags"], case.expected_tags,
            "case {name:?}: expected the tags the create carried, got {answered}"
        );
        let created_at = turned_down["CreatedAt"].as_f64().unwrap_or_default();
        let updated_at = turned_down["UpdatedAt"].as_f64().unwrap_or_default();
        assert!(
            updated_at >= created_at && created_at > 0.0,
            "case {name:?}: the rejection is the attachment's latest change, got CreatedAt {created_at} and UpdatedAt {updated_at}"
        );

        // The decision was STORED: the reader of this kind answers the rejected attachment,
        // and so does the listing selected on that state — two operations reading one store.
        let (_, stored) = read_back(&emulator, name, case.kind, &id).await;
        assert_eq!(
            stored["State"],
            json!(REJECTED),
            "case {name:?}: the rejection must be persisted, got {stored}"
        );
        let in_rejected = listed_in_state(&emulator, name, REJECTED).await;
        assert!(
            in_rejected.contains(&id),
            "case {name:?}: expected {id} among the {REJECTED} attachments, got {in_rejected:?}"
        );
        let still_pending = listed_in_state(&emulator, name, PENDING_ACCEPTANCE).await;
        assert!(
            !still_pending.contains(&id),
            "case {name:?}: {id} waits for nothing any more, got {still_pending:?}"
        );

        // The decision is taken ONCE: neither a second rejection nor an acceptance has an
        // attachment request left to decide.
        let (status, again) = reject(&emulator, &id).await;
        assert_eq!(
            status, 409,
            "case {name:?}: a second RejectAttachment expected 409, got {status} with {again}"
        );
        assert_eq!(
            error_code(&again),
            "ConflictException",
            "case {name:?}: expected ConflictException from the second rejection, got {again}"
        );
        let (status, late_accept) = accept(&emulator, &id).await;
        assert_eq!(
            status, 409,
            "case {name:?}: AcceptAttachment after a rejection expected 409, got {status} with {late_accept}"
        );
        assert_eq!(
            error_code(&late_accept),
            "ConflictException",
            "case {name:?}: expected ConflictException from the late acceptance, got {late_accept}"
        );
        let (_, unchanged) = read_back(&emulator, name, case.kind, &id).await;
        assert_eq!(
            unchanged["State"],
            json!(REJECTED),
            "case {name:?}: a refused decision must leave the rejection standing, got {unchanged}"
        );

        rejected.push((name, case.kind, id));
    }

    assert_eq!(
        rejected.len(),
        4,
        "the table must exercise a rejection of every attachment kind, got {rejected:?}"
    );

    // Waiting past the span a started attachment settles in leaves a rejected one exactly
    // where it is: the request was never granted, so nothing is owed on it.
    tokio::time::sleep(PAST_SETTLE).await;
    for (name, kind, id) in &rejected {
        let (_, late) = read_back(&emulator, name, *kind, id).await;
        assert_eq!(
            late["State"],
            json!(REJECTED),
            "case {name:?}: no span of the clock may move a rejected attachment, got {late}"
        );

        // The record is still THERE — a rejection stores a decision, it does not remove the
        // attachment — so the delete that does remove it still finds it.
        let (status, removed) = emulator.delete(&format!("/attachments/{id}")).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a rejected attachment is still stored, so DeleteAttachment expected 200, got {status} with {removed}"
        );
    }
}

#[tokio::test]
async fn rejecting_an_attachment_id_that_names_nothing_is_the_services_not_found_refusal() {
    let emulator = Emulator::new();

    // Ids this service never minted, differing in length across the span the model's own
    // `^attachment-([0-9a-f]{8,17})$` allows (`smithy_query shape=AttachmentId`): the 8-hex
    // lower bound, the 17-hex upper bound, and one in between.
    let unknown: Vec<(&str, &str)> = vec![
        ("the 8-hex lower bound of the pattern", "attachment-0123abcd"),
        ("a 12-hex id inside the pattern", "attachment-0123456789ab"),
        (
            "the 17-hex upper bound of the pattern",
            "attachment-0123456789abcdef0",
        ),
    ];

    for (case, id) in unknown {
        let (status, body) = reject(&emulator, id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for an id naming nothing, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(id),
            "case {case:?}: the refusal names the id asked for, got {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!(ATTACHMENT),
            "case {case:?}: the refusal names the resource type {ATTACHMENT}, got {body}"
        );
    }

    // An id outside the modelled pattern never reaches the handler: the constraint walk
    // refuses it first, with the modelled `ValidationException` (HTTP 400).
    let (status, refused) = reject(&emulator, "attachment-zzzz").await;
    assert_eq!(
        status, 400,
        "an id outside the modelled pattern is HTTP 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected ValidationException, got {refused}"
    );
}
