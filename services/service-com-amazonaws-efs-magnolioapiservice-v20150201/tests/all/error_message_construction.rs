// SPDX-License-Identifier: CC0-1.0

//! How EFS CONSTRUCTS the request-derived sentence in its refusals, asserted over a set
//! of inputs rather than on one recorded call.
//!
//! The rule under test: a refusal that names a resource the request identified renders
//! that id (or creation token) in SINGLE QUOTES inside the sentence, so the value can be
//! lifted back out of the text; a "not found" sentence is `<Noun> '<id>' does not exist.`,
//! terminated with a period, and `FileSystemAlreadyExists` names BOTH the id of the file
//! system that already holds the token and the `CreationToken` the request carried.
//!
//! The tables below vary what the rule generalises over — the id's hex length at both
//! modelled bounds (`^(fs|fsmt|fsap)-[0-9a-f]{8,40}$`), the digits themselves, the id's
//! FORM (bare id vs the ARN those members also accept), the resource the id names (file
//! system, mount target, access point), the entry point the id is resolved through
//! (describe, delete, create; query, path, body) and whether the id was never minted or
//! was minted and then deleted. The ids quoted in the issue are single rows.

use serde_json::json;

use crate::file_system_lifecycle::{settle, Emulator, Reply};

/// The values between successive single quotes in an error message — what a client that
/// extracts a quoted id from EFS text reads out of the sentence.
fn quoted_values(message: &str) -> Vec<String> {
    message
        .split('\'')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// Assert one refusal: the status, the error shape, and the message CONSTRUCTION — the
/// expected sentence, the single quoted value a client extracts from it, and the
/// terminating period.
fn assert_not_found(case: &str, reply: &Reply, error_type: &str, noun: &str, id: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected 404, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(error_type),
        "case {case:?}: expected error type {error_type:?}, got {:?}",
        reply.error_type
    );
    let message = reply.string("Message");
    assert_eq!(
        message,
        format!("{noun} '{id}' does not exist."),
        "case {case:?}: unexpected Message construction, got {message:?}"
    );
    assert_eq!(
        quoted_values(&message),
        vec![id.to_string()],
        "case {case:?}: the message must quote exactly the requested id, got {message:?}"
    );
    assert!(
        message.ends_with('.'),
        "case {case:?}: the not-found sentence is terminated with a period, got {message:?}"
    );
}

/// The entry points that resolve a `FileSystemId` and can refuse with
/// `FileSystemNotFound`. `takes_id_in_path` marks the ones that carry the id in the URI
/// path, which an ARN cannot travel through as one label.
#[derive(Debug, Clone, Copy)]
enum FileSystemEntry {
    DescribeFileSystems,
    DeleteFileSystem,
    DescribeMountTargets,
    CreateMountTarget,
    PutLifecycleConfiguration,
    DescribeLifecycleConfiguration,
    UpdateFileSystem,
}

impl FileSystemEntry {
    const ALL: [Self; 7] = [
        Self::DescribeFileSystems,
        Self::DeleteFileSystem,
        Self::DescribeMountTargets,
        Self::CreateMountTarget,
        Self::PutLifecycleConfiguration,
        Self::DescribeLifecycleConfiguration,
        Self::UpdateFileSystem,
    ];

    fn takes_id_in_path(self) -> bool {
        matches!(
            self,
            Self::DeleteFileSystem
                | Self::PutLifecycleConfiguration
                | Self::DescribeLifecycleConfiguration
                | Self::UpdateFileSystem
        )
    }

    async fn request(self, efs: &Emulator, requested: &str) -> Reply {
        match self {
            Self::DescribeFileSystems => {
                efs.describe_file_systems(&format!("FileSystemId={requested}"))
                    .await
            }
            Self::DeleteFileSystem => efs.delete_file_system(requested).await,
            Self::DescribeMountTargets => {
                efs.describe_mount_targets(&format!("FileSystemId={requested}"))
                    .await
            }
            Self::CreateMountTarget => {
                efs.create_mount_target(json!({
                    "FileSystemId": requested,
                    "SubnetId": "subnet-0123456789abcdef0",
                }))
                .await
            }
            Self::PutLifecycleConfiguration => {
                efs.send(
                    "PUT",
                    &format!("/2015-02-01/file-systems/{requested}/lifecycle-configuration"),
                    Some(json!({
                        "LifecyclePolicies": [{ "TransitionToIA": "AFTER_30_DAYS" }],
                    })),
                )
                .await
            }
            Self::DescribeLifecycleConfiguration => {
                efs.send(
                    "GET",
                    &format!("/2015-02-01/file-systems/{requested}/lifecycle-configuration"),
                    None,
                )
                .await
            }
            Self::UpdateFileSystem => {
                efs.send(
                    "PUT",
                    &format!("/2015-02-01/file-systems/{requested}"),
                    Some(json!({ "ThroughputMode": "elastic" })),
                )
                .await
            }
        }
    }
}

/// `FileSystemId` values no file system was ever created for, as `(what the request
/// carries, the file system the message must name)`. The hex bodies sit at the modelled
/// pattern's bounds (8 and 40 digits) and the last row is the ARN form the member also
/// accepts ("either a file system ID or ARN"), which names the same resource.
const MISSING_FILE_SYSTEMS: &[(&str, &str)] = &[
    ("fs-00000000", "fs-00000000"),
    ("fs-0123456789abcdef", "fs-0123456789abcdef"),
    (
        "fs-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "fs-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ),
    (
        "arn:aws:elasticfilesystem:us-east-1:000000000000:file-system/fs-deadbeefcafe",
        "fs-deadbeefcafe",
    ),
];

#[tokio::test]
async fn file_system_not_found_is_built_from_the_requested_id_at_every_entry_point() {
    let efs = Emulator::new();

    for (requested, named) in MISSING_FILE_SYSTEMS {
        for entry in FileSystemEntry::ALL {
            if entry.takes_id_in_path() && requested.starts_with("arn:") {
                continue;
            }
            let case = format!("{entry:?}({requested})");
            let reply = entry.request(&efs, requested).await;
            assert_not_found(&case, &reply, "FileSystemNotFound", "File system", named);
        }
    }
}

#[tokio::test]
async fn a_deleted_file_system_is_refused_with_the_same_construction_as_an_unknown_one() {
    let efs = Emulator::new();

    // A MINTED id, not a hand-written one: the same construction must hold for an id the
    // emulator itself produced and then released.
    let file_system_id = efs.available_file_system("construction-delete-1").await;
    let deleted = efs.delete_file_system(&file_system_id).await;
    assert_eq!(
        deleted.status, 204,
        "DeleteFileSystem({file_system_id}): expected 204, got {} {:?}",
        deleted.status, deleted.body
    );

    for entry in FileSystemEntry::ALL {
        let case = format!("{entry:?}(deleted {file_system_id})");
        let reply = entry.request(&efs, &file_system_id).await;
        assert_not_found(
            &case,
            &reply,
            "FileSystemNotFound",
            "File system",
            &file_system_id,
        );
    }
}

/// `MountTargetId` values no mount target holds: the modelled `^fsmt-[0-9a-f]{8,40}$`
/// pattern's shortest and longest bodies, through both entry points that resolve one.
const MISSING_MOUNT_TARGETS: &[&str] = &[
    "fsmt-00000000",
    "fsmt-0123456789abcdef",
    "fsmt-ffffffffffffffffffffffffffffffffffffffff",
];

#[tokio::test]
async fn mount_target_not_found_is_built_from_the_requested_id() {
    let efs = Emulator::new();

    for mount_target_id in MISSING_MOUNT_TARGETS {
        let deleted = efs.delete_mount_target(mount_target_id).await;
        assert_not_found(
            &format!("DeleteMountTarget({mount_target_id})"),
            &deleted,
            "MountTargetNotFound",
            "Mount target",
            mount_target_id,
        );

        let described = efs
            .describe_mount_targets(&format!("MountTargetId={mount_target_id}"))
            .await;
        assert_not_found(
            &format!("DescribeMountTargets(MountTargetId={mount_target_id})"),
            &described,
            "MountTargetNotFound",
            "Mount target",
            mount_target_id,
        );
    }
}

/// `AccessPointId` values, as `(what the request carries, the access point the message
/// must name)` — bare id at both pattern bounds, plus the ARN form the member accepts.
const MISSING_ACCESS_POINTS: &[(&str, &str)] = &[
    ("fsap-00000000", "fsap-00000000"),
    (
        "fsap-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "fsap-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    ),
    (
        "arn:aws:elasticfilesystem:us-east-1:000000000000:access-point/fsap-0f1e2d3c",
        "fsap-0f1e2d3c",
    ),
];

#[tokio::test]
async fn access_point_not_found_is_built_from_the_requested_id() {
    let efs = Emulator::new();

    for (requested, named) in MISSING_ACCESS_POINTS {
        let reply = efs
            .describe_mount_targets(&format!("AccessPointId={requested}"))
            .await;
        assert_not_found(
            &format!("DescribeMountTargets(AccessPointId={requested})"),
            &reply,
            "AccessPointNotFound",
            "Access point",
            named,
        );
    }
}

/// The `CreationToken` values the already-exists rule is asserted over: the modelled
/// length bounds (`length 1..64`, `pattern ^.+$`), a hyphenated probe token, and a token
/// whose characters are not id-like, since the rule quotes whatever the request carried.
fn creation_tokens() -> Vec<String> {
    vec![
        "t".to_string(),
        "sim2real-probe-efs-fs-lifecycle-93866a91".to_string(),
        "x".repeat(64),
        "token with spaces and .dots".to_string(),
    ]
}

#[tokio::test]
async fn file_system_already_exists_names_the_existing_file_system_and_the_request_token() {
    let efs = Emulator::new();

    for token in creation_tokens() {
        let created = efs
            .create_file_system(json!({ "CreationToken": token }))
            .await;
        assert_eq!(
            created.status, 201,
            "case {token:?}: CreateFileSystem expected 201, got {} {:?}",
            created.status, created.body
        );
        let file_system_id = created.string("FileSystemId");

        // The conflict: the same token again.
        let repeat = efs
            .create_file_system(json!({ "CreationToken": token }))
            .await;
        assert_eq!(
            repeat.status, 409,
            "case {token:?}: repeat CreateFileSystem expected 409, got {} {:?}",
            repeat.status, repeat.body
        );
        assert_eq!(
            repeat.error_type.as_deref(),
            Some("FileSystemAlreadyExists"),
            "case {token:?}: unexpected error type, got {:?}",
            repeat.error_type
        );
        let message = repeat.string("Message");
        assert_eq!(
            message,
            format!("File system '{file_system_id}' already exists with creation token '{token}'"),
            "case {token:?}: unexpected Message construction, got {message:?}"
        );
        // The two actionable values are both extractable from the sentence, in order.
        assert_eq!(
            quoted_values(&message),
            vec![file_system_id.clone(), token.clone()],
            "case {token:?}: expected the existing id then the request token quoted, got \
             {message:?}"
        );
        // The modelled member carries the same id the message names.
        assert_eq!(
            repeat.string("FileSystemId"),
            file_system_id,
            "case {token:?}: FileSystemAlreadyExists$FileSystemId must be the existing id, got \
             {:?}",
            repeat.get("FileSystemId")
        );

        // ROUND TRIP: the conflict named a file system that really is stored under that
        // token, and the repeat created no second one.
        let described = efs
            .describe_file_systems(&format!("FileSystemId={file_system_id}"))
            .await;
        assert_eq!(
            described.status, 200,
            "case {token:?}: DescribeFileSystems expected 200, got {} {:?}",
            described.status, described.body
        );
        let listed = described
            .get("FileSystems")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            listed.len(),
            1,
            "case {token:?}: expected exactly the one file system the conflict named, got \
             {listed:?}"
        );
        assert_eq!(
            listed[0]["CreationToken"].as_str().unwrap_or_default(),
            token,
            "case {token:?}: the named file system holds the request's token, got {:?}",
            listed[0]
        );
    }
}

#[tokio::test]
async fn an_existing_file_system_is_not_refused_by_the_not_found_rule() {
    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("construction-positive-1").await;
    settle().await;

    // The refusal is keyed to existence, not to the sentence: the same entry points
    // answer the stored resource.
    let described = efs
        .describe_file_systems(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        described.status, 200,
        "DescribeFileSystems({file_system_id}): expected 200, got {} {:?}",
        described.status, described.body
    );
    let mount_targets = efs
        .describe_mount_targets(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        mount_targets.status, 200,
        "DescribeMountTargets({file_system_id}): expected 200, got {} {:?}",
        mount_targets.status, mount_targets.body
    );
    assert!(
        described.string("Message").is_empty() && mount_targets.string("Message").is_empty(),
        "an existing file system carries no refusal message, got {:?} / {:?}",
        described.body,
        mount_targets.body
    );
}
