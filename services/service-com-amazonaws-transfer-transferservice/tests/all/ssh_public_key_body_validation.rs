// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what the service accepts as an `SshPublicKeyBody`.
//!
//! RULE under test: the member's modelled pattern admits every `ssh-<name>` and
//! `ecdsa-<name>` label and a payload group whose every part is optional, so two things
//! are the SERVICE's to check and are refused with the modelled
//! `InvalidRequestException` (HTTP 400) before anything is stored:
//!
//! 1. the ALGORITHM must be one of the key types the member documents the service
//!    accepting — "Transfer Family accepts RSA, ECDSA, and ED25519 keys"
//!    (`smithy_query(query="doc",shape="ImportSshPublicKeyRequest",member="SshPublicKeyBody")`),
//!    i.e. the OpenSSH names `ssh-rsa`, `ssh-ed25519` and the `ecdsa-` family; and
//! 2. the base64 PAYLOAD after it must be present and decodable — a payload the modelled
//!    pattern admits but that cannot be whole bytes (one character, or a length one past
//!    a whole group) carries no key material.
//!
//! Nothing else is checked: an accepted body is stored VERBATIM without its key material
//! being parsed, so material truncated mid-key, material belonging to another algorithm
//! than the label names, and an unpadded payload are all accepted and read back byte for
//! byte.
//!
//! The rule is about the MEMBER, so every row is driven through BOTH writes that take it
//! (`CreateUser` and `ImportSshPublicKey`), and the rows differ along the algorithm name
//! (each accepted type, each curve of the ECDSA family, labels the pattern admits that
//! name no accepted type) and along the payload (padded, unpadded, truncated, absent, a
//! single character, and one character past a whole group).
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{is_modelled_key_id, member_of, Emulator, ROLE};

/// The refusal wording the service answers for a body it does not accept.
const UNSUPPORTED: &str = "Unsupported or invalid SSH public key format";

/// (case, the body, whether the service stores it)
type Row = (&'static str, &'static str, bool);

/// The rows: the two conditions crossed with the classes each generalises over.
fn rows() -> Vec<Row> {
    vec![
        // Accepted: each documented key type, under its OpenSSH name.
        ("an RSA key", "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==", true),
        (
            "an ED25519 key, whose payload carries no padding",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAbCdEfGhIjKlMnOpQrStUvWxYz0123456789ab",
            true,
        ),
        (
            "an ECDSA key on the P-256 curve",
            "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTY=",
            true,
        ),
        (
            "an ECDSA key on the P-384 curve",
            "ecdsa-sha2-nistp384 AAAAE2VjZHNhLXNoYTItbmlzdHAzODQ=",
            true,
        ),
        (
            "an ECDSA key on the P-521 curve",
            "ecdsa-sha2-nistp521 AAAAE2VjZHNhLXNoYTItbmlzdHA1MjE=",
            true,
        ),
        // Accepted: the key MATERIAL is not parsed, so these are stored as given.
        (
            "a payload truncated mid-key, which is still bytes",
            "ssh-rsa AAAAB3NzaC1yc2EAAAADAQAB",
            true,
        ),
        (
            "an RSA payload under an ED25519 label",
            "ssh-ed25519 AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
            true,
        ),
        (
            "a key with a comment after its material",
            "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ== user@host",
            true,
        ),
        // Refused: an algorithm the service does not accept, though the pattern admits it.
        ("a DSA key", "ssh-dss AAAAB3NzaC1kc3MAAAABIQ==", false),
        (
            "an ssh- label naming no key type at all",
            "ssh-foo AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
            false,
        ),
        (
            "an ssh-rsa-like label that is not the RSA key type",
            "ssh-rsa-v2 AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
            false,
        ),
        // Refused: no key material at all.
        ("an accepted algorithm with no payload", "ssh-rsa ", false),
        (
            "an accepted algorithm with a payload of one base64 character",
            "ssh-rsa A",
            false,
        ),
        (
            "a payload whose length is one character past a whole base64 group",
            "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQAAA",
            false,
        ),
    ]
}

/// The bodies `DescribeUser` reports for `user_name`'s stored keys, in order.
async fn stored_bodies(emulator: &Emulator, server_id: &str, user_name: &str) -> Vec<String> {
    let described = emulator
        .call(
            "DescribeUser",
            json!({"ServerId": server_id, "UserName": user_name}),
        )
        .await;
    assert_eq!(
        described.status, 200,
        "DescribeUser {user_name}: expected 200, got {} with body {}",
        described.status, described.body
    );
    member_of(described.get("User"), "SshPublicKeys")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|key| {
            member_of(key, "SshPublicKeyBody")
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Assert `answer` is the modelled refusal for a body the service does not accept.
fn assert_refused(case: &str, operation: &str, answer: &crate::harness::Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?} at {operation}: expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "InvalidRequestException",
        "case {case:?} at {operation}: expected InvalidRequestException, got body {}",
        answer.body
    );
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        message, UNSUPPORTED,
        "case {case:?} at {operation}: unexpected refusal wording"
    );
}

#[tokio::test]
async fn an_import_stores_a_supported_key_and_refuses_one_the_service_does_not_accept() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    for (index, (case, body, accepted)) in rows().into_iter().enumerate() {
        // One user per row, so each row's key set is its own and a refusal is visible as
        // "the set did not grow".
        let user_name = format!("u-import-{index}");
        emulator.create_user(&server_id, &user_name).await;
        let before = stored_bodies(&emulator, &server_id, &user_name).await;

        let answer = emulator
            .call(
                "ImportSshPublicKey",
                json!({
                    "ServerId": &server_id,
                    "UserName": &user_name,
                    "SshPublicKeyBody": body,
                }),
            )
            .await;

        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the import to be served, got {} with body {}",
                answer.status, answer.body
            );
            let key_id = answer.get("SshPublicKeyId").as_str().unwrap_or_default();
            assert!(
                is_modelled_key_id(key_id),
                "case {case:?}: expected a minted SshPublicKeyId, got {key_id:?}"
            );
            let after = stored_bodies(&emulator, &server_id, &user_name).await;
            assert_eq!(
                after,
                vec![body.trim_end().to_string()],
                "case {case:?}: expected the accepted body to be stored verbatim"
            );
        } else {
            assert_refused(case, "ImportSshPublicKey", &answer);
            let after = stored_bodies(&emulator, &server_id, &user_name).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused import must leave the key set alone"
            );
        }
    }
}

#[tokio::test]
async fn a_create_applies_the_same_rule_to_the_key_body_it_carries() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    for (index, (case, body, accepted)) in rows().into_iter().enumerate() {
        let user_name = format!("u-create-{index}");
        let answer = emulator
            .call(
                "CreateUser",
                json!({
                    "ServerId": &server_id,
                    "UserName": &user_name,
                    "Role": ROLE,
                    "SshPublicKeyBody": body,
                }),
            )
            .await;

        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the create to be served, got {} with body {}",
                answer.status, answer.body
            );
            let stored = stored_bodies(&emulator, &server_id, &user_name).await;
            assert_eq!(
                stored,
                vec![body.trim_end().to_string()],
                "case {case:?}: expected the accepted body to be stored verbatim"
            );
        } else {
            assert_refused(case, "CreateUser", &answer);
            // A refused create persists no user at all, so the read of it is refused.
            let described = emulator
                .call(
                    "DescribeUser",
                    json!({"ServerId": &server_id, "UserName": &user_name}),
                )
                .await;
            assert_eq!(
                described.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: a refused create must persist no user, got {} with body {}",
                described.status,
                described.body
            );
        }
    }
}

/// RULE (the same rule, as a client sees it twice): an accepted key is the user's and a
/// refused one changes nothing, so a user whose first import was refused still takes the
/// next accepted one, and the key set a reader reports is exactly the accepted bodies in
/// the order they were imported.
#[tokio::test]
async fn a_refused_import_does_not_stop_the_next_accepted_one() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    let user_name = "u-mixed";
    emulator.create_user(&server_id, user_name).await;

    // An alternating sequence, so no row's outcome can be read off its position.
    let sequence: Vec<Row> = vec![
        ("a DSA key first", "ssh-dss AAAAB3NzaC1kc3MAAAABIQ==", false),
        (
            "an accepted RSA key",
            "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==",
            true,
        ),
        ("an empty payload", "ssh-rsa ", false),
        (
            "an accepted ECDSA key",
            "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTY=",
            true,
        ),
        ("a one-character payload", "ssh-ed25519 A", false),
        (
            "an accepted ED25519 key",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAbCdEfGhIjKlMnOpQrStUvWxYz0123456789ab",
            true,
        ),
    ];

    let mut expected: Vec<String> = Vec::new();
    for (case, body, accepted) in sequence {
        let answer = emulator
            .call(
                "ImportSshPublicKey",
                json!({
                    "ServerId": &server_id,
                    "UserName": user_name,
                    "SshPublicKeyBody": body,
                }),
            )
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the import to be served, got {} with body {}",
                answer.status, answer.body
            );
            expected.push(body.trim_end().to_string());
        } else {
            assert_refused(case, "ImportSshPublicKey", &answer);
        }
        let stored = stored_bodies(&emulator, &server_id, user_name).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the key set to hold exactly the accepted bodies"
        );
    }
}

/// The modelled pattern admits leading whitespace, so a body carrying it is the same key
/// to the algorithm check as one without — and is stored as submitted apart from the
/// trailing whitespace this service strips.
#[tokio::test]
async fn a_body_with_the_leading_whitespace_the_pattern_admits_is_accepted() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    // (case, the submitted body)
    let rows: Vec<(&str, Value)> = vec![
        ("a leading space", json!(" ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==")),
        (
            "a leading newline and spaces",
            json!("\n  ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ=="),
        ),
        (
            "a leading tab with trailing whitespace too",
            json!("\tssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==  \n"),
        ),
    ];

    for (index, (case, body)) in rows.into_iter().enumerate() {
        let user_name = format!("u-space-{index}");
        emulator.create_user(&server_id, &user_name).await;
        let answer = emulator
            .call(
                "ImportSshPublicKey",
                json!({
                    "ServerId": &server_id,
                    "UserName": &user_name,
                    "SshPublicKeyBody": &body,
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the import to be served, got {} with body {}",
            answer.status, answer.body
        );
        let submitted = body.as_str().unwrap_or_default();
        let stored = stored_bodies(&emulator, &server_id, &user_name).await;
        assert_eq!(
            stored,
            vec![submitted.trim_end().to_string()],
            "case {case:?}: expected the submitted body stored with its trailing whitespace \
             removed"
        );
    }
}
