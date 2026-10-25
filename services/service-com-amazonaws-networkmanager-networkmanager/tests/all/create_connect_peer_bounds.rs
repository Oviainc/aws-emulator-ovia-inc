// SPDX-License-Identifier: CC0-1.0

//! What makes a `CreateConnectPeer` store a peer AT ALL, where that question is about how
//! MANY peers one Connect attachment carries, the `Tags` the request carries and the
//! modelled `@idempotencyToken` — the bounds of the create, as distinct from what a stored
//! peer reports, which `crate::connect_peer_lifecycle` covers. Driven end to end over HTTP
//! through the real router (the harness lives in `crate::global_network_lifecycle`), over
//! the Connect attachments `crate::connect_peer_lifecycle::subject` builds.
//!
//! The rules under test, each asserted over a table or a sequence that spans it:
//!
//! * a Connect peer belongs to ONE Connect attachment, and an attachment carries a bounded
//!   number of them: creates up to the bound are accepted and read back through
//!   `ListConnectPeers`, the create one past it is refused with the modelled
//!   `ValidationException` naming the member that asked, and the bound is per attachment, so
//!   a sibling attachment of the same core network still accepts a peer;
//! * the `Tags` a create carries obey the same published tag rules a later `TagResource`
//!   does, refused through the error class this operation models — it models no
//!   `ServiceQuotaExceededException` (`smithy_query query="operation"
//!   operation="CreateConnectPeer"`), so a request past the per-resource tag quota is the
//!   modelled `ValidationException` with the `Other` reason, while a tag no resource may
//!   carry is a field failure naming the member that carried it;
//! * the `@idempotencyToken` joins two creates only where it carries characters: a token of
//!   no characters is no token at all, so an identical request under it mints a second peer,
//!   while a token that does carry characters makes the identical request a retry of the
//!   peer it first minted — and resolves it even once the attachment is at its peer bound,
//!   since a retry asks for nothing new.
//!
//! Every refusal is also asserted to store NOTHING: the peers `ListConnectPeers` reports
//! afterwards are exactly the accepted ones. Nothing reaches into `crate::state` — every
//! peer is created, listed and read back through an operation.

use serde_json::{json, Value};

use crate::connect_peer_lifecycle::{create_connect_peer, subject, Subject, CONNECT_PEER};
use crate::global_network_lifecycle::{error_code, expected_arn, ids_of, Emulator};
use crate::resource_tagging::{list_tags, tag, MAX_KEY_CHARS, MAX_TAGS, MAX_VALUE_CHARS};

/// How many Connect peers one Connect attachment may carry: the bound
/// `state::MAX_CONNECT_PEERS_PER_ATTACHMENT` fixes, which is what keeps repeated creates
/// from growing the store without end.
const MAX_PEERS_PER_ATTACHMENT: usize = 100;

/// The widest `ClientToken` the model admits (`ClientToken`, len 0..256: `smithy_query
/// query="operation" operation="CreateConnectPeer"`).
const MAX_TOKEN_CHARS: usize = 256;

/// The `Reason` a `ValidationException` of this service carries when an input member is at
/// fault, and the one it carries when none is — the request is invalid against a count or
/// against stored state (`smithy_query shape=ValidationExceptionReason`).
const REASON_FIELD: &str = "FieldValidationFailed";
/// See [`REASON_FIELD`].
const REASON_OTHER: &str = "Other";

/// The `CreateConnectPeer` member that names the attachment the peer hangs off, as this
/// service reports it in a refusal.
const CONNECT_ATTACHMENT_ID: &str = "connectAttachmentId";
/// The member the tag rules blame, as `TagResource` blames it too.
const TAGS: &str = "Tags";

/// A distinct appliance address per row, out of the block reserved for benchmarking
/// (198.18.0.0/15), so no two peers of one attachment are asked for at the same address.
fn peer_address(index: usize) -> String {
    format!("198.18.{}.{}", index / 256, index % 256)
}

/// The `CreateConnectPeer` body asking for the peer `index` on `attachment`.
fn create_request(attachment: &str, index: usize) -> Value {
    json!({
        "ConnectAttachmentId": attachment,
        "PeerAddress": peer_address(index),
    })
}

/// `body` with `member` set to `value`.
fn with(mut body: Value, member: &str, value: Value) -> Value {
    let object = body
        .as_object_mut()
        .expect("the request body of every row here is a JSON object");
    object.insert(member.to_string(), value);
    body
}

/// A create that must be accepted, answering the minted peer id.
async fn create_ok(emulator: &Emulator, case: &str, body: Value) -> String {
    let (status, created) = create_connect_peer(emulator, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConnectPeer expected 200, got {status} with {created}"
    );
    created["ConnectPeer"]["ConnectPeerId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no ConnectPeer.ConnectPeerId in {created}"))
        .to_string()
}

/// A create that must be refused with `status` and `code`, answering the error body.
async fn create_refused(
    emulator: &Emulator,
    case: &str,
    body: Value,
    status: u16,
    code: &str,
) -> Value {
    let (actual, refusal) = create_connect_peer(emulator, body).await;
    assert_eq!(
        actual, status,
        "case {case:?}: expected {status}, got {actual} with {refusal}"
    );
    assert_eq!(
        error_code(&refusal),
        code,
        "case {case:?}: expected {code}, got {refusal}"
    );
    refusal
}

/// The peer ids `ListConnectPeers` reports for `attachment` — the stored inventory of that
/// attachment, read through an operation. The listing names no `MaxResults`, so it carries
/// every selected peer on one page.
async fn stored_ids(emulator: &Emulator, attachment: &str) -> Vec<String> {
    let (status, body) = emulator
        .get(&format!("/connect-peers?connectAttachmentId={attachment}"))
        .await;
    assert_eq!(
        status, 200,
        "ListConnectPeers expected 200, got {status} with {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "a listing of one attachment's peers must fit one unpaged page, got {body}"
    );
    ids_of(&body, "ConnectPeers", "ConnectPeerId")
}

/// The `ConnectPeer` `GetConnectPeer` answers for `peer`.
async fn read_back(emulator: &Emulator, case: &str, peer: &str) -> Value {
    let (status, body) = emulator.get(&format!("/connect-peers/{peer}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConnectPeer expected 200, got {status} with {body}"
    );
    body["ConnectPeer"].clone()
}

/// The ids of `expected`, sorted, beside the ids `ListConnectPeers` reports for
/// `attachment`, sorted — so a caller compares two inventories in one assertion.
async fn inventories(
    emulator: &Emulator,
    attachment: &str,
    expected: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut listed = stored_ids(emulator, attachment).await;
    listed.sort();
    let mut wanted = expected.to_vec();
    wanted.sort();
    (listed, wanted)
}

/// Build the subject every test here hangs its peers off: a global network, a core network
/// and the Connect attachments of `crate::connect_peer_lifecycle::TRANSPORTS`.
async fn built(emulator: &Emulator, name: &str) -> Subject {
    subject(emulator, name, 0).await
}

#[tokio::test]
async fn an_attachment_carries_a_bounded_number_of_peers_and_the_bound_is_its_own() {
    let emulator = Emulator::new();
    let subject = built(&emulator, "peer bounds").await;
    let attachment = subject.attachments[0].clone();
    let sibling = subject.attachments[1].clone();

    // Up to the bound EXACTLY: every create is accepted and mints its own peer. The last one
    // carries an idempotency token, so the retry below is a request this attachment already
    // served.
    let token = "t".repeat(MAX_TOKEN_CHARS);
    let mut accepted: Vec<String> = Vec::new();
    for index in 0..MAX_PEERS_PER_ATTACHMENT {
        let case = if index + 1 == MAX_PEERS_PER_ATTACHMENT {
            "the create at the bound"
        } else {
            "a create below the bound"
        };
        let mut request = create_request(&attachment, index);
        if index + 1 == MAX_PEERS_PER_ATTACHMENT {
            request = with(request, "ClientToken", json!(token));
        }
        let id = create_ok(&emulator, case, request).await;
        assert!(
            !accepted.contains(&id),
            "case {case:?}: every accepted create mints its own peer, got {id} twice"
        );
        accepted.push(id);
    }
    let (listed, wanted) = inventories(&emulator, &attachment, &accepted).await;
    assert_eq!(
        listed, wanted,
        "case \"at the bound\": expected the {} accepted peers, got {listed:?}",
        accepted.len()
    );

    // One past the bound: the COUNT this attachment would hold is what cannot be stored, so
    // the refusal carries the other reason and names the member that asked for it.
    let refusal = create_refused(
        &emulator,
        "a create one past the bound",
        create_request(&attachment, MAX_PEERS_PER_ATTACHMENT),
        400,
        "ValidationException",
    )
    .await;
    assert_eq!(
        refusal["Reason"].as_str(),
        Some(REASON_OTHER),
        "case \"a create one past the bound\": expected the {REASON_OTHER} reason, got {refusal}"
    );
    assert_eq!(
        refusal["Fields"][0]["Name"].as_str(),
        Some(CONNECT_ATTACHMENT_ID),
        "case \"a create one past the bound\": the refusal must name the member that asked, got {refusal}"
    );
    let (listed, wanted) = inventories(&emulator, &attachment, &accepted).await;
    assert_eq!(
        listed, wanted,
        "case \"a create one past the bound\": a refused create stored a peer, got {listed:?}"
    );

    // A retry of a request this attachment already served asks for nothing new, so the token
    // resolves it even now that the attachment is full.
    let retried = create_ok(
        &emulator,
        "the tokened request retried at the bound",
        with(
            create_request(&attachment, MAX_PEERS_PER_ATTACHMENT - 1),
            "ClientToken",
            json!(token),
        ),
    )
    .await;
    assert_eq!(
        Some(&retried),
        accepted.last(),
        "case \"the tokened request retried at the bound\": a retry must answer the peer the first call minted, got {retried}"
    );
    let (listed, wanted) = inventories(&emulator, &attachment, &accepted).await;
    assert_eq!(
        listed, wanted,
        "case \"the tokened request retried at the bound\": a retry must not mint a second peer, got {listed:?}"
    );

    // The bound belongs to the attachment, not to the core network: a sibling Connect
    // attachment of the same core network still accepts a peer.
    let on_sibling = create_ok(
        &emulator,
        "a peer on a sibling attachment",
        create_request(&sibling, 0),
    )
    .await;
    let (listed, wanted) =
        inventories(&emulator, &sibling, std::slice::from_ref(&on_sibling)).await;
    assert_eq!(
        listed, wanted,
        "case \"a peer on a sibling attachment\": expected only {on_sibling}, got {listed:?}"
    );
    let stored = read_back(&emulator, "a peer on a sibling attachment", &on_sibling).await;
    assert_eq!(
        stored["ConnectAttachmentId"],
        json!(sibling),
        "case \"a peer on a sibling attachment\": the peer must report its own attachment, got {stored}"
    );
}

#[tokio::test]
async fn the_tags_a_create_carries_obey_the_published_tag_rules() {
    let emulator = Emulator::new();
    let subject = built(&emulator, "peer tags").await;
    let attachment = subject.attachments[2].clone();

    // Accepted rows: each published bound EXACTLY — a key of the maximum length, a value of
    // the maximum length, and the maximum number of distinct keys — stored and read back
    // both through `ListTagsForResource` and through the peer's own reader.
    let at_the_bound: Vec<(&str, Value)> = vec![
        (
            "a key of the maximum length",
            json!([tag(&"k".repeat(MAX_KEY_CHARS), "x")]),
        ),
        (
            "a value of the maximum length",
            json!([tag("k", &"v".repeat(MAX_VALUE_CHARS))]),
        ),
        (
            "the maximum number of distinct keys",
            json!((0..MAX_TAGS)
                .map(|index| tag(&format!("key-{index:02}"), "x"))
                .collect::<Vec<Value>>()),
        ),
        ("no tags at all", json!([])),
    ];
    let mut accepted: Vec<String> = Vec::new();
    for (row, (case, tags)) in at_the_bound.into_iter().enumerate() {
        let id = create_ok(
            &emulator,
            case,
            with(create_request(&attachment, row), "Tags", tags.clone()),
        )
        .await;
        let (status, listed) = list_tags(&emulator, &expected_arn(CONNECT_PEER, &id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource expected 200, got {status} with {listed}"
        );
        assert_eq!(
            listed, tags,
            "case {case:?}: expected the tags the create carried, got {listed}"
        );
        let stored = read_back(&emulator, case, &id).await;
        assert_eq!(
            stored["Tags"], tags,
            "case {case:?}: the peer's own reader must answer the same tags, got {stored}"
        );
        accepted.push(id);
    }

    // Refused rows: each ONE step outside a bound, plus both shapes of a key AWS reserves.
    // The per-resource tag quota is the count the request would leave rather than a member
    // at fault, so it carries the `Other` reason; every row names `Tags`.
    let refusals: Vec<(&str, Value, &str)> = vec![
        ("an empty key", json!([tag("", "x")]), REASON_FIELD),
        (
            "a key one character past the bound",
            json!([tag(&"k".repeat(MAX_KEY_CHARS + 1), "x")]),
            REASON_FIELD,
        ),
        (
            "a value one character past the bound",
            json!([tag("k", &"v".repeat(MAX_VALUE_CHARS + 1))]),
            REASON_FIELD,
        ),
        (
            "a key carrying the prefix AWS reserves",
            json!([tag("aws:cloudformation:stack-name", "x")]),
            REASON_FIELD,
        ),
        (
            "that reserved prefix in another case",
            json!([tag("AWS:billing", "x")]),
            REASON_FIELD,
        ),
        (
            "one distinct key past the maximum number",
            json!((0..=MAX_TAGS)
                .map(|index| tag(&format!("key-{index:02}"), "x"))
                .collect::<Vec<Value>>()),
            REASON_OTHER,
        ),
    ];
    for (row, (case, tags, reason)) in refusals.into_iter().enumerate() {
        let refusal = create_refused(
            &emulator,
            case,
            with(create_request(&attachment, 100 + row), "Tags", tags),
            400,
            "ValidationException",
        )
        .await;
        assert_eq!(
            refusal["Reason"].as_str(),
            Some(reason),
            "case {case:?}: expected the {reason} reason, got {refusal}"
        );
        assert_eq!(
            refusal["Fields"][0]["Name"].as_str(),
            Some(TAGS),
            "case {case:?}: the refusal must name the member that carried them, got {refusal}"
        );
    }

    // No refused create stored a peer: the inventory is exactly the accepted rows.
    let (listed, wanted) = inventories(&emulator, &attachment, &accepted).await;
    assert_eq!(
        listed, wanted,
        "a create refused over its tags stored a peer: expected {wanted:?}, got {listed:?}"
    );
}

#[tokio::test]
async fn a_client_token_with_no_characters_is_no_token() {
    let emulator = Emulator::new();
    let subject = built(&emulator, "peer tokens").await;
    let attachment = subject.attachments[3].clone();
    let empty = json!("");

    // The same request twice under the same EMPTY token: were the empty string taken for an
    // idempotency token, the second call would answer the first peer. It is no token, so each
    // create mints its own peer.
    let request = with(create_request(&attachment, 1), "ClientToken", empty);
    let first = create_ok(
        &emulator,
        "the first create under an empty token",
        request.clone(),
    )
    .await;
    let second = create_ok(
        &emulator,
        "the same request repeated under the same empty token",
        request,
    )
    .await;
    assert_ne!(
        first, second,
        "an empty client token must not make the second create a retry, got {first} twice"
    );

    // A token that DOES carry characters makes the identical request a retry of the peer it
    // first minted rather than a third peer.
    let token = "token-of-this-caller";
    let tokened = with(
        create_request(&attachment, 2),
        "ClientToken",
        json!(token),
    );
    let third = create_ok(&emulator, "a create under a token", tokened.clone()).await;
    let retried = create_ok(&emulator, "the identical request retried", tokened.clone()).await;
    assert_eq!(
        retried, third,
        "a retry under the same token minted a second peer, got {retried} after {third}"
    );

    // The same token asking for something else — another appliance address — is the
    // service's conflict, naming the peer that token already minted, and stores nothing.
    let refusal = create_refused(
        &emulator,
        "the same token asking for another peer address",
        with(tokened, "PeerAddress", json!(peer_address(3))),
        409,
        "ConflictException",
    )
    .await;
    assert_eq!(
        refusal["ResourceId"],
        json!(third),
        "the conflict must name the peer the token already minted, got {refusal}"
    );
    assert_eq!(
        refusal["ResourceType"],
        json!(CONNECT_PEER),
        "the conflict must name the resource type, got {refusal}"
    );

    let (listed, wanted) = inventories(
        &emulator,
        &attachment,
        &[first.clone(), second.clone(), third.clone()],
    )
    .await;
    assert_eq!(
        listed, wanted,
        "a retry or a refused create changed the stored inventory: expected {wanted:?}, got {listed:?}"
    );
}
