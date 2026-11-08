// SPDX-License-Identifier: CC0-1.0

//! Whether an error document CARRIES its `Message` member at all.
//!
//! RULE: every error document this service answers with carries the error shape's
//! `Message` member — a JSON string where a rule produces the sentence, and JSON `null`
//! for an error class no rule produces one for — so the key is never simply ABSENT, and
//! a client that distinguishes "this class carries no sentence" from "the sentence is
//! the empty string" reads the same thing from the emulator as from Amazon EFS. An SDK
//! substitutes the empty string for an absent key (botocore reports
//! `Error.Message == ''` for an absent key and `None` only for a key whose value is
//! JSON null), so dropping the member inverts the contract direction rather than
//! expressing it.
//!
//! Every error shape in this service's model is a structure with a required `ErrorCode`
//! and an OPTIONAL `Message` (target `ErrorMessage`), some with one more required member
//! naming the resource:
//! `smithy_query(query="shape",shape="AccessPointAlreadyExists,AccessPointNotFound,FileSystemAlreadyExists")`.
//! One class of this service's refusals — `AccessPointAlreadyExists` — carries no
//! sentence (neither the model nor the documentation states a rule that would produce
//! one; the actionable datum travels in its required `AccessPointId`), and the rest are
//! worded from the request by `crate::state`'s message helpers.
//!
//! The rule is checked over a TABLE that spans the class it covers: every error code
//! these operations can answer rather than one, several requests per code, several
//! operations per code, both the modelled failures and the two documents the framework
//! itself renders (a boundary `ValidationException` and the scaffold channel's 501).
//! The one request the issue quoted — a repeated `CreateAccessPoint` with the same
//! `ClientToken` — is one row.
//!
//! The rule is about the ERROR document only: the last test here pins that a SUCCESS
//! body still omits the optional members it has no value for, which is what every AWS
//! JSON response body does.

use serde_json::{json, Value};

use crate::file_system_lifecycle::{Emulator, Reply};

/// What an error document's message member is expected to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The class carries no sentence: the member is present and JSON `null`.
    Null,
    /// A rule produces the sentence: the member is present and a non-empty string.
    Sentence,
}

/// The `Message` slot of an error document: the JSON value under the key the error
/// shape's member occupies, or `None` when the document carries NO such key at all —
/// which is the difference this suite is about, and which the usual
/// `body["Message"].as_str()` reader cannot see.
pub(crate) fn message_slot(reply: &Reply) -> Option<Value> {
    let body = reply.body.as_object()?;
    // Both spellings: `Message` is what this service's model declares, `message` is the
    // Smithy default the framework's own synthetic error shapes use.
    for key in ["Message", "message"] {
        if let Some(value) = body.get(key) {
            return Some(value.clone());
        }
    }
    None
}

/// Assert one error document's message member is PRESENT and JSON `null`: the rendering
/// of an error class that carries no sentence.
pub(crate) fn assert_null_message_member(case: &str, reply: &Reply) {
    let Some(slot) = message_slot(reply) else {
        panic!(
            "case {case}: the error document omits the message key entirely, got {:?}",
            reply.body
        )
    };
    assert!(
        slot.is_null(),
        "case {case}: this error class carries no sentence, so the member must be JSON \
         null, got {slot:?} in {:?}",
        reply.body
    );
}

/// Assert one error document: its status, its error code, and that its message member is
/// present and exactly what the class is worded with.
fn assert_message_member(case: &str, reply: &Reply, status: u16, code: &str, expected: Expected) {
    assert_eq!(
        (reply.status, reply.error_type.as_deref()),
        (status, Some(code)),
        "case {case:?}: wrong refusal, body {:?}",
        reply.body
    );
    assert_eq!(
        reply.string("ErrorCode"),
        code,
        "case {case:?}: the required ErrorCode member must name the error, got {:?}",
        reply.get("ErrorCode")
    );
    match expected {
        Expected::Null => assert_null_message_member(&format!("{case:?}"), reply),
        Expected::Sentence => {
            let Some(slot) = message_slot(reply) else {
                panic!(
                    "case {case:?}: the error document omits the message key entirely, got {:?}",
                    reply.body
                )
            };
            let text = slot.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: expected a string message member, got {slot:?}")
            });
            assert!(
                !text.is_empty(),
                "case {case:?}: this class is worded from the request, so the member must \
                 carry a non-empty sentence, got {text:?}"
            );
        }
    }
}

/// The `ClientToken` values the rule is asserted over: the modelled length bounds
/// (`ClientToken`, `@length 1..64`, `pattern ^.+$`) and tokens whose characters are not
/// id-like, since the rendering is a property of the error class and not of the token.
fn creation_tokens() -> Vec<String> {
    vec![
        "t".to_string(),
        "null-message-probe-efs-ap".to_string(),
        "x".repeat(64),
        "token with spaces and .dots".to_string(),
    ]
}

/// RULE: the one error class that carries no sentence renders its `Message` member as
/// JSON null — over the `ClientToken` values the model admits, and whatever else the
/// repeated request says.
#[tokio::test]
async fn access_point_already_exists_carries_a_null_message_member() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("null-message-1").await;
    let other_file_system_id = efs.available_file_system("null-message-2").await;

    for token in creation_tokens() {
        let created = efs
            .create_access_point(json!({
                "ClientToken": token,
                "FileSystemId": file_system_id,
                "RootDirectory": { "Path": "/original-path" },
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {token:?}: CreateAccessPoint expected 200, got {} {:?}",
            created.status, created.body
        );
        let access_point_id = created.string("AccessPointId");

        // The repeats vary every member the request can carry besides the token: the
        // identical call, a different root directory, a different POSIX identity, added
        // tags, and another file system. The rendering is the same for all of them.
        let repeats = vec![
            (
                "identical parameters",
                json!({
                    "ClientToken": token,
                    "FileSystemId": file_system_id,
                    "RootDirectory": { "Path": "/original-path" },
                }),
            ),
            (
                "a different root directory",
                json!({
                    "ClientToken": token,
                    "FileSystemId": file_system_id,
                    "RootDirectory": { "Path": "/different-path" },
                }),
            ),
            (
                "a different POSIX identity",
                json!({
                    "ClientToken": token,
                    "FileSystemId": file_system_id,
                    "PosixUser": { "Uid": 1001, "Gid": 1001 },
                }),
            ),
            (
                "added tags",
                json!({
                    "ClientToken": token,
                    "FileSystemId": file_system_id,
                    "Tags": [{ "Key": "Name", "Value": "renamed" }],
                }),
            ),
            (
                "another file system",
                json!({ "ClientToken": token, "FileSystemId": other_file_system_id }),
            ),
        ];
        for (variation, body) in repeats {
            let case = format!("{token:?} repeated with {variation}");
            let repeat = efs.create_access_point(body).await;
            assert_message_member(
                &case,
                &repeat,
                409,
                "AccessPointAlreadyExists",
                Expected::Null,
            );
            // The required members still travel, and `AccessPointId` names the access
            // point that already holds the token.
            assert_eq!(
                repeat.string("AccessPointId"),
                access_point_id,
                "case {case:?}: AccessPointAlreadyExists$AccessPointId must be the existing \
                 id, got {:?}",
                repeat.get("AccessPointId")
            );
        }

        // ROUND TRIP: the refused repeats minted nothing, and the access point the
        // conflict named really is the one stored under that token.
        let described = efs
            .describe_access_points(&format!("AccessPointId={access_point_id}"))
            .await;
        let listed = described
            .get("AccessPoints")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            listed.len(),
            1,
            "case {token:?}: expected exactly the access point the conflict named, got \
             {listed:?}"
        );
        assert_eq!(
            listed[0]["ClientToken"].as_str(),
            Some(token.as_str()),
            "case {token:?}: the named access point holds the request's token, got {:?}",
            listed[0]
        );
        assert_eq!(
            listed[0]["RootDirectory"]["Path"].as_str(),
            Some("/original-path"),
            "case {token:?}: a refused repeat must not have rewritten the root directory, \
             got {:?}",
            listed[0]
        );
    }
}

/// RULE, over the codes this service answers: the `Message` member is present on EVERY
/// error document — JSON null for the class that carries no sentence, a non-empty string
/// for the classes worded from the request — so one code's rendering is not a special
/// case of another's.
#[tokio::test]
async fn every_error_document_carries_its_message_member() {
    let efs = Emulator::new();
    let mut rows: Vec<(String, Reply, u16, &str, Expected)> = Vec::new();

    // `FileSystemNotFound`, through three entry points and several id forms (bare id at
    // both bounds of `^fs-[0-9a-f]{8,40}$`, and the ARN the member also accepts).
    for requested in [
        "fs-00000000",
        "fs-0123456789abcdef",
        "fs-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        rows.push((
            format!("DescribeFileSystems({requested})"),
            efs.describe_file_systems(&format!("FileSystemId={requested}"))
                .await,
            404,
            "FileSystemNotFound",
            Expected::Sentence,
        ));
        rows.push((
            format!("DeleteFileSystem({requested})"),
            efs.delete_file_system(requested).await,
            404,
            "FileSystemNotFound",
            Expected::Sentence,
        ));
        rows.push((
            format!("CreateAccessPoint on {requested}"),
            efs.create_access_point(json!({
                "ClientToken": format!("missing-{requested}"),
                "FileSystemId": requested,
            }))
            .await,
            404,
            "FileSystemNotFound",
            Expected::Sentence,
        ));
    }

    // `AccessPointNotFound` and `MountTargetNotFound`: the other two not-found classes.
    for requested in ["fsap-00000000", "fsap-bbbbbbbbbbbbbbbb"] {
        rows.push((
            format!("DescribeAccessPoints({requested})"),
            efs.describe_access_points(&format!("AccessPointId={requested}"))
                .await,
            404,
            "AccessPointNotFound",
            Expected::Sentence,
        ));
        rows.push((
            format!("DeleteAccessPoint({requested})"),
            efs.delete_access_point(requested).await,
            404,
            "AccessPointNotFound",
            Expected::Sentence,
        ));
    }
    for requested in ["fsmt-00000000", "fsmt-0123456789abcdef"] {
        rows.push((
            format!("DeleteMountTarget({requested})"),
            efs.delete_mount_target(requested).await,
            404,
            "MountTargetNotFound",
            Expected::Sentence,
        ));
    }

    // `FileSystemAlreadyExists`: the OTHER already-exists class, whose sentence IS built
    // from the request — so the null rendering above is this class's, not "every
    // conflict's".
    for token in ["already-1", "already-2"] {
        let created = efs.create_file_system(json!({ "CreationToken": token })).await;
        assert_eq!(
            created.status, 201,
            "CreateFileSystem({token}): expected 201, got {} {:?}",
            created.status, created.body
        );
        rows.push((
            format!("CreateFileSystem({token}) repeated"),
            efs.create_file_system(json!({ "CreationToken": token })).await,
            409,
            "FileSystemAlreadyExists",
            Expected::Sentence,
        ));
    }

    // `IncorrectFileSystemLifeCycleState`: the file system is still `creating`, because
    // nothing waited out the clock-owed transition.
    let creating = efs
        .create_file_system(json!({ "CreationToken": "still-creating" }))
        .await;
    let creating_id = creating.string("FileSystemId");
    rows.push((
        "CreateAccessPoint while the file system is creating".to_string(),
        efs.create_access_point(json!({
            "ClientToken": "while-creating",
            "FileSystemId": creating_id,
        }))
        .await,
        409,
        "IncorrectFileSystemLifeCycleState",
        Expected::Sentence,
    ));

    // `BadRequest` and `UnsupportedAvailabilityZone`: refusals the handlers word from
    // the request.
    rows.push((
        "CreateAccessPoint with a RootDirectory and no Path".to_string(),
        efs.create_access_point(json!({
            "ClientToken": "no-path",
            "FileSystemId": creating_id,
            "RootDirectory": { "CreationInfo": { "OwnerUid": 1, "OwnerGid": 1, "Permissions": "0755" } },
        }))
        .await,
        400,
        "BadRequest",
        Expected::Sentence,
    ));
    rows.push((
        "CreateFileSystem in an Availability Zone of another Region".to_string(),
        efs.create_file_system(json!({
            "CreationToken": "foreign-az",
            "AvailabilityZoneName": "eu-west-1a",
        }))
        .await,
        400,
        "UnsupportedAvailabilityZone",
        Expected::Sentence,
    ));

    // `AccessPointAlreadyExists`: the class that carries none, in the same table.
    let file_system_id = efs.available_file_system("table-access-points").await;
    for token in ["table-ap-1", "table-ap-2"] {
        let created = efs
            .create_access_point(json!({ "ClientToken": token, "FileSystemId": file_system_id }))
            .await;
        assert_eq!(
            created.status, 200,
            "CreateAccessPoint({token}): expected 200, got {} {:?}",
            created.status, created.body
        );
        rows.push((
            format!("CreateAccessPoint({token}) repeated"),
            efs.create_access_point(json!({
                "ClientToken": token,
                "FileSystemId": file_system_id,
            }))
            .await,
            409,
            "AccessPointAlreadyExists",
            Expected::Null,
        ));
    }

    // The table spans the service's error vocabulary, and includes the one class that
    // carries no sentence — otherwise this test could pass on strings alone.
    let mut codes: Vec<&str> = rows.iter().map(|(_, _, _, code, _)| *code).collect();
    codes.sort_unstable();
    codes.dedup();
    assert!(
        codes.len() >= 7 && codes.contains(&"AccessPointAlreadyExists"),
        "the table must span the error codes, got {codes:?}"
    );

    for (case, reply, status, code, expected) in &rows {
        assert_message_member(case, reply, *status, code, *expected);
    }
}

/// RULE, for the two documents the framework itself renders: a boundary refusal and the
/// scaffold channel's 501 carry a message member too — theirs is always a non-empty
/// string, because both are built from the request or name the operation.
#[tokio::test]
async fn the_framework_documents_carry_a_string_message_member() {
    let efs = Emulator::new();
    let mut rows: Vec<(String, Reply, u16, &str)> = Vec::new();

    // A member outside its modelled enum never reaches the handler: the boundary refuses
    // it, with a message built from the request.
    for (member, value) in [("PerformanceMode", "turbo"), ("ThroughputMode", "unlimited")] {
        rows.push((
            format!("CreateFileSystem with {member}={value}"),
            efs.create_file_system(json!({
                "CreationToken": format!("boundary-{value}"),
                member: value,
            }))
            .await,
            400,
            "ValidationException",
        ));
    }

    // Two reads of this service are still scaffolded (they have their own stub issues),
    // so they answer the 501 whose message names the operation.
    let mount_target_id = "fsmt-0123456789abcdef";
    for (operation, path) in [
        ("DescribeAccountPreferences", "/2015-02-01/account-preferences".to_string()),
        (
            "DescribeMountTargetSecurityGroups",
            format!("/2015-02-01/mount-targets/{mount_target_id}/security-groups"),
        ),
    ] {
        rows.push((
            format!("{operation}({path})"),
            efs.send("GET", &path, None).await,
            501,
            "NotImplemented",
        ));
    }

    for (case, reply, status, code) in &rows {
        assert_eq!(
            (reply.status, reply.error_type.as_deref()),
            (*status, Some(*code)),
            "case {case:?}: wrong refusal, body {:?}",
            reply.body
        );
        let Some(slot) = message_slot(reply) else {
            panic!(
                "case {case:?}: the error document omits the message key entirely, got {:?}",
                reply.body
            )
        };
        let text = slot
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a string message, got {slot:?}"));
        assert!(
            !text.is_empty(),
            "case {case:?}: expected a non-empty message, got {text:?}"
        );
    }
}

/// RULE: the always-present member is a property of the ERROR document. A SUCCESS body
/// still omits every optional member the resource has no value for, which is what an AWS
/// JSON response body does.
#[tokio::test]
async fn a_success_body_omits_the_members_it_has_no_value_for() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("sparse-success").await;

    // No `PosixUser` and no `Name` tag, so the description has no value for either.
    let created = efs
        .create_access_point(json!({
            "ClientToken": "sparse-access-point",
            "FileSystemId": file_system_id,
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateAccessPoint: expected 200, got {} {:?}",
        created.status, created.body
    );
    for member in ["PosixUser", "Name", "Message"] {
        assert!(
            created.body.get(member).is_none(),
            "member {member:?}: an absent optional member stays OUT of a success body, got \
             {:?}",
            created.body.get(member)
        );
    }
    // And the members it does have a value for are there, so the check above is not
    // passing on an empty body.
    assert_eq!(
        created.string("FileSystemId"),
        file_system_id,
        "the access point's own members must still be reported, got {:?}",
        created.body
    );

    // The same body read back through DescribeAccessPoints.
    let described = efs
        .describe_access_points(&format!("FileSystemId={file_system_id}"))
        .await;
    let listed = described
        .get("AccessPoints")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        1,
        "expected the one stored access point, got {listed:?}"
    );
    for member in ["PosixUser", "Name"] {
        assert!(
            listed[0].get(member).is_none(),
            "member {member:?}: an absent optional member stays out of the read-back too, \
             got {:?}",
            listed[0].get(member)
        );
    }
}
