// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the failures it answers.
//!
//! Two rules are asserted here, each over a TABLE whose rows differ along what the
//! rule generalises over:
//!
//! 1. Every error `Message` this service answers is a FIXED text for the condition it
//!    reports and carries no request data: the identifier a caller needs travels in
//!    the error's own `Resource` member, so one request's values never reach the
//!    message another client logs or asserts on, and the text is per CONDITION — two
//!    different bad references are not told apart by their `ResourceType` alone.
//!    The rows differ along the condition, the operation, the resource type, why the
//!    reference is bad (never created, or deleted), and the identifiers the request
//!    carried.
//! 2. The `ValidationException` family is the one message built from the request, and
//!    it is a template over the request's STRUCTURE, never its values: `N validation
//!    error(s) detected: ` over one clause per violation, each naming the member PATH
//!    and the MODELLED constraint it failed — with a `@pattern` quoted as the model's
//!    expression without the anchors a Smithy pattern writes around it, since Smithy
//!    pattern semantics are a find. The rows differ along the constraint class, the
//!    member, how deep it sits in the request, and how many members are wrong at once.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees.

use serde_json::{json, Value};

use crate::harness::{member_of, Answer, Emulator, PAST_START_SPAN, ROLE, SSH_KEY};

/// `SERVER_STOP_SPAN` in `src/state.rs` is 500 ms, like the start span
/// [`PAST_START_SPAN`] waits out; wait past it.
const PAST_STOP_SPAN: std::time::Duration = std::time::Duration::from_millis(900);

/// A name at `UserName`'s modelled upper length bound (`len 3..100`, pattern
/// `^[\w][\w@.-]{2,99}$`).
const LONG_USER_NAME: &str =
    "aaaaaaaaaabbbbbbbbbbccccccccccddddddddddeeeeeeeeeeffffffffffgggggggggghhhhhhhhhhiiiiiiiiiijjjjjjjjjj";

/// A second `Role`, so a duplicate create can differ from the first in everything but
/// the name. Satisfies `Role`'s modelled `len 20..2048`, pattern `^arn:.*role/\S+$`.
const OTHER_ROLE: &str = "arn:aws:iam::000000000000:role/transfer-other";

/// Key bodies satisfying `SshPublicKeyBody`'s modelled pattern, one per key type the
/// pattern admits, so a duplicate import is asserted over more than one key shape.
const SSH_KEY_ED25519: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAbCdEfGhIjKlMnOpQrStUvWxYz0123456789ab";
const SSH_KEY_ECDSA: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTY=";

/// Syntactically valid `SshPublicKeyId`s (`len 21..21`, `^key-[0-9a-f]{17}$`) that no
/// import ever minted: the low end of the id space, a mid value, and the high end.
const NEVER_MINTED_KEYS: [&str; 3] = [
    "key-00000000000000000",
    "key-0000000000000dead",
    "key-fffffffffffffffff",
];

/// A `ServerId` satisfying `^s-([0-9a-f]{17})$` that was never minted. The rows that
/// use it are refused before any lookup, so a well-formed id is all they need.
const UNMINTED_SERVER_ID: &str = "s-0000000000000dead";

/// A certificate body satisfying `CertificateBodyType`'s modelled `len 1..16384`,
/// pattern `^[\t\n\r\u0020-\u00FF]+$`, so a row testing `Description` is refused for
/// its description and nothing else.
const CERTIFICATE_BODY: &str =
    "-----BEGIN CERTIFICATE-----\nMIIBdummybody==\n-----END CERTIFICATE-----";

/// The `Message` of an answer, whichever casing the envelope carries it under: the
/// modelled Transfer errors spell it `Message`, the framework's synthetic
/// `ValidationException` spells it `message`.
fn message_of(body: &Value) -> String {
    for member in ["Message", "message"] {
        if let Some(text) = body.get(member).and_then(Value::as_str) {
            return text.to_string();
        }
    }
    String::new()
}

/// A member of an error body as a string, or `""` when absent.
fn member_text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert `answer` is the refusal `code` with the fixed `message`, and that none of
/// `absent` — every identifier and value the request carried — reached it. Returns the
/// message so a caller can compare it across rows.
fn assert_fixed_message(
    case: &str,
    answer: &Answer,
    status: u16,
    code: &str,
    message: &str,
    absent: &[&str],
) -> String {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected {code}, got body {}",
        answer.body
    );
    let actual = message_of(&answer.body);
    assert_eq!(
        actual, message,
        "case {case:?}: expected the fixed text {message:?}, got {actual:?}"
    );
    for fragment in absent {
        assert!(
            !actual.contains(fragment),
            "case {case:?}: expected no request data in Message, got {actual:?} carrying \
             {fragment:?}"
        );
    }
    actual
}

// ---------------------------------------------------------------------------
// Rule 1: a fixed text per condition, with the identifier in `Resource`
// ---------------------------------------------------------------------------

/// RULE: a `CreateUser` for a `UserName` the server already holds is refused with the
/// service's fixed duplicate-user text, and the name that collided travels in
/// `Resource` rather than in the message.
///
/// The rows differ along the shape of the name, what else the duplicate request
/// changed, and which server the name is taken on — a renamed probe is not a different
/// input, so each row changes one of those instead.
#[tokio::test]
async fn a_duplicate_user_name_is_refused_with_the_fixed_duplicate_user_text() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    let other_server = emulator.create_server(json!({})).await;

    // (case, server to create on, user name, the members the SECOND create changes)
    let cases: Vec<(&str, &String, &str, Value)> = vec![
        (
            "the same request twice",
            &server_id,
            "bob",
            json!({}),
        ),
        (
            "a name carrying the punctuation the pattern allows",
            &server_id,
            "re.peat-user@example",
            json!({"HomeDirectory": "/bucket/home"}),
        ),
        (
            "a name at the modelled length bound, duplicated with another role",
            &server_id,
            LONG_USER_NAME,
            json!({"Role": OTHER_ROLE}),
        ),
        (
            "a name taken on a second server",
            &other_server,
            "bob",
            json!({"HomeDirectoryType": "PATH", "HomeDirectory": "/bucket/bob"}),
        ),
    ];

    let mut messages: Vec<(String, String)> = Vec::new();
    for (case, server, user_name, changed) in cases {
        emulator.create_user(server, user_name).await;

        let mut request = json!({"ServerId": server, "UserName": user_name, "Role": ROLE});
        for (member, value) in changed.as_object().expect("the changes are an object") {
            request[member] = value.clone();
        }
        let answer = emulator.call("CreateUser", request).await;

        // 409 is `ResourceExistsException`'s modelled `@httpError`.
        let message = assert_fixed_message(
            case,
            &answer,
            409,
            "ResourceExistsException",
            "User already exists",
            &[user_name, server.as_str()],
        );
        assert_eq!(
            member_text(&answer.body, "Resource"),
            user_name,
            "case {case:?}: expected the colliding name in Resource, got {}",
            answer.body
        );
        assert!(
            !member_text(&answer.body, "ResourceType").is_empty(),
            "case {case:?}: ResourceType is modelled required, got {}",
            answer.body
        );
        messages.push((case.to_string(), message));
    }

    assert_same_message("the duplicate-user condition", &messages);
}

/// RULE: an `ImportSshPublicKey` for a body the user already holds is refused with the
/// service's fixed duplicate-key text, which carries neither the user, the server nor
/// the key material.
///
/// The rows differ along the key type the body names and the user that holds it.
#[tokio::test]
async fn a_duplicate_ssh_public_key_is_refused_with_the_fixed_duplicate_key_text() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    // (case, user name, key body)
    let cases: [(&str, &str, &str); 3] = [
        ("an RSA body", "alice", SSH_KEY),
        ("an ed25519 body", "bob", SSH_KEY_ED25519),
        ("an ECDSA body", "carol", SSH_KEY_ECDSA),
    ];

    let mut messages: Vec<(String, String)> = Vec::new();
    for (case, user_name, body) in cases {
        emulator.create_user(&server_id, user_name).await;
        let request = json!({
            "ServerId": &server_id,
            "UserName": user_name,
            "SshPublicKeyBody": body,
        });
        let imported = emulator.call("ImportSshPublicKey", request.clone()).await;
        assert_eq!(
            imported.status, 200,
            "case {case:?}: expected the first import to succeed, got {} with body {}",
            imported.status, imported.body
        );

        let answer = emulator.call("ImportSshPublicKey", request).await;
        let message = assert_fixed_message(
            case,
            &answer,
            409,
            "ResourceExistsException",
            "Public key already exists",
            &[user_name, server_id.as_str(), body],
        );
        messages.push((case.to_string(), message));
    }

    assert_same_message("the duplicate-key condition", &messages);
}

/// RULE: the not-found text is per RESOURCE KIND, so an unresolvable public key id, an
/// unresolvable user and an unresolvable server are told apart by the `Message` — each
/// fixed across every row of its kind, and none carrying an identifier.
///
/// The rows differ along the kind, why the reference is bad (never minted, or deleted),
/// the shape of the identifier, and the operation that resolves it.
#[tokio::test]
async fn the_not_found_text_differs_per_resource_kind_and_is_fixed_within_one() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    emulator.create_user(&server_id, "alice").await;

    let mut key_messages: Vec<(String, String)> = Vec::new();
    let mut user_messages: Vec<(String, String)> = Vec::new();
    let mut server_messages: Vec<(String, String)> = Vec::new();

    // A key the user never held, over the id space.
    for absent in NEVER_MINTED_KEYS {
        let case = format!("DeleteSshPublicKey for the unminted key {absent}");
        let answer = emulator
            .call(
                "DeleteSshPublicKey",
                json!({"ServerId": &server_id, "UserName": "alice", "SshPublicKeyId": absent}),
            )
            .await;
        let message = assert_fixed_message(
            &case,
            &answer,
            400,
            "ResourceNotFoundException",
            "Unknown public key ID",
            &[absent, "alice", server_id.as_str()],
        );
        assert_eq!(
            member_text(&answer.body, "Resource"),
            absent,
            "case {case:?}: expected the requested key id in Resource, got {}",
            answer.body
        );
        key_messages.push((case, message));
    }

    // A key that WAS held and was deleted: the same reference is bad for a different
    // reason, so it answers the same text.
    let imported = emulator
        .call(
            "ImportSshPublicKey",
            json!({"ServerId": &server_id, "UserName": "alice", "SshPublicKeyBody": SSH_KEY}),
        )
        .await;
    let key_id = imported
        .get("SshPublicKeyId")
        .as_str()
        .unwrap_or_else(|| panic!("ImportSshPublicKey answered no id: {}", imported.body))
        .to_string();
    for round in ["first", "second"] {
        let deleted = emulator
            .call(
                "DeleteSshPublicKey",
                json!({"ServerId": &server_id, "UserName": "alice", "SshPublicKeyId": &key_id}),
            )
            .await;
        if round == "first" {
            assert_eq!(
                deleted.status, 200,
                "the first DeleteSshPublicKey must succeed, got {} with body {}",
                deleted.status, deleted.body
            );
            continue;
        }
        let case = "DeleteSshPublicKey for a key already deleted".to_string();
        let message = assert_fixed_message(
            &case,
            &deleted,
            400,
            "ResourceNotFoundException",
            "Unknown public key ID",
            &[key_id.as_str(), "alice", server_id.as_str()],
        );
        key_messages.push((case, message));
    }

    // The same operation, one reference further out: the USER cannot be resolved.
    for absent_user in ["nobody", "no.such-user", LONG_USER_NAME] {
        let case = format!("DeleteSshPublicKey for the absent user {absent_user}");
        let answer = emulator
            .call(
                "DeleteSshPublicKey",
                json!({
                    "ServerId": &server_id,
                    "UserName": absent_user,
                    "SshPublicKeyId": NEVER_MINTED_KEYS[0],
                }),
            )
            .await;
        let message = message_of(&answer.body);
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
        user_messages.push((case, message));
    }

    // And one further out again: the SERVER cannot be resolved.
    for (case, op, request) in [
        (
            "DeleteSshPublicKey on an unminted server",
            "DeleteSshPublicKey",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": "alice",
                "SshPublicKeyId": NEVER_MINTED_KEYS[0],
            }),
        ),
        (
            "DescribeServer on an unminted server",
            "DescribeServer",
            json!({"ServerId": UNMINTED_SERVER_ID}),
        ),
    ] {
        let answer = emulator.call(op, request).await;
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
        server_messages.push((case.to_string(), message_of(&answer.body)));
    }

    assert_same_message("the unknown-key condition", &key_messages);
    assert_same_message("the unknown-user condition", &user_messages);
    assert_same_message("the unknown-server condition", &server_messages);

    // The texts tell the three kinds APART: the message is what says which reference
    // was bad.
    let kinds = [
        ("public key", &key_messages),
        ("user", &user_messages),
        ("server", &server_messages),
    ];
    for (left_index, (left_name, left)) in kinds.iter().enumerate() {
        for (right_name, right) in kinds.iter().skip(left_index + 1) {
            assert_ne!(
                left[0].1, right[0].1,
                "expected the {left_name} and {right_name} not-found texts to differ, both are \
                 {:?}",
                left[0].1
            );
        }
    }
}

/// RULE: a user whose `HomeDirectoryType` is `LOGICAL` with no mappings violates
/// `HomeDirectoryMappings`' own modelled minimum (`@length` min 1), so the refusal
/// states that member's requirement and names nothing of the request.
///
/// The rows differ along the operation (create, and an update of each kind of stored
/// user) and whether the stored user had mappings to lose.
#[tokio::test]
async fn a_logical_home_directory_without_mappings_states_the_lists_own_minimum() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    let mapping = json!([{"Entry": "/", "Target": "/bucket/home"}]);

    emulator.create_user(&server_id, "path-user").await;
    let created = emulator
        .call(
            "CreateUser",
            json!({
                "ServerId": &server_id,
                "UserName": "logical-user",
                "Role": ROLE,
                "HomeDirectoryType": "LOGICAL",
                "HomeDirectoryMappings": mapping,
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "creating the mapped user must succeed, got {} with body {}",
        created.status, created.body
    );

    // (case, operation, request)
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "CreateUser with LOGICAL and no mappings",
            "CreateUser",
            json!({
                "ServerId": &server_id,
                "UserName": "new-user",
                "Role": ROLE,
                "HomeDirectoryType": "LOGICAL",
            }),
        ),
        (
            "CreateUser with LOGICAL and no mappings on an unminted server",
            "CreateUser",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": "new-user",
                "Role": ROLE,
                "HomeDirectoryType": "LOGICAL",
            }),
        ),
        (
            "UpdateUser turning a PATH user LOGICAL without mappings",
            "UpdateUser",
            json!({
                "ServerId": &server_id,
                "UserName": "path-user",
                "HomeDirectoryType": "LOGICAL",
            }),
        ),
    ];

    let mut messages: Vec<(String, String)> = Vec::new();
    for (case, op, request) in cases {
        let answer = emulator.call(op, request).await;
        let message = assert_fixed_message(
            case,
            &answer,
            400,
            "InvalidRequestException",
            "HomeDirectoryMappings must not be empty",
            &[server_id.as_str(), UNMINTED_SERVER_ID, "path-user"],
        );
        messages.push((case.to_string(), message));
    }

    assert_same_message("the empty-mappings condition", &messages);
}

/// RULE: `IpAddressType` "cannot be changed while the server is online. You must stop
/// the server before modifying this parameter" — so the refusal states the one state
/// the change IS allowed from, the same text whichever other state the server is in
/// and whichever value was asked for.
///
/// The rows differ along the server's state when the change is asked for (`STARTING`,
/// `ONLINE`, `STOPPING`) and the value asked for.
#[tokio::test]
async fn an_ip_address_type_change_off_an_offline_server_states_the_required_state() {
    let emulator = Emulator::new();
    let mut messages: Vec<(String, String)> = Vec::new();

    // Row 1: STARTING — a server answers the transient state straight after the create.
    let starting = emulator.create_server(json!({})).await;
    let answer = emulator
        .call(
            "UpdateServer",
            json!({"ServerId": &starting, "IpAddressType": "DUALSTACK"}),
        )
        .await;
    messages.push((
        "while STARTING".to_string(),
        assert_fixed_message(
            "while STARTING",
            &answer,
            400,
            "InvalidRequestException",
            "Server must be OFFLINE to change IpAddressType",
            &[starting.as_str(), "STARTING"],
        ),
    ));

    // Row 2: ONLINE — the state the server settles into.
    tokio::time::sleep(PAST_START_SPAN).await;
    let answer = emulator
        .call(
            "UpdateServer",
            json!({"ServerId": &starting, "IpAddressType": "DUALSTACK"}),
        )
        .await;
    messages.push((
        "while ONLINE".to_string(),
        assert_fixed_message(
            "while ONLINE",
            &answer,
            400,
            "InvalidRequestException",
            "Server must be OFFLINE to change IpAddressType",
            &[starting.as_str(), "ONLINE"],
        ),
    ));

    // Row 3: STOPPING — on the way to OFFLINE, but not there yet.
    let stopped = emulator
        .call("StopServer", json!({"ServerId": &starting}))
        .await;
    assert_eq!(
        stopped.status, 200,
        "StopServer must succeed, got {} with body {}",
        stopped.status, stopped.body
    );
    let answer = emulator
        .call(
            "UpdateServer",
            json!({"ServerId": &starting, "IpAddressType": "DUALSTACK"}),
        )
        .await;
    messages.push((
        "while STOPPING".to_string(),
        assert_fixed_message(
            "while STOPPING",
            &answer,
            400,
            "InvalidRequestException",
            "Server must be OFFLINE to change IpAddressType",
            &[starting.as_str(), "STOPPING"],
        ),
    ));

    // Row 4: the other direction of the change, refused the same way. Once OFFLINE the
    // change goes through, so the server is started again to ask for the reverse.
    tokio::time::sleep(PAST_STOP_SPAN).await;
    let accepted = emulator
        .call(
            "UpdateServer",
            json!({"ServerId": &starting, "IpAddressType": "DUALSTACK"}),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "an OFFLINE server must accept the change, got {} with body {}",
        accepted.status, accepted.body
    );
    let started = emulator
        .call("StartServer", json!({"ServerId": &starting}))
        .await;
    assert_eq!(
        started.status, 200,
        "StartServer must succeed, got {} with body {}",
        started.status, started.body
    );
    tokio::time::sleep(PAST_START_SPAN).await;
    let answer = emulator
        .call(
            "UpdateServer",
            json!({"ServerId": &starting, "IpAddressType": "IPV4"}),
        )
        .await;
    messages.push((
        "back to IPV4 while ONLINE".to_string(),
        assert_fixed_message(
            "back to IPV4 while ONLINE",
            &answer,
            400,
            "InvalidRequestException",
            "Server must be OFFLINE to change IpAddressType",
            &[starting.as_str(), "IPV4"],
        ),
    ));

    assert_same_message("the IpAddressType state condition", &messages);
}

/// Every row of one condition must answer the SAME `Message`: it is fixed for the
/// condition, never derived from the request.
fn assert_same_message(condition: &str, messages: &[(String, String)]) {
    let Some((first_case, first)) = messages.first() else {
        panic!("condition {condition}: no case produced a message");
    };
    for (case, message) in messages {
        assert_eq!(
            message, first,
            "case {case:?}: expected the same fixed text for {condition} as case \
             {first_case:?} ({first:?}), got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the ValidationException template
// ---------------------------------------------------------------------------

/// The clause ONE violated constraint contributes, written from the rule rather than
/// from the implementation: the member PATH and the modelled requirement, with nothing
/// of the value that was submitted.
fn clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// The clause an ABSENT member contributes. It submitted no value at all, and `null`
/// is the service's own word for that absence rather than anything the caller sent.
fn absent_clause(path: &str) -> String {
    format!("Value null at '{path}' failed to satisfy constraint: Member must not be null")
}

/// Assert `answer` is the 400 `ValidationException` whose message is the envelope over
/// exactly `clauses`, carries none of `rejected` (the values and anchored patterns the
/// request's own data would have contributed), and counts its clauses the way AWS does.
fn assert_validation_message(case: &str, answer: &Answer, clauses: &[String], rejected: &[&str]) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got body {}",
        answer.body
    );
    let message = message_of(&answer.body);
    let n = clauses.len();
    let prefix = if n == 1 {
        "1 validation error detected: ".to_string()
    } else {
        format!("{n} validation errors detected: ")
    };
    assert!(
        message.starts_with(&prefix),
        "case {case:?}: expected the message to open with {prefix:?}, got {message:?}"
    );
    for expected in clauses {
        assert!(
            message.contains(expected.as_str()),
            "case {case:?}: expected the clause {expected:?}, got {message:?}"
        );
    }
    // Exactly as many clauses as violations: no clause was added beyond the ones the
    // rule accounts for.
    assert_eq!(
        message.matches("failed to satisfy constraint:").count(),
        n,
        "case {case:?}: expected {n} clause(s), got {message:?}"
    );
    for fragment in rejected {
        assert!(
            !message.contains(fragment),
            "case {case:?}: expected {fragment:?} to be absent from the message, got {message:?}"
        );
    }
}

/// One row: the case, the operation, the request, the clauses the message must carry,
/// and the fragments it must not.
type ValidationCase<'a> = (&'a str, &'a str, Value, Vec<String>, Vec<&'a str>);

/// RULE: a constraint violation is answered as HTTP 400 `ValidationException` whose
/// message names the member path and the MODELLED constraint it failed, never the
/// submitted value, and quotes a `@pattern` without the anchors the model writes
/// around it.
///
/// The rows differ along the constraint class (absent member, length minimum and
/// maximum, range minimum and maximum, pattern, enum value set), the member, how deep
/// it sits in the request (top level, inside a structure, inside a list), and how many
/// members are wrong at once. Every bound and value set below is the one this service's
/// model declares — `smithy_query(query="shape", shape="com.amazonaws.transfer#PosixProfile")`
/// for `@range` 0..4294967295 on `Uid`/`Gid`,
/// `shape="com.amazonaws.transfer#UserName"` for `len 3..100` and
/// `^[\w][\w@.-]{2,99}$`, `shape="com.amazonaws.transfer#Role"` for `len 20..2048`,
/// `query="operation", operation="ImportCertificate"` for `Description`'s
/// `^[\u0021-\u007E]+$`, and `shape="com.amazonaws.transfer#Domain"` for `[S3, EFS]`.
#[tokio::test]
async fn a_validation_message_names_the_member_and_the_modelled_constraint_only() {
    let emulator = Emulator::new();

    let cases: Vec<ValidationCase<'_>> = vec![
        (
            "range above the modelled maximum",
            "CreateUser",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": "posix-user",
                "Role": ROLE,
                "PosixProfile": {"Uid": 4294967296_i64, "Gid": 1000},
            }),
            vec![clause(
                "posixProfile.uid",
                "Member must have value less than or equal to 4294967295",
            )],
            vec!["4294967296"],
        ),
        (
            "range below the modelled minimum, on the sibling member",
            "CreateUser",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": "posix-user",
                "Role": ROLE,
                "PosixProfile": {"Uid": 1000, "Gid": -1},
            }),
            vec![clause(
                "posixProfile.gid",
                "Member must have value greater than or equal to 0",
            )],
            vec!["'-1'"],
        ),
        (
            "pattern on a top-level member",
            "CreateUser",
            json!({"ServerId": UNMINTED_SERVER_ID, "UserName": "a b", "Role": ROLE}),
            vec![clause(
                "userName",
                r"Member must satisfy regular expression pattern: [\w][\w@.-]{2,99}",
            )],
            vec!["a b", r"^[\w][\w@.-]{2,99}$"],
        ),
        (
            "pattern refusing a value outside the printable-ASCII range",
            "ImportCertificate",
            json!({
                "Usage": "SIGNING",
                "Certificate": CERTIFICATE_BODY,
                "Description": "证书说明 – café ✅",
            }),
            vec![clause(
                "description",
                r"Member must satisfy regular expression pattern: [\u0021-\u007E]+",
            )],
            vec!["证书说明", "café", r"^[\u0021-\u007E]+$"],
        ),
        (
            "pattern refusing the character one below the range's lower bound",
            "ImportCertificate",
            json!({
                "Usage": "ENCRYPTION",
                "Certificate": CERTIFICATE_BODY,
                "Description": " ",
            }),
            vec![clause(
                "description",
                r"Member must satisfy regular expression pattern: [\u0021-\u007E]+",
            )],
            vec![r"^[\u0021-\u007E]+$"],
        ),
        (
            "length below the modelled minimum",
            "CreateUser",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": "short-role-user",
                "Role": "arn:a:iam::1:role/r",
            }),
            vec![clause(
                "role",
                "Member must have length greater than or equal to 20",
            )],
            vec!["arn:a:iam::1:role/r"],
        ),
        (
            "length above the modelled maximum, with the pattern it also breaks",
            "CreateUser",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": format!("{LONG_USER_NAME}k"),
                "Role": ROLE,
            }),
            vec![
                clause("userName", "Member must have length less than or equal to 100"),
                clause(
                    "userName",
                    r"Member must satisfy regular expression pattern: [\w][\w@.-]{2,99}",
                ),
            ],
            vec![LONG_USER_NAME, r"^[\w][\w@.-]{2,99}$"],
        ),
        (
            "an absent required member",
            "CreateProfile",
            json!({"As2Id": "partner-as2-id"}),
            vec![absent_clause("profileType")],
            vec!["partner-as2-id"],
        ),
        (
            "an enum value set at the top level",
            "CreateServer",
            json!({"Domain": "GLACIER"}),
            vec![clause(
                "domain",
                "Member must satisfy enum value set: [S3, EFS]",
            )],
            vec!["GLACIER"],
        ),
        (
            "a member inside a structure",
            "CreateServer",
            json!({"S3StorageOptions": {"DirectoryListingOptimization": "MAYBE"}}),
            vec![clause(
                "s3StorageOptions.directoryListingOptimization",
                "Member must satisfy enum value set: [ENABLED, DISABLED]",
            )],
            vec!["MAYBE"],
        ),
        (
            "a member inside a list",
            "CreateUser",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": "mapped-user",
                "Role": ROLE,
                "HomeDirectoryType": "LOGICAL",
                "HomeDirectoryMappings": [
                    {"Entry": "/", "Target": "/bucket/home"},
                    {"Entry": "/shared", "Target": "/bucket/shared", "Type": "SYMLINK"},
                ],
            }),
            vec![clause(
                "homeDirectoryMappings.2.member.type",
                "Member must satisfy enum value set: [FILE, DIRECTORY]",
            )],
            vec!["SYMLINK"],
        ),
        (
            "two members wrong at once",
            "CreateUser",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": "two-wrong",
                "Role": ROLE,
                "HomeDirectoryType": "ABSOLUTE",
                "PosixProfile": {"Uid": 4294967296_i64, "Gid": 1000},
            }),
            vec![
                clause(
                    "homeDirectoryType",
                    "Member must satisfy enum value set: [PATH, LOGICAL]",
                ),
                clause(
                    "posixProfile.uid",
                    "Member must have value less than or equal to 4294967295",
                ),
            ],
            vec!["ABSOLUTE", "4294967296"],
        ),
    ];

    for (case, op, request, clauses, rejected) in cases {
        let answer = emulator.call(op, request).await;
        assert_validation_message(case, &answer, &clauses, &rejected);
    }
}

/// RULE (the other side of the same rule): a value INSIDE a modelled bound is not
/// refused for it — so the clauses above are the model's constraints, not a blanket
/// refusal.
///
/// The rows walk each published bound of `PosixProfile`'s `@range` (0 and 4294967295,
/// per `smithy_query(query="shape", shape="com.amazonaws.transfer#PosixProfile")`) and
/// the printable-ASCII bounds of `Description`'s pattern (`\u0021` `!` and `\u007E`
/// `~`); the rows one past each bound are in the table above.
#[tokio::test]
async fn a_value_inside_the_modelled_bound_is_not_refused() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    // (case, user name, the POSIX ids at a bound)
    let posix_cases: [(&str, &str, i64, i64); 3] = [
        ("both ids at the minimum", "posix-min", 0, 0),
        ("both ids at the maximum", "posix-max", 4294967295, 4294967295),
        ("one id at each bound", "posix-mixed", 0, 4294967295),
    ];
    for (case, user_name, uid, gid) in posix_cases {
        let answer = emulator
            .call(
                "CreateUser",
                json!({
                    "ServerId": &server_id,
                    "UserName": user_name,
                    "Role": ROLE,
                    "PosixProfile": {"Uid": uid, "Gid": gid},
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        // Read it back: the accepted bound really is what the user now holds.
        let described = emulator
            .call(
                "DescribeUser",
                json!({"ServerId": &server_id, "UserName": user_name}),
            )
            .await;
        let profile = member_of(described.get("User"), "PosixProfile").clone();
        assert_eq!(
            (
                profile.get("Uid").and_then(Value::as_i64),
                profile.get("Gid").and_then(Value::as_i64)
            ),
            (Some(uid), Some(gid)),
            "case {case:?}: expected the stored POSIX profile to be ({uid}, {gid}), got {}",
            described.body
        );
    }

    // A description INSIDE the pattern's character range is not refused for the
    // pattern: whatever this operation answers, it is not a ValidationException.
    for (case, description) in [
        ("the character at the range's lower bound", "!"),
        ("the character at the range's upper bound", "~"),
        ("printable ASCII in between", "cert-for-partner_01"),
    ] {
        let answer = emulator
            .call(
                "ImportCertificate",
                json!({
                    "Usage": "TLS",
                    "Certificate": CERTIFICATE_BODY,
                    "Description": description,
                }),
            )
            .await;
        assert_ne!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: a description inside the modelled pattern must not be refused for \
             it, got {} with body {}",
            answer.status,
            answer.body
        );
    }
}
