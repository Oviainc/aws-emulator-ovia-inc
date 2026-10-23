// SPDX-License-Identifier: CC0-1.0

//! What this service refuses to STORE, and what it normalises before storing it.
//!
//! Three rules, each over a TABLE whose rows differ along what the rule generalises
//! over — the document's syntax, the protocol/identity-provider combination, the
//! shape of the trailing whitespace:
//!
//! 1. A session `Policy` that is not a JSON document is refused with
//!    `InvalidRequestException` (HTTP 400) and never stored; one that parses is
//!    stored verbatim.
//! 2. `Protocols` naming FTP or FTPS is refused on the IDENTITY-PROVIDER half of the
//!    documented requirement before the endpoint-type half, so the combination that
//!    satisfies the endpoint rule and not the provider rule is refused too.
//! 3. The `SshPublicKeyBody` a write stores is the submitted one with trailing
//!    whitespace removed and every other byte untouched, so a reader reports what
//!    the service holds.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{is_modelled_key_id, member_of, Emulator, ROLE};

/// A VPC id for the rows that host an endpoint in a VPC.
const VPC_ID: &str = "vpc-0123456789abcdef0";

/// Satisfies `Function`'s modelled `len 1..170`, pattern `^arn:[a-z-]+:lambda:.*$`.
const LAMBDA_IDP: &str = "arn:aws:lambda:us-east-1:000000000000:function:idp";

/// Satisfies `DirectoryId`'s modelled `len 12..12`, pattern `^d-[0-9a-f]{10}$`.
const DIRECTORY_ID: &str = "d-0123456789";

/// An ACM certificate ARN for the FTPS rows; `Certificate` is `len 0..1600`.
const CERTIFICATE: &str = "arn:aws:acm:us-east-1:000000000000:certificate/abcd";

/// The key material every normalisation row submits, before any trailing whitespace
/// is appended. Satisfies `SshPublicKeyBody`'s modelled pattern.
const KEY_MATERIAL: &str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ==";

// ---------------------------------------------------------------------------
// 1. A session policy is parsed before it is stored.
// ---------------------------------------------------------------------------

/// RULE: a `Policy` that is not a JSON document is refused with the modelled
/// `InvalidRequestException` (HTTP 400) and nothing is written; a document that
/// parses is stored exactly as submitted and read back by `DescribeUser`.
///
/// The rows differ along the document's syntax — a truncated statement list, an
/// unterminated string, a bare word, an empty object, a full policy, a document that
/// is valid JSON of another shape — and the table is driven through BOTH writes that
/// take a policy (`CreateUser` and `UpdateUser`), because the rule is about the
/// member, not about one operation. The empty string, which the member's
/// `len 0..2048` admits, names no policy and is accepted.
#[tokio::test]
async fn a_session_policy_is_refused_unless_it_is_a_json_document() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    // (case, policy, is it a JSON document)
    let rows: Vec<(&str, &str, bool)> = vec![
        (
            "a statement list that is not closed",
            r#"{"Version": "2012-10-17", "Statement": [ this is not json"#,
            false,
        ),
        (
            "an unterminated string",
            r#"{"Version": "2012-10-17", "Statement": "#,
            false,
        ),
        ("a bare word", "not-json", false),
        ("a trailing comma", r#"{"Version": "2012-10-17",}"#, false),
        ("an empty object", "{}", true),
        (
            "a full policy document",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*"}]}"#,
            true,
        ),
        ("valid JSON of another shape", "[]", true),
        ("the empty string, which names no policy", "", true),
    ];

    for (index, (case, policy, parses)) in rows.into_iter().enumerate() {
        let user_name = format!("u-policy-{index}");

        // The create: the policy is refused before the user exists.
        let answer = emulator
            .call(
                "CreateUser",
                json!({
                    "ServerId": &server_id,
                    "UserName": &user_name,
                    "Role": ROLE,
                    "Policy": policy,
                }),
            )
            .await;
        if parses {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the create to be served, got {} with body {}",
                answer.status, answer.body
            );
            let described = emulator
                .call(
                    "DescribeUser",
                    json!({"ServerId": &server_id, "UserName": &user_name}),
                )
                .await;
            assert_eq!(
                member_of(described.get("User"), "Policy"),
                &json!(policy),
                "case {case:?}: the stored policy must be the submitted document"
            );
        } else {
            assert_policy_refusal(case, &answer);
            let described = emulator
                .call(
                    "DescribeUser",
                    json!({"ServerId": &server_id, "UserName": &user_name}),
                )
                .await;
            assert_eq!(
                described.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: a refused create must persist no user, got {}",
                described.body
            );
        }

        // The update: the same rule on a user that already exists, whose stored
        // policy a refused update must not change.
        let updated_user = "u-policy-update";
        if emulator
            .call(
                "DescribeUser",
                json!({"ServerId": &server_id, "UserName": updated_user}),
            )
            .await
            .status
            != 200
        {
            emulator.create_user(&server_id, updated_user).await;
        }
        let before = emulator
            .call(
                "DescribeUser",
                json!({"ServerId": &server_id, "UserName": updated_user}),
            )
            .await;
        let stored_before = member_of(before.get("User"), "Policy").clone();
        let answer = emulator
            .call(
                "UpdateUser",
                json!({
                    "ServerId": &server_id,
                    "UserName": updated_user,
                    "Policy": policy,
                }),
            )
            .await;
        let after = emulator
            .call(
                "DescribeUser",
                json!({"ServerId": &server_id, "UserName": updated_user}),
            )
            .await;
        let stored_after = member_of(after.get("User"), "Policy").clone();
        if parses {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the update to be served, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                stored_after,
                json!(policy),
                "case {case:?}: the update must store the submitted document"
            );
        } else {
            assert_policy_refusal(case, &answer);
            assert_eq!(
                stored_after, stored_before,
                "case {case:?}: a refused update must leave the stored policy alone"
            );
        }
    }
}

/// RULE (the same rule, the third write that takes a policy document): `CreateAccess`
/// parses its `Policy` before it stores it too, and a document that does not parse is
/// refused with `InvalidRequestException` (400) with nothing written — while one that
/// parses is stored and read back by `DescribeAccess`.
///
/// The rows are the same syntax classes as above, so what differs from that table is
/// the OPERATION the rule is asserted through, not the rule.
#[tokio::test]
async fn create_access_parses_its_policy_before_storing_it() {
    let emulator = Emulator::new();
    // An access names a group in the server's DIRECTORY, so the server this table
    // writes to has a directory identity provider.
    let server_id = emulator
        .create_server(json!({
            "IdentityProviderType": "AWS_DIRECTORY_SERVICE",
            "IdentityProviderDetails": {"DirectoryId": DIRECTORY_ID},
        }))
        .await;

    // (case, policy, is it a JSON document)
    let rows: Vec<(&str, &str, bool)> = vec![
        (
            "a statement list that is not closed",
            r#"{"Version": "2012-10-17", "Statement": [ this is not json"#,
            false,
        ),
        ("a bare word", "not-json", false),
        (
            "a full policy document",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*"}]}"#,
            true,
        ),
        ("the empty string, which names no policy", "", true),
    ];

    for (index, (case, policy, parses)) in rows.into_iter().enumerate() {
        // `ExternalId` is modelled `len 1..256`, pattern `^S-1-[\d-]+$`: a Windows SID.
        let external_id = format!("S-1-1-11-1111111111-2222222222-3333333333-{}", 1000 + index);
        let answer = emulator
            .call(
                "CreateAccess",
                json!({
                    "ServerId": &server_id,
                    "ExternalId": &external_id,
                    "Role": ROLE,
                    "Policy": policy,
                }),
            )
            .await;
        let described = emulator
            .call(
                "DescribeAccess",
                json!({"ServerId": &server_id, "ExternalId": &external_id}),
            )
            .await;
        if parses {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the create to be served, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                member_of(described.get("Access"), "Policy"),
                &json!(policy),
                "case {case:?}: the stored policy must be the submitted document, got {}",
                described.body
            );
        } else {
            assert_policy_refusal(case, &answer);
            assert_eq!(
                described.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: a refused create must persist no access, got {}",
                described.body
            );
        }
    }
}

/// Assert `answer` is the modelled refusal for a policy that is not a JSON document.
fn assert_policy_refusal(case: &str, answer: &crate::harness::Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "InvalidRequestException",
        "case {case:?}: expected InvalidRequestException, got body {}",
        answer.body
    );
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        message, "Policy is not a valid JSON document",
        "case {case:?}: the refusal must name the member and what it failed"
    );
}

// ---------------------------------------------------------------------------
// 2. FTP/FTPS is an identity-provider rule first, an endpoint rule second.
// ---------------------------------------------------------------------------

/// RULE: `Protocols` naming FTP or FTPS requires BOTH an identity provider that can
/// serve them (`AWS_DIRECTORY_SERVICE`, `AWS_LAMBDA` or `API_GATEWAY`) and
/// `EndpointType=VPC`, and the provider half is evaluated FIRST — so the
/// service-managed provider is refused for the provider's sake whatever the endpoint
/// type, and the combination that satisfies only the endpoint rule is refused too.
///
/// The rows cross the whole `IdentityProviderType` value set (`[SERVICE_MANAGED,
/// API_GATEWAY, AWS_DIRECTORY_SERVICE, AWS_LAMBDA]`, plus the absent member that
/// resolves to `SERVICE_MANAGED`) with the endpoint types and with which of FTP and
/// FTPS is named.
#[tokio::test]
async fn ftp_is_refused_on_the_identity_provider_before_the_endpoint_type() {
    let emulator = Emulator::new();
    // One row names an API_GATEWAY provider, whose invocation role this service
    // verifies; the row gives none, so there is nothing to verify.
    let vpc = json!({"VpcId": VPC_ID});

    // (case, request, the refusal fragments, or None to accept)
    let rows: Vec<(&str, Value, Option<Vec<&str>>)> = vec![
        (
            "FTP, service-managed, VPC endpoint: the endpoint rule is satisfied and \
             the provider rule is not",
            json!({"Protocols": ["FTP"], "EndpointType": "VPC", "EndpointDetails": vpc}),
            Some(vec![
                "FTP and FTPS protocols unsupported",
                "IdentityProviderType SERVICE_MANAGED",
            ]),
        ),
        (
            "FTP, service-managed, public endpoint: neither half is satisfied, and \
             the provider half is the one answered",
            json!({"Protocols": ["FTP"], "EndpointType": "PUBLIC"}),
            Some(vec![
                "FTP and FTPS protocols unsupported",
                "IdentityProviderType SERVICE_MANAGED",
            ]),
        ),
        (
            "FTP with no IdentityProviderType at all, which resolves to SERVICE_MANAGED",
            json!({"Protocols": ["SFTP", "FTP"], "EndpointType": "VPC", "EndpointDetails": vpc}),
            Some(vec![
                "FTP and FTPS protocols unsupported",
                "IdentityProviderType SERVICE_MANAGED",
            ]),
        ),
        (
            "FTPS, service-managed, with a certificate and a VPC endpoint",
            json!({
                "Protocols": ["FTPS"],
                "Certificate": CERTIFICATE,
                "EndpointType": "VPC",
                "EndpointDetails": vpc,
            }),
            Some(vec![
                "FTP and FTPS protocols unsupported",
                "IdentityProviderType SERVICE_MANAGED",
            ]),
        ),
        (
            "FTP, AWS_LAMBDA provider, public endpoint: the provider rule is \
             satisfied, so the endpoint rule is the one answered",
            json!({
                "Protocols": ["FTP"],
                "IdentityProviderType": "AWS_LAMBDA",
                "IdentityProviderDetails": {"Function": LAMBDA_IDP},
                "EndpointType": "PUBLIC",
            }),
            Some(vec!["EndpointType", "Protocols includes FTP"]),
        ),
        (
            "FTPS, AWS_DIRECTORY_SERVICE provider, public endpoint",
            json!({
                "Protocols": ["FTPS"],
                "Certificate": CERTIFICATE,
                "IdentityProviderType": "AWS_DIRECTORY_SERVICE",
                "IdentityProviderDetails": {"DirectoryId": DIRECTORY_ID},
                "EndpointType": "PUBLIC",
            }),
            Some(vec!["EndpointType", "Protocols includes FTPS"]),
        ),
        (
            "FTP, AWS_DIRECTORY_SERVICE provider, VPC endpoint: both halves satisfied",
            json!({
                "Protocols": ["FTP"],
                "IdentityProviderType": "AWS_DIRECTORY_SERVICE",
                "IdentityProviderDetails": {"DirectoryId": DIRECTORY_ID},
                "EndpointType": "VPC",
                "EndpointDetails": vpc,
            }),
            None,
        ),
        (
            "FTP and FTPS, AWS_LAMBDA provider, VPC endpoint, with a certificate",
            json!({
                "Protocols": ["SFTP", "FTP", "FTPS"],
                "Certificate": CERTIFICATE,
                "IdentityProviderType": "AWS_LAMBDA",
                "IdentityProviderDetails": {"Function": LAMBDA_IDP},
                "EndpointType": "VPC",
                "EndpointDetails": vpc,
            }),
            None,
        ),
        (
            "SFTP alone on a public, service-managed server: the rule says nothing \
             about it",
            json!({"Protocols": ["SFTP"]}),
            None,
        ),
    ];

    for (case, request, refusal) in rows {
        let answer = emulator.call("CreateServer", request).await;
        match refusal {
            Some(named) => {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {} with body {}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "InvalidRequestException",
                    "case {case:?}: expected InvalidRequestException, got body {}",
                    answer.body
                );
                let message = member_of(&answer.body, "Message")
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                for fragment in named {
                    assert!(
                        message.contains(fragment),
                        "case {case:?}: expected the refusal to name {fragment:?}, \
                         got {message:?}"
                    );
                }
                assert!(
                    answer.body.get("ServerId").is_none(),
                    "case {case:?}: a refused create must answer no ServerId, got {}",
                    answer.body
                );
            }
            None => assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the create to be served, got {} with body {}",
                answer.status, answer.body
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// 3. The stored SSH public key body is the submitted one, rstripped.
// ---------------------------------------------------------------------------

/// RULE: the `SshPublicKeyBody` a write stores — and so the one every reader reports
/// — is the submitted body with TRAILING whitespace removed; every other byte,
/// including the comment after the key material and the whitespace inside it, is
/// kept.
///
/// The rows differ along the shape of the trailing whitespace (none, spaces, a tab,
/// a newline, CRLF, a mixture) and along whether the body carries a comment, and the
/// table is driven through BOTH writes that take a key body (`ImportSshPublicKey`
/// and `CreateUser`), because the rule is about the member.
#[tokio::test]
async fn a_stored_ssh_public_key_body_has_its_trailing_whitespace_removed() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    let with_comment = format!("{KEY_MATERIAL} user@host");

    // (case, the submitted body, the body the service must store — stated here as a
    // LITERAL, not re-derived with the `trim_end` the implementation uses, so a row
    // cannot agree with a wrong normalisation)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "no trailing whitespace",
            KEY_MATERIAL.to_string(),
            KEY_MATERIAL.to_string(),
        ),
        (
            "two trailing spaces",
            format!("{KEY_MATERIAL}  "),
            KEY_MATERIAL.to_string(),
        ),
        (
            "a trailing tab",
            format!("{KEY_MATERIAL}\t"),
            KEY_MATERIAL.to_string(),
        ),
        (
            "a trailing newline",
            format!("{KEY_MATERIAL}\n"),
            KEY_MATERIAL.to_string(),
        ),
        (
            "a trailing CRLF",
            format!("{KEY_MATERIAL}\r\n"),
            KEY_MATERIAL.to_string(),
        ),
        (
            "a mixture of trailing whitespace",
            format!("{KEY_MATERIAL} \t\n \n"),
            KEY_MATERIAL.to_string(),
        ),
        (
            "a comment and no trailing whitespace",
            with_comment.clone(),
            format!("{KEY_MATERIAL} user@host"),
        ),
        (
            "a comment and trailing whitespace",
            format!("{with_comment}  \n"),
            format!("{KEY_MATERIAL} user@host"),
        ),
    ];

    for (index, (case, submitted, expected)) in rows.into_iter().enumerate() {

        // The import: one user per row, so each row's key set is its own.
        let user_name = format!("u-key-{index}");
        emulator.create_user(&server_id, &user_name).await;
        let answer = emulator
            .call(
                "ImportSshPublicKey",
                json!({
                    "ServerId": &server_id,
                    "UserName": &user_name,
                    "SshPublicKeyBody": &submitted,
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the import to be served, got {} with body {}",
            answer.status, answer.body
        );
        let key_id = answer
            .get("SshPublicKeyId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a SshPublicKeyId"))
            .to_string();
        assert!(
            is_modelled_key_id(&key_id),
            "case {case:?}: expected a modelled SshPublicKeyId, got {key_id:?}"
        );
        let stored = stored_key_body(&emulator, &server_id, &user_name, &key_id).await;
        assert_eq!(
            stored, expected,
            "case {case:?}: expected the stored body to be the submitted one \
             rstripped, got {stored:?}"
        );

        // A second import of the same key differing only in trailing whitespace is
        // the same key: it is refused rather than stored twice.
        let repeat = emulator
            .call(
                "ImportSshPublicKey",
                json!({
                    "ServerId": &server_id,
                    "UserName": &user_name,
                    "SshPublicKeyBody": format!("{expected}   "),
                }),
            )
            .await;
        assert_eq!(
            repeat.error_code(),
            "ResourceExistsException",
            "case {case:?}: re-importing the same key must be refused, got {} with body {}",
            repeat.status,
            repeat.body
        );

        // The create: the same member on the other write that takes it.
        let created_user = format!("u-key-create-{index}");
        let answer = emulator
            .call(
                "CreateUser",
                json!({
                    "ServerId": &server_id,
                    "UserName": &created_user,
                    "Role": ROLE,
                    "SshPublicKeyBody": &submitted,
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the create to be served, got {} with body {}",
            answer.status, answer.body
        );
        let described = emulator
            .call(
                "DescribeUser",
                json!({"ServerId": &server_id, "UserName": &created_user}),
            )
            .await;
        let keys = member_of(described.get("User"), "SshPublicKeys")
            .as_array()
            .cloned()
            .unwrap_or_default();
        let body = keys
            .first()
            .map(|key| member_of(key, "SshPublicKeyBody").clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            body,
            json!(expected),
            "case {case:?}: the create must store the submitted body rstripped"
        );
    }
}

/// The body `DescribeUser` reports for the key `key_id` names.
async fn stored_key_body(
    emulator: &Emulator,
    server_id: &str,
    user_name: &str,
    key_id: &str,
) -> String {
    let described = emulator
        .call(
            "DescribeUser",
            json!({"ServerId": server_id, "UserName": user_name}),
        )
        .await;
    let keys = member_of(described.get("User"), "SshPublicKeys")
        .as_array()
        .cloned()
        .unwrap_or_default();
    keys.iter()
        .find(|key| member_of(key, "SshPublicKeyId") == &json!(key_id))
        .map(|key| {
            member_of(key, "SshPublicKeyBody")
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .unwrap_or_else(|| panic!("DescribeUser reported no key {key_id}: {}", described.body))
}
