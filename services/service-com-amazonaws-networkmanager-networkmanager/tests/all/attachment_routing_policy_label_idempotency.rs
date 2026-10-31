// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the `@idempotencyToken` `PutAttachmentRoutingPolicyLabel` models,
//! driven over HTTP through the real router (the harness lives in
//! `crate::global_network_lifecycle`, the label helpers in
//! `crate::attachment_routing_policy_label`).
//!
//! The rule under test is the one every tokened operation of this service follows, and
//! `crate::client_token_idempotency` spans across operations: a non-empty `ClientToken` is
//! resolved against what the store already holds under it — the same request is ANSWERED
//! from the record that token already wrote and writes nothing more, and the same token
//! asking for anything else is refused `ConflictException` (HTTP 409) naming that record and
//! writes nothing — while an EMPTY token is no token at all, so a request carrying one
//! behaves exactly as a request carrying no `ClientToken` member. For this call the record
//! is the label one attachment carries, so "anything else" is another label or the label of
//! another attachment. The resources a request addresses are resolved before its token, so a
//! token already spent never turns a not-found refusal into a conflict.
//!
//! The table below spans that class: rows differing in what the second call under one token
//! changes — the label, the empty label the modelled `length 0..256` allows, the label at
//! that 256-character upper bound, a label carrying spaces and non-ASCII (which the modelled
//! `^[\s\S]*$` allows), the attachment labelled, and an attachment of ANOTHER core network —
//! and in the kind and edge location of the attachment each row labels. Nothing reaches into
//! `crate::state`: every label is applied and read back through an operation.

use serde_json::Value;

use crate::attachment_routing_policy_label::{
    create_case, put, put_body, put_ok, seeded, stored_label, AttachmentSpec, Kind, ATTACHMENT,
    CORE_NETWORK, MAX_LABEL_CHARS, PUT_URI,
};
use crate::attachment_routing_policy_label_removal::remove_uri;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::resource_not_found_message::assert_not_found_answer;

/// Which attachment the SECOND call under a row's token addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The attachment the first call under the token labelled.
    Same,
    /// Another attachment of the same core network.
    Sibling,
    /// An attachment of another core network entirely.
    Elsewhere,
}

/// One row: an attachment to label, the label the row's token first applies to it, and the
/// request the same token makes afterwards.
struct Row {
    /// Names the case in every assertion message.
    name: &'static str,
    /// The kind of attachment the row labels.
    kind: Kind,
    /// The edge location that attachment sits at, which the core network under test
    /// configures an edge for.
    region: &'static str,
    /// The label the first call under the row's token applies.
    first: String,
    /// The attachment the second call under that token addresses.
    target: Target,
    /// The label that second call asks for.
    second: String,
}

/// The table: rows differing along everything the rule generalises over — which member of
/// the request the second call changes, and the kind and edge location of the attachment
/// being labelled.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "another label on the same attachment",
            kind: Kind::Vpc,
            region: "us-west-2",
            first: "blue".to_string(),
            target: Target::Same,
            second: "red".to_string(),
        },
        Row {
            name: "the empty label the modelled length 0..256 allows",
            kind: Kind::Vpc,
            region: "eu-central-1",
            first: "blue".to_string(),
            target: Target::Same,
            second: String::new(),
        },
        Row {
            name: "an empty label first, a non-empty one second",
            kind: Kind::Vpc,
            region: "ap-southeast-4",
            first: String::new(),
            target: Target::Same,
            second: "green".to_string(),
        },
        Row {
            name: "the label at the modelled 256-character bound, then a shorter one",
            kind: Kind::Vpc,
            region: "eu-central-1",
            first: "l".repeat(MAX_LABEL_CHARS),
            target: Target::Same,
            second: "l".repeat(MAX_LABEL_CHARS - 1),
        },
        Row {
            name: "a label with spaces and non-ASCII",
            kind: Kind::Vpn,
            region: "ap-southeast-4",
            first: "ingress policy \u{2013} \u{3c0}".to_string(),
            target: Target::Same,
            second: "ingress policy \u{2013} \u{3c1}".to_string(),
        },
        Row {
            name: "the same label on another attachment of the same core network",
            kind: Kind::Connect,
            region: "us-west-2",
            first: "shared".to_string(),
            target: Target::Sibling,
            second: "shared".to_string(),
        },
        Row {
            name: "the same label on an attachment of another core network",
            kind: Kind::Vpn,
            region: "eu-central-1",
            first: "shared".to_string(),
            target: Target::Elsewhere,
            second: "shared".to_string(),
        },
    ]
}

/// The resources the rows label: one attachment per row, one sibling of the same core
/// network, and one attachment of a second core network.
struct Fixture {
    core_network: String,
    /// The attachment each row of [`rows`] labels, in row order.
    subjects: Vec<String>,
    /// Another attachment of [`Self::core_network`], which no row's first call labels.
    sibling: String,
    other_network: String,
    /// An attachment of [`Self::other_network`], which no row's first call labels.
    elsewhere: String,
}

impl Fixture {
    /// The `(core network, attachment)` pair the second call of `row` addresses, and the
    /// attachment each row labels.
    fn target(&self, row: &Row, subject: &str) -> (&str, String) {
        match row.target {
            Target::Same => (self.core_network.as_str(), subject.to_string()),
            Target::Sibling => (self.core_network.as_str(), self.sibling.clone()),
            Target::Elsewhere => (self.other_network.as_str(), self.elsewhere.clone()),
        }
    }
}

/// Build the fixture in a fresh emulator: a core network with an edge in each region the
/// table names, one attachment per row, a sibling attachment, and a second core network
/// holding one attachment of its own.
async fn fixture(emulator: &Emulator, rows: &[Row]) -> Fixture {
    let (core_network, transport) = seeded(emulator, "label-idempotency", 0).await;
    let mut subjects = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        subjects.push(
            create_case(
                emulator,
                &core_network,
                index + 1,
                &transport,
                &AttachmentSpec {
                    name: row.name,
                    kind: row.kind,
                    region: row.region,
                    label: None,
                },
            )
            .await,
        );
    }
    let sibling = create_case(
        emulator,
        &core_network,
        50,
        &transport,
        &AttachmentSpec {
            name: "the sibling attachment",
            kind: Kind::Vpc,
            region: "us-west-2",
            label: None,
        },
    )
    .await;

    let (other_network, other_transport) = seeded(emulator, "label-idempotency-other", 100).await;
    let elsewhere = create_case(
        emulator,
        &other_network,
        101,
        &other_transport,
        &AttachmentSpec {
            name: "an attachment of another core network",
            kind: Kind::Vpc,
            region: "eu-central-1",
            label: None,
        },
    )
    .await;

    Fixture {
        core_network,
        subjects,
        sibling,
        other_network,
        elsewhere,
    }
}

/// The message a refusal carries: `ConflictException` reports it in `Message`.
fn message_of(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_token_replays_its_label_and_refuses_a_different_request() {
    let emulator = Emulator::new();
    let rows = rows();
    let fixture = fixture(&emulator, &rows).await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.name;
        // One token per row, so each row starts from a token nothing is stored under.
        let token = format!("label-token-{index}");
        let subject = fixture.subjects[index].clone();
        let (target_network, target) = fixture.target(row, &subject);

        let first = put_ok(
            &emulator,
            case,
            &fixture.core_network,
            &subject,
            &row.first,
            Some(&token),
        )
        .await;
        assert_eq!(
            stored_label(&emulator, case, &fixture.core_network, &subject).await,
            Some(row.first.clone()),
            "case {case:?}: the first call must store the label it carried"
        );

        // The same request again: answered from the label that token already wrote, with
        // nothing else stored.
        let retried = put_ok(
            &emulator,
            case,
            &fixture.core_network,
            &subject,
            &row.first,
            Some(&token),
        )
        .await;
        assert_eq!(
            retried, first,
            "case {case:?}: a retry must answer what the first call answered ({first}), got {retried}"
        );
        assert_eq!(
            stored_label(&emulator, case, &fixture.core_network, &subject).await,
            Some(row.first.clone()),
            "case {case:?}: a retry must leave the same single label"
        );

        // The same token asking for something else.
        let target_before = stored_label(&emulator, case, target_network, &target).await;
        let (status, refused) = put(
            &emulator,
            put_body(target_network, &target, &row.second, Some(&token)),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: the same token asking for something else expected 409, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused}"
        );
        assert_eq!(
            refused["ResourceId"].as_str(),
            Some(subject.as_str()),
            "case {case:?}: the refusal must name the attachment the token labelled, got {refused}"
        );
        assert_eq!(
            refused["ResourceType"].as_str(),
            Some(ATTACHMENT),
            "case {case:?}: the refusal must name the resource type, got {refused}"
        );
        let expected_message = format!(
            "client token {token:?} already applied routing policy label {:?} to attachment {subject}, with a different request",
            row.first
        );
        assert_eq!(
            message_of(&refused),
            expected_message,
            "case {case:?}: expected the refusal to read {expected_message:?}, got {refused}"
        );

        // The refusal wrote nothing: neither the attachment the token labelled nor the one
        // the refused request addressed changed.
        assert_eq!(
            stored_label(&emulator, case, &fixture.core_network, &subject).await,
            Some(row.first.clone()),
            "case {case:?}: a refused call must leave the stored label alone"
        );
        assert_eq!(
            stored_label(&emulator, case, target_network, &target).await,
            target_before,
            "case {case:?}: a refused call must leave the addressed attachment alone"
        );

        // And the token still answers the label it first wrote.
        let again = put_ok(
            &emulator,
            case,
            &fixture.core_network,
            &subject,
            &row.first,
            Some(&token),
        )
        .await;
        assert_eq!(
            again, first,
            "case {case:?}: the refusal must have stored nothing, got {again}"
        );
    }
}

/// Drive every row's first call, a retry of it and the changed request under `token`,
/// answering the `(case, status, error code, label stored on the row's attachment)` of each
/// call — an empty error code for a success.
async fn sequence(
    emulator: &Emulator,
    rows: &[Row],
    fixture: &Fixture,
    token: Option<&str>,
) -> Vec<(String, u16, String, Option<String>)> {
    let mut answers = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let subject = fixture.subjects[index].clone();
        let (target_network, target) = fixture.target(row, &subject);
        for (step, network, attachment, label) in [
            ("the first call", fixture.core_network.as_str(), &subject, &row.first),
            ("the identical retry", fixture.core_network.as_str(), &subject, &row.first),
            ("the changed request", target_network, &target, &row.second),
        ] {
            let (status, body) = put(
                emulator,
                put_body(network, attachment, label, token),
            )
            .await;
            let code = if (200..300).contains(&status) {
                String::new()
            } else {
                error_code(&body).to_string()
            };
            if token == Some("") {
                let message = message_of(&body);
                assert!(
                    !message.contains("client token"),
                    "case {:?} / {step}: an empty token is no token, so no answer may refuse one, got {message:?}",
                    row.name
                );
            }
            let stored = stored_label(
                emulator,
                row.name,
                fixture.core_network.as_str(),
                &subject,
            )
            .await;
            answers.push((format!("{} / {step}", row.name), status, code, stored));
        }
    }
    answers
}

#[tokio::test]
async fn an_empty_client_token_answers_exactly_what_no_client_token_answers() {
    let rows = rows();

    let empty_emulator = Emulator::new();
    let empty_fixture = fixture(&empty_emulator, &rows).await;
    let empty = sequence(&empty_emulator, &rows, &empty_fixture, Some("")).await;

    let absent_emulator = Emulator::new();
    let absent_fixture = fixture(&absent_emulator, &rows).await;
    let absent = sequence(&absent_emulator, &rows, &absent_fixture, None).await;

    assert_eq!(
        empty, absent,
        "an empty ClientToken must answer exactly what no ClientToken member answers"
    );
}

#[tokio::test]
async fn a_label_written_under_another_token_is_that_token_s_to_replay() {
    let emulator = Emulator::new();
    let rows = rows();
    let fixture = fixture(&emulator, &rows).await;
    let case = "a second token over the same attachment";
    let subject = fixture.subjects[0].clone();

    put_ok(
        &emulator,
        case,
        &fixture.core_network,
        &subject,
        "blue",
        Some("first-token"),
    )
    .await;

    // A DIFFERENT token is a different request, not a retry of one: it applies its own
    // label, which is how a caller changes the label an attachment carries.
    put_ok(
        &emulator,
        case,
        &fixture.core_network,
        &subject,
        "red",
        Some("second-token"),
    )
    .await;
    assert_eq!(
        stored_label(&emulator, case, &fixture.core_network, &subject).await,
        Some("red".to_string()),
        "case {case:?}: a call under another token must store its own label"
    );

    // That second token now owns the label, so IT is the one a different request under it is
    // refused against.
    let (status, refused) = put(
        &emulator,
        put_body(&fixture.core_network, &subject, "blue", Some("second-token")),
    )
    .await;
    assert_eq!(
        status, 409,
        "case {case:?}: expected 409, got {status} with {refused}"
    );
    assert_eq!(
        message_of(&refused),
        format!(
            "client token {:?} already applied routing policy label {:?} to attachment {subject}, with a different request",
            "second-token", "red"
        ),
        "case {case:?}: wrong refusal in {refused}"
    );
    assert_eq!(
        stored_label(&emulator, case, &fixture.core_network, &subject).await,
        Some("red".to_string()),
        "case {case:?}: a refused call must leave the stored label alone"
    );
}

#[tokio::test]
async fn removing_the_label_releases_the_token_that_wrote_it() {
    let emulator = Emulator::new();
    let rows = rows();
    let fixture = fixture(&emulator, &rows).await;
    let case = "a token whose label was removed";
    let subject = fixture.subjects[0].clone();
    let token = "removal-token";

    put_ok(
        &emulator,
        case,
        &fixture.core_network,
        &subject,
        "blue",
        Some(token),
    )
    .await;

    let (status, removed) = emulator
        .delete(&remove_uri(&fixture.core_network, &subject))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: RemoveAttachmentRoutingPolicyLabel expected 200, got {status} with {removed}"
    );
    assert_eq!(
        stored_label(&emulator, case, &fixture.core_network, &subject).await,
        None,
        "case {case:?}: the removal must leave the attachment with no label"
    );

    // Nothing is stored under that token any more, so a call carrying it is a fresh request
    // whatever label it asks for, rather than a conflict with a label the store no longer
    // holds.
    put_ok(
        &emulator,
        case,
        &fixture.core_network,
        &subject,
        "green",
        Some(token),
    )
    .await;
    assert_eq!(
        stored_label(&emulator, case, &fixture.core_network, &subject).await,
        Some("green".to_string()),
        "case {case:?}: the call after the removal must store its label"
    );
}

#[tokio::test]
async fn a_spent_token_leaves_a_not_found_refusal_a_not_found_refusal() {
    let emulator = Emulator::new();
    let rows = rows();
    let fixture = fixture(&emulator, &rows).await;
    let subject = fixture.subjects[0].clone();
    let token = "spent-token";

    put_ok(
        &emulator,
        "the call that spends the token",
        &fixture.core_network,
        &subject,
        "blue",
        Some(token),
    )
    .await;

    // Each row: a request carrying the spent token and addressing a resource this account
    // holds nothing for, with the resource its refusal has to identify. The resources are
    // resolved before the token, so what is answered is the not-found refusal rather than
    // the token's conflict.
    for (case, core_network_id, attachment, resource_id, resource_type) in [
        (
            "a core network id nothing was minted for",
            "core-network-00000000000000099".to_string(),
            subject.clone(),
            "core-network-00000000000000099".to_string(),
            CORE_NETWORK,
        ),
        (
            "an attachment id nothing was minted for",
            fixture.core_network.clone(),
            "attachment-00000000000000099".to_string(),
            "attachment-00000000000000099".to_string(),
            ATTACHMENT,
        ),
        (
            "an attachment of another core network",
            fixture.core_network.clone(),
            fixture.elsewhere.clone(),
            fixture.elsewhere.clone(),
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
                Some(token),
            )),
            &resource_id,
            resource_type,
        )
        .await;
    }

    // None of those refusals touched the label the token wrote, so it still replays.
    assert_eq!(
        stored_label(
            &emulator,
            "the labelled attachment",
            &fixture.core_network,
            &subject
        )
        .await,
        Some("blue".to_string()),
        "a refused call must store nothing"
    );
}
