// SPDX-License-Identifier: CC0-1.0

//! A user's PROPERTIES and its SSH public keys, over the whole wire path.
//!
//! Three rules about the stored user record, each asserted over a TABLE whose rows
//! differ along what the rule generalises over:
//!
//! 1. `HomeDirectoryType` is resolved ONCE by the write that set the landing
//!    directory — the caller's value when it sent one, else `PATH`, the type the
//!    documented pairing leaves a user configured through `HomeDirectory` — and BOTH
//!    readers (`DescribeUser`, `ListUsers`) report that one stored value. (The write
//!    that breaks the pairing is refused; `tests/all/input_validation.rs` owns that
//!    half of the rule.)
//! 2. `UpdateUser` replaces exactly the properties the request names and leaves every
//!    other property of the stored user as it was, so the readers report the updated
//!    record; a server or user it does not hold is refused, not created.
//! 3. A user's SSH public keys are a stored set: `ImportSshPublicKey` adds one under a
//!    minted `SshPublicKeyId`, `DeleteSshPublicKey` removes exactly that one, and
//!    `DescribeUser`/`ListUsers` report the set and its live count.
//!
//! Every test drives the real wire path through the shared [`crate::harness`] — `POST /`
//! with `X-Amz-Target: TransferService.<Op>` — so what is asserted is what an SDK sees.

use serde_json::{json, Value};

use crate::harness::{is_modelled_key_id, member_of, Emulator, ROLE, SSH_KEY};

/// A second key body, distinct from [`SSH_KEY`], satisfying `SshPublicKeyBody`'s
/// modelled pattern `^\s*(ssh|ecdsa)-[a-z0-9-]+[ \t]+(([A-Za-z0-9+/]{4})*...)$`.
const SSH_KEY_2: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAbCdEfGhIjKlMnOpQrStUvWxYz0123456789ab";

/// A third key body, in the ECDSA form the same pattern admits.
const SSH_KEY_3: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTY=";

/// The `HomeDirectoryType` the readers report for a stored user, or `None`.
async fn described_type(emulator: &Emulator, server_id: &str, user_name: &str) -> Option<String> {
    let answer = emulator
        .call(
            "DescribeUser",
            json!({"ServerId": server_id, "UserName": user_name}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "DescribeUser {user_name}: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    member_of(answer.get("User"), "HomeDirectoryType")
        .as_str()
        .map(str::to_string)
}

/// The listed entry for `user_name`, from the server's `ListUsers` page.
async fn listed(emulator: &Emulator, server_id: &str, user_name: &str) -> Value {
    let answer = emulator
        .call("ListUsers", json!({"ServerId": server_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "ListUsers: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let users = answer.get("Users").as_array().cloned().unwrap_or_default();
    users
        .iter()
        .find(|entry| member_of(entry, "UserName").as_str() == Some(user_name))
        .cloned()
        .unwrap_or_else(|| panic!("ListUsers did not list {user_name:?}: {}", answer.body))
}

/// The user's stored keys as `(SshPublicKeyId, SshPublicKeyBody)` pairs, in the order
/// `DescribeUser` reports them.
async fn described_keys(
    emulator: &Emulator,
    server_id: &str,
    user_name: &str,
) -> Vec<(String, String)> {
    let answer = emulator
        .call(
            "DescribeUser",
            json!({"ServerId": server_id, "UserName": user_name}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "DescribeUser {user_name}: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let keys = member_of(answer.get("User"), "SshPublicKeys")
        .as_array()
        .cloned()
        .unwrap_or_default();
    keys.iter()
        .map(|key| {
            (
                member_of(key, "SshPublicKeyId")
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                member_of(key, "SshPublicKeyBody")
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: the type a write resolves is the type every reader reports.
// ---------------------------------------------------------------------------

/// RULE: a stored user's `HomeDirectoryType` is the value the create named; when the
/// create named none it is `PATH`, the type the documented pairing leaves a user
/// configured through `HomeDirectory`, and `DescribeUser` and `ListUsers` report that
/// same stored value.
///
/// The rows differ along which directory members the create supplied: neither, a path,
/// an EMPTY path (the shortest `HomeDirectory`'s `len 0..1024` admits), an explicit
/// `PATH` with a path, and an explicit `LOGICAL` with mappings. The combinations the
/// documented pairing refuses (mappings without `LOGICAL`, `LOGICAL` without mappings)
/// are refused before any of this applies — `tests/all/input_validation.rs` asserts
/// that — so they are not rows here.
#[tokio::test]
async fn the_home_directory_type_a_write_resolves_is_what_every_reader_reports() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    let mappings = json!([{"Entry": "/", "Target": "/bucket/home"}]);
    let cases: Vec<(&str, Value, &str)> = vec![
        ("no directory member at all", json!({}), "PATH"),
        (
            "a path landing directory",
            json!({"HomeDirectory": "/bucket/home"}),
            "PATH",
        ),
        (
            "the empty path HomeDirectory admits",
            json!({"HomeDirectory": ""}),
            "PATH",
        ),
        (
            "an explicit PATH with a path",
            json!({"HomeDirectory": "/bucket/explicit", "HomeDirectoryType": "PATH"}),
            "PATH",
        ),
        (
            "an explicit LOGICAL with mappings",
            json!({"HomeDirectoryType": "LOGICAL", "HomeDirectoryMappings": mappings}),
            "LOGICAL",
        ),
    ];

    for (index, (case, members, expected)) in cases.into_iter().enumerate() {
        let user_name = format!("hdt_{index}");
        let mut request = members;
        request["ServerId"] = json!(&server_id);
        request["UserName"] = json!(&user_name);
        request["Role"] = json!(ROLE);
        let created = emulator.call("CreateUser", request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateUser 200, got {} with body {}",
            created.status, created.body
        );

        let described = described_type(&emulator, &server_id, &user_name).await;
        assert_eq!(
            described.as_deref(),
            Some(expected),
            "case {case:?}: expected DescribeUser to report HomeDirectoryType {expected:?}, \
             got {described:?}"
        );
        let entry = listed(&emulator, &server_id, &user_name).await;
        assert_eq!(
            member_of(&entry, "HomeDirectoryType").as_str(),
            Some(expected),
            "case {case:?}: expected ListUsers to report the same stored \
             HomeDirectoryType {expected:?}, got {entry}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: UpdateUser replaces what it names and keeps the rest.
// ---------------------------------------------------------------------------

/// RULE: `UpdateUser` assigns the properties the request names to the STORED user and
/// leaves every property it does not name as it was, so the user-scoped reads report
/// the updated record.
///
/// The rows differ along which property the update names — the landing directory, the
/// role, the session policy, the POSIX profile, the mappings (which move the resolved
/// type to LOGICAL), an explicit type, and nothing at all — each applied to a user
/// created with the full set of properties, so an unnamed property changing is
/// detected.
#[tokio::test]
async fn update_user_replaces_the_named_properties_and_keeps_the_rest() {
    let base = json!({
        "Role": ROLE,
        "HomeDirectory": "/bucket/base",
        "HomeDirectoryType": "PATH",
        "Policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
        "PosixProfile": {"Uid": 1001, "Gid": 1002},
    });
    let other_role = "arn:aws:iam::000000000000:role/transfer-other";

    // (case, the update's members, the members the stored user must report after it).
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "nothing but the required members",
            json!({}),
            json!({
                "HomeDirectory": "/bucket/base",
                "HomeDirectoryType": "PATH",
                "Role": ROLE,
            }),
        ),
        (
            "the landing directory only",
            json!({"HomeDirectory": "/bucket/moved"}),
            json!({
                "HomeDirectory": "/bucket/moved",
                "HomeDirectoryType": "PATH",
                "Role": ROLE,
                "Policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
            }),
        ),
        (
            "the role only",
            json!({"Role": other_role}),
            json!({
                "Role": other_role,
                "HomeDirectory": "/bucket/base",
            }),
        ),
        (
            "the session policy only",
            json!({"Policy": "{\"Version\":\"2012-10-17\",\"Statement\":[{}]}"}),
            json!({
                "Policy": "{\"Version\":\"2012-10-17\",\"Statement\":[{}]}",
                "HomeDirectory": "/bucket/base",
                "Role": ROLE,
            }),
        ),
        (
            "the POSIX profile only",
            json!({"PosixProfile": {"Uid": 2001, "Gid": 2002, "SecondaryGids": [3001]}}),
            json!({
                "PosixProfile": {"Uid": 2001, "Gid": 2002, "SecondaryGids": [3001]},
                "HomeDirectory": "/bucket/base",
            }),
        ),
        (
            // Mappings move the user to a logical home directory; the documented
            // pairing requires the type to come with them.
            "logical mappings and the matching type",
            json!({
                "HomeDirectoryType": "LOGICAL",
                "HomeDirectoryMappings": [{"Entry": "/", "Target": "/bucket/logical"}],
            }),
            json!({
                "HomeDirectoryMappings": [{"Entry": "/", "Target": "/bucket/logical"}],
                "HomeDirectoryType": "LOGICAL",
                "HomeDirectory": "/bucket/base",
            }),
        ),
        (
            "an explicit type and a directory",
            json!({"HomeDirectoryType": "PATH", "HomeDirectory": "/bucket/typed"}),
            json!({
                "HomeDirectoryType": "PATH",
                "HomeDirectory": "/bucket/typed",
            }),
        ),
    ];

    for (index, (case, update, expected)) in cases.into_iter().enumerate() {
        // A fresh server and user per row, so one row's update cannot carry into
        // the next.
        let emulator = Emulator::new();
        let server_id = emulator.create_server(json!({})).await;
        let user_name = format!("upd_{index}");
        let mut create = base.clone();
        create["ServerId"] = json!(&server_id);
        create["UserName"] = json!(&user_name);
        let created = emulator.call("CreateUser", create).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateUser 200, got {} with body {}",
            created.status, created.body
        );

        let mut request = update;
        request["ServerId"] = json!(&server_id);
        request["UserName"] = json!(&user_name);
        let updated = emulator.call("UpdateUser", request).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateUser 200, got {} with body {}",
            updated.status, updated.body
        );
        assert_eq!(
            (
                updated.get("ServerId").as_str(),
                updated.get("UserName").as_str()
            ),
            (Some(server_id.as_str()), Some(user_name.as_str())),
            "case {case:?}: expected UpdateUser to answer the user's own key, got {}",
            updated.body
        );

        let answer = emulator
            .call(
                "DescribeUser",
                json!({"ServerId": &server_id, "UserName": &user_name}),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DescribeUser 200, got {} with body {}",
            answer.status, answer.body
        );
        let user = answer.get("User").clone();
        for (member, want) in expected.as_object().expect("the expectation is an object") {
            assert_eq!(
                member_of(&user, member),
                want,
                "case {case:?}: expected {member} to be {want} after the update, got {user}"
            );
        }
        // The identity of the user is not an update parameter: its name and ARN are
        // the ones the create minted.
        assert_eq!(
            member_of(&user, "UserName").as_str(),
            Some(user_name.as_str()),
            "case {case:?}: expected the update to leave the user's name, got {user}"
        );
        assert_eq!(
            member_of(&user, "Arn").as_str(),
            member_of(&listed(&emulator, &server_id, &user_name).await, "Arn").as_str(),
            "case {case:?}: expected both readers to report one stored ARN, got {user}"
        );
    }
}

/// RULE (the same rule, from the other side): an update that names NEITHER the type
/// nor the mappings leaves the stored `HomeDirectoryType` exactly as it was, because
/// the type is a property the request did not name.
///
/// The rows are the properties such an update can name instead — the landing
/// directory, the role, the session policy, nothing at all — each applied to a user
/// created `LOGICAL` with mappings, the value a type that was silently re-resolved
/// from a request naming no type would not keep.
#[tokio::test]
async fn an_update_naming_no_directory_member_leaves_the_stored_type() {
    let mappings = json!([{"Entry": "/", "Target": "/bucket/home"}]);
    let updates: Vec<(&str, Value)> = vec![
        ("the landing directory only", json!({"HomeDirectory": "/bucket/moved"})),
        (
            "the role only",
            json!({"Role": "arn:aws:iam::000000000000:role/transfer-other"}),
        ),
        (
            "the session policy only",
            json!({"Policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}"}),
        ),
        ("nothing but the required members", json!({})),
    ];

    for (case, update) in updates {
        let emulator = Emulator::new();
        let server_id = emulator.create_server(json!({})).await;
        let user_name = "typed";
        let created = emulator
            .call(
                "CreateUser",
                json!({
                    "ServerId": &server_id,
                    "UserName": user_name,
                    "Role": ROLE,
                    "HomeDirectoryType": "LOGICAL",
                    "HomeDirectoryMappings": mappings.clone(),
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateUser 200, got {} with body {}",
            created.status, created.body
        );

        let mut request = update;
        request["ServerId"] = json!(&server_id);
        request["UserName"] = json!(user_name);
        let updated = emulator.call("UpdateUser", request).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateUser 200, got {} with body {}",
            updated.status, updated.body
        );

        let described = described_type(&emulator, &server_id, user_name).await;
        assert_eq!(
            described.as_deref(),
            Some("LOGICAL"),
            "case {case:?}: expected the stored HomeDirectoryType to stay LOGICAL, \
             got {described:?}"
        );
    }
}

/// RULE: the configuration an update would LEAVE has to satisfy the same documented
/// pairing a create's does — `HomeDirectoryMappings` "can only be set when
/// HomeDirectoryType is set to LOGICAL", and a `LOGICAL` type requires mappings — so
/// an update that would leave the user breaking it is refused with
/// `InvalidRequestException` and the stored user is unchanged.
///
/// The rows differ along which side of the pairing the update would break, and along
/// whether the offending value comes from the REQUEST or from the record the request
/// leaves in place: mappings added to a PATH user, the type moved to LOGICAL with no
/// mappings, and the mappings emptied out from under a LOGICAL user.
#[tokio::test]
async fn an_update_that_would_break_the_home_directory_pairing_is_refused() {
    let mappings = json!([{"Entry": "/", "Target": "/bucket/home"}]);

    // (case, the create's directory members, the update's members).
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "mappings added to a PATH user",
            json!({"HomeDirectoryType": "PATH", "HomeDirectory": "/bucket/base"}),
            json!({"HomeDirectoryMappings": mappings.clone()}),
        ),
        (
            "the type moved to LOGICAL with no mappings stored",
            json!({"HomeDirectoryType": "PATH", "HomeDirectory": "/bucket/base"}),
            json!({"HomeDirectoryType": "LOGICAL"}),
        ),
        (
            "the mappings emptied out from under a LOGICAL user",
            json!({"HomeDirectoryType": "LOGICAL", "HomeDirectoryMappings": mappings.clone()}),
            json!({"HomeDirectoryType": "PATH"}),
        ),
    ];

    for (index, (case, create_members, update)) in cases.into_iter().enumerate() {
        let emulator = Emulator::new();
        let server_id = emulator.create_server(json!({})).await;
        let user_name = format!("pair_{index}");
        let mut create = create_members;
        create["ServerId"] = json!(&server_id);
        create["UserName"] = json!(&user_name);
        create["Role"] = json!(ROLE);
        let created = emulator.call("CreateUser", create).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateUser 200, got {} with body {}",
            created.status, created.body
        );
        let before = described_type(&emulator, &server_id, &user_name).await;

        let mut request = update;
        request["ServerId"] = json!(&server_id);
        request["UserName"] = json!(&user_name);
        let refused = emulator.call("UpdateUser", request).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected the update to be refused 400 \
             InvalidRequestException, got {} with body {}",
            refused.status,
            refused.body
        );

        let after = described_type(&emulator, &server_id, &user_name).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused update to leave the stored type \
             {before:?}, got {after:?}"
        );
    }
}

/// RULE: `UpdateUser` assigns properties to a user the named server HOLDS; a server or
/// a user it does not hold is refused with `ResourceNotFoundException` and nothing is
/// created.
///
/// (Which 4xx the Transfer wire dresses that refusal in is the subject of its own
/// issue, so the rows assert the error CODE and a 4xx, as the sibling suites do.)
#[tokio::test]
async fn updating_a_user_no_server_holds_is_refused() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    emulator.create_user(&server_id, "present").await;

    // Rows: an unknown server, an unknown user on a real server, and a name differing
    // from a real one only in case.
    let cases: Vec<(&str, Value)> = vec![
        (
            "an unknown server",
            json!({"ServerId": "s-0123456789abcdef0", "UserName": "present"}),
        ),
        (
            "an unknown user",
            json!({"ServerId": &server_id, "UserName": "absent"}),
        ),
        (
            "a user differing only in case",
            json!({"ServerId": &server_id, "UserName": "PRESENT"}),
        ),
    ];

    for (case, mut request) in cases {
        request["HomeDirectory"] = json!("/bucket/nowhere");
        let answer = emulator.call("UpdateUser", request).await;
        assert!(
            (400..500).contains(&answer.status)
                && answer.error_code() == "ResourceNotFoundException",
            "case {case:?}: expected a 4xx ResourceNotFoundException, got {} with body {}",
            answer.status,
            answer.body
        );
    }

    // The refusals created nothing and changed nothing.
    let answer = emulator
        .call("ListUsers", json!({"ServerId": &server_id}))
        .await;
    let users = answer.get("Users").as_array().cloned().unwrap_or_default();
    let names: Vec<&str> = users
        .iter()
        .filter_map(|entry| member_of(entry, "UserName").as_str())
        .collect();
    assert_eq!(
        names,
        vec!["present"],
        "expected the refused updates to leave the stored set alone, got {names:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a user's SSH public keys are a stored set.
// ---------------------------------------------------------------------------

/// RULE: `ImportSshPublicKey` adds a key to the stored user under a minted
/// `^key-[0-9a-f]{17}$` id, `DeleteSshPublicKey` removes exactly the key that id names,
/// and `DescribeUser` reports the resulting set while `ListUsers` reports its live
/// count.
///
/// The rows differ along how many keys the user holds (one, two, three), whether the
/// first key came from the create or from an import, and which position in the set is
/// deleted (first, middle, last).
#[tokio::test]
async fn a_users_ssh_keys_are_the_set_the_imports_and_deletes_left() {
    // (case, the key body the create supplied, the bodies imported afterwards,
    //  the index of the key to delete).
    let cases: Vec<(&str, Option<&str>, Vec<&str>, usize)> = vec![
        ("one imported key, deleted", None, vec![SSH_KEY], 0),
        (
            "a create key plus one import, first deleted",
            Some(SSH_KEY),
            vec![SSH_KEY_2],
            0,
        ),
        (
            "three imported keys, the middle one deleted",
            None,
            vec![SSH_KEY, SSH_KEY_2, SSH_KEY_3],
            1,
        ),
        (
            "a create key plus two imports, the last deleted",
            Some(SSH_KEY),
            vec![SSH_KEY_2, SSH_KEY_3],
            2,
        ),
    ];

    for (case, create_key, imports, delete_at) in cases {
        let emulator = Emulator::new();
        let server_id = emulator.create_server(json!({})).await;
        let user_name = "keyholder";
        let mut create = json!({"ServerId": &server_id, "UserName": user_name, "Role": ROLE});
        if let Some(body) = create_key {
            create["SshPublicKeyBody"] = json!(body);
        }
        let created = emulator.call("CreateUser", create).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateUser 200, got {} with body {}",
            created.status, created.body
        );

        let mut want: Vec<String> = create_key.into_iter().map(str::to_string).collect();
        for body in &imports {
            let imported = emulator
                .call(
                    "ImportSshPublicKey",
                    json!({
                        "ServerId": &server_id,
                        "UserName": user_name,
                        "SshPublicKeyBody": body,
                    }),
                )
                .await;
            assert_eq!(
                imported.status, 200,
                "case {case:?}: expected ImportSshPublicKey 200, got {} with body {}",
                imported.status, imported.body
            );
            let key_id = imported.get("SshPublicKeyId").as_str().unwrap_or_default();
            assert!(
                is_modelled_key_id(key_id),
                "case {case:?}: expected a minted SshPublicKeyId matching \
                 ^key-[0-9a-f]{{17}}$, got {key_id:?}"
            );
            assert_eq!(
                (
                    imported.get("ServerId").as_str(),
                    imported.get("UserName").as_str()
                ),
                (Some(server_id.as_str()), Some(user_name)),
                "case {case:?}: expected the import to answer the user's own key, got {}",
                imported.body
            );
            want.push((*body).to_string());

            // Importing a body the user already holds does not store it twice.
            let repeat = emulator
                .call(
                    "ImportSshPublicKey",
                    json!({
                        "ServerId": &server_id,
                        "UserName": user_name,
                        "SshPublicKeyBody": body,
                    }),
                )
                .await;
            assert!(
                (400..500).contains(&repeat.status)
                    && repeat.error_code() == "ResourceExistsException",
                "case {case:?}: expected a repeat import to be refused with a 4xx \
                 ResourceExistsException, got {} with body {}",
                repeat.status,
                repeat.body
            );
        }

        let stored = described_keys(&emulator, &server_id, user_name).await;
        let bodies: Vec<String> = stored.iter().map(|(_, body)| body.clone()).collect();
        assert_eq!(
            bodies, want,
            "case {case:?}: expected the stored key set {want:?}, got {bodies:?}"
        );
        let ids: Vec<&String> = stored.iter().map(|(id, _)| id).collect();
        assert!(
            ids.iter().all(|id| is_modelled_key_id(id)),
            "case {case:?}: expected every stored key to carry a minted id, got {ids:?}"
        );
        assert_eq!(
            ids.iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            ids.len(),
            "case {case:?}: expected every stored key id to be distinct, got {ids:?}"
        );
        let entry = listed(&emulator, &server_id, user_name).await;
        assert_eq!(
            member_of(&entry, "SshPublicKeyCount").as_i64(),
            Some(want.len() as i64),
            "case {case:?}: expected ListUsers to count {} stored key(s), got {entry}",
            want.len()
        );

        // A key id the user does not hold is refused, and nothing is removed for it.
        let unknown = emulator
            .call(
                "DeleteSshPublicKey",
                json!({
                    "ServerId": &server_id,
                    "UserName": user_name,
                    "SshPublicKeyId": "key-0123456789abcdef0",
                }),
            )
            .await;
        assert!(
            (400..500).contains(&unknown.status)
                && unknown.error_code() == "ResourceNotFoundException",
            "case {case:?}: expected an unknown key id to be refused with a 4xx \
             ResourceNotFoundException, got {} with body {}",
            unknown.status,
            unknown.body
        );

        // The delete removes exactly the key its id names.
        let (doomed_id, doomed_body) = stored[delete_at].clone();
        let deleted = emulator
            .call(
                "DeleteSshPublicKey",
                json!({
                    "ServerId": &server_id,
                    "UserName": user_name,
                    "SshPublicKeyId": &doomed_id,
                }),
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteSshPublicKey 200, got {} with body {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body,
            json!({}),
            "case {case:?}: expected DeleteSshPublicKey to answer an empty body, got {}",
            deleted.body
        );

        want.retain(|body| *body != doomed_body);
        let after = described_keys(&emulator, &server_id, user_name).await;
        let after_bodies: Vec<String> = after.iter().map(|(_, body)| body.clone()).collect();
        assert_eq!(
            after_bodies, want,
            "case {case:?}: expected the key set {want:?} after the delete, got {after_bodies:?}"
        );
        assert!(
            after.iter().all(|(id, _)| *id != doomed_id),
            "case {case:?}: expected the deleted key id {doomed_id:?} to be gone, got {after:?}"
        );
        let entry = listed(&emulator, &server_id, user_name).await;
        assert_eq!(
            member_of(&entry, "SshPublicKeyCount").as_i64(),
            Some(want.len() as i64),
            "case {case:?}: expected ListUsers to count {} key(s) after the delete, got {entry}",
            want.len()
        );

        // Deleting it again is refused: the key is gone from every path.
        let twice = emulator
            .call(
                "DeleteSshPublicKey",
                json!({
                    "ServerId": &server_id,
                    "UserName": user_name,
                    "SshPublicKeyId": &doomed_id,
                }),
            )
            .await;
        assert!(
            (400..500).contains(&twice.status)
                && twice.error_code() == "ResourceNotFoundException",
            "case {case:?}: expected a second delete to be refused with a 4xx \
             ResourceNotFoundException, got {} with body {}",
            twice.status,
            twice.body
        );
    }
}

/// RULE: a user holds at most the service's published number of SSH public keys, so
/// the import that would pass the bound is refused while every one up to it is
/// stored — and a delete frees exactly one slot.
///
/// The rows are the counts around the published bound: one below it, AT it, and one
/// past it (the refused import), then the same import accepted after a delete. The
/// bound itself comes from `state::MAX_SSH_KEYS_PER_USER`, so this test follows the
/// quota rather than restating a number.
#[tokio::test]
async fn a_user_holds_at_most_the_published_number_of_ssh_keys() {
    const BOUND: usize =
        service_com_amazonaws_transfer_transferservice::state::MAX_SSH_KEYS_PER_USER;

    // The bound is AWS Transfer Family's published per-user quota, so the suite pins
    // the published number as well as walking whatever the constant says: a constant
    // that drifts away from the quota fails here rather than silently redefining it.
    assert_eq!(
        BOUND, 10,
        "expected the published quota of 10 SSH public keys per user, got {BOUND}"
    );
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    let user_name = "keybound";
    emulator.create_user(&server_id, user_name).await;

    // Key bodies that differ only in their base64 payload, so each import is a new
    // key rather than a repeat — `SshPublicKeyBody`'s modelled pattern admits any
    // base64 body.
    let bodies: Vec<String> = (0..=BOUND)
        .map(|index| format!("ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ{index:04}=="))
        .collect();

    for (index, body) in bodies.iter().enumerate() {
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
        let case = format!("key {} of a {BOUND}-key bound", index + 1);
        if index < BOUND {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the import to be accepted, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            // One past the bound: refused, and the stored set is untouched.
            assert_eq!(
                (answer.status, answer.error_code().as_str()),
                (400, "InvalidRequestException"),
                "case {case:?}: expected the import past the bound to be refused 400 \
                 InvalidRequestException, got {} with body {}",
                answer.status,
                answer.body
            );
        }
        let stored = described_keys(&emulator, &server_id, user_name).await;
        let want = (index + 1).min(BOUND);
        assert_eq!(
            stored.len(),
            want,
            "case {case:?}: expected {want} stored key(s), got {}",
            stored.len()
        );
    }

    // A delete frees exactly one slot, so the import refused above is accepted now.
    let stored = described_keys(&emulator, &server_id, user_name).await;
    let (freed_id, _) = stored.first().cloned().expect("the user holds keys");
    let deleted = emulator
        .call(
            "DeleteSshPublicKey",
            json!({
                "ServerId": &server_id,
                "UserName": user_name,
                "SshPublicKeyId": &freed_id,
            }),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteSshPublicKey 200, got {} with body {}",
        deleted.status, deleted.body
    );
    let accepted = emulator
        .call(
            "ImportSshPublicKey",
            json!({
                "ServerId": &server_id,
                "UserName": user_name,
                "SshPublicKeyBody": bodies.last().expect("a body"),
            }),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "expected the freed slot to take the import, got {} with body {}",
        accepted.status, accepted.body
    );
    let stored = described_keys(&emulator, &server_id, user_name).await;
    assert_eq!(
        stored.len(),
        BOUND,
        "expected the user to hold the bound again, got {}",
        stored.len()
    );
}

/// RULE: a key is imported into a user the named server HOLDS; an unknown server or
/// user is refused with `ResourceNotFoundException` rather than materialising one.
#[tokio::test]
async fn importing_a_key_for_a_user_no_server_holds_is_refused() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    emulator.create_user(&server_id, "present").await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "an unknown server",
            json!({"ServerId": "s-0123456789abcdef0", "UserName": "present"}),
        ),
        (
            "an unknown user",
            json!({"ServerId": &server_id, "UserName": "absent"}),
        ),
        (
            "a user differing only in case",
            json!({"ServerId": &server_id, "UserName": "PRESENT"}),
        ),
    ];

    for (case, mut request) in cases {
        request["SshPublicKeyBody"] = json!(SSH_KEY);
        let answer = emulator.call("ImportSshPublicKey", request.clone()).await;
        assert!(
            (400..500).contains(&answer.status)
                && answer.error_code() == "ResourceNotFoundException",
            "case {case:?}: expected ImportSshPublicKey to refuse with a 4xx \
             ResourceNotFoundException, got {} with body {}",
            answer.status,
            answer.body
        );

        request["SshPublicKeyId"] = json!("key-0123456789abcdef0");
        let answer = emulator.call("DeleteSshPublicKey", request).await;
        assert!(
            (400..500).contains(&answer.status)
                && answer.error_code() == "ResourceNotFoundException",
            "case {case:?}: expected DeleteSshPublicKey to refuse with a 4xx \
             ResourceNotFoundException, got {} with body {}",
            answer.status,
            answer.body
        );
    }

    // The real user still holds no key: nothing was materialised for the refusals.
    let stored = described_keys(&emulator, &server_id, "present").await;
    assert!(
        stored.is_empty(),
        "expected the refusals to store no key, got {stored:?}"
    );
}
