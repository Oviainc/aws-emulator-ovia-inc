// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RULE that an FSx not-found error names the identifier the
//! request supplied.
//!
//! Every not-found `Message` this service serializes is built from the request: the
//! `FileSystemId` the caller asked about for `FileSystemNotFound`, and the resource id in
//! the caller's `ResourceARN` for `ResourceNotFound`. A fixed sentence would tell a client
//! nothing about WHICH of the resources it named was missing, so these tables assert the
//! per-operation template over a span of identifiers rather than over one recorded id.
//!
//! The requests go through the same front door as `file_system_lifecycle.rs` (its harness is
//! reused), so nothing here reaches into the store directly.

use serde_json::{json, Value};
use service_com_amazonaws_fsx_awssimbaapiservice_v20180301::state::MAX_ERROR_MESSAGE_LEN;
use smithy_server_runtime::MultiServiceRouter;

use crate::file_system_lifecycle::{
    call, emulator, ok, with_client_request_token, Emulator, ACCOUNT, REGION,
};

/// Absent `FileSystemId`s spanning the modelled `FileSystemId` shape: pattern
/// `^(fs-[0-9a-f]{8,})$` with `length 11..21`, so the shortest legal id, a mid-length one
/// and the longest legal id, with differing hex payloads.
const ABSENT_FILE_SYSTEM_IDS: &[&str] = &[
    "fs-deadbeef",
    "fs-00000000deadbeef",
    "fs-0123456789abcdef0",
    "fs-ffffffffffffffffff",
];

/// One absent resource per type FSx owns, as `(resource_type, resource_id)`.
///
/// The types come from `crate::state::FSX_RESOURCE_TYPES`, which is itself one entry per id
/// shape the model declares (`FileSystemId` `fs-`, `BackupId` `backup-`, `VolumeId`
/// `fsvol-`, `SnapshotId` `(fs)?volsnap-`, `StorageVirtualMachineId` `svm-`, `TaskId`
/// `task-`, `AssociationId` `dra-`, `FileCacheId` `fc-`). None of these resources can exist
/// in this slice, so each resolves to `ResourceNotFound`.
const ABSENT_RESOURCES: &[(&str, &str)] = &[
    ("file-system", "fs-00000000deadbeef"),
    ("backup", "backup-0123456789abcdef0"),
    ("volume", "fsvol-0123456789abcdef0"),
    ("snapshot", "fsvolsnap-0123456789abcdef0"),
    ("storage-virtual-machine", "svm-0123456789abcdef0"),
    ("task", "task-0123456789abcdef0"),
    ("association", "dra-0123456789abcdef0"),
    ("file-cache", "fc-0123456789abcdef0"),
];

/// Dispatch one operation, require a failure, and return `(status, error code, Message)`.
async fn error_of(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, String, String) {
    let (status, parsed) = call(router, operation, body).await;
    assert_ne!(
        status, 200,
        "{operation} must fail, got HTTP 200 with body {parsed}"
    );
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        // awsJson error codes may be namespaced (`com.amazonaws.fsx#X`); clients match on
        // the shape name.
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = parsed
        .get("Message")
        .or_else(|| parsed.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("{operation} must carry an error Message, got body {parsed}");
        })
        .to_string();
    (status, code, message)
}

/// Create one file system and return its `(FileSystemId, ResourceARN)`.
async fn create_one(router: &Emulator) -> (String, String) {
    let created = ok(
        router,
        "CreateFileSystem",
        &with_client_request_token(&json!({
            "FileSystemType": "WINDOWS",
            "StorageCapacity": 32,
            "SubnetIds": [router.subnet_a()],
        })),
    )
    .await;
    let file_system = &created["FileSystem"];
    (
        file_system["FileSystemId"]
            .as_str()
            .expect("a created file system carries an id")
            .to_string(),
        file_system["ResourceARN"]
            .as_str()
            .expect("a created file system carries an ARN")
            .to_string(),
    )
}

/// Assert a not-found `Message` stays inside the modelled `ErrorMessage` `length 1..256`.
fn assert_message_length(case: &str, message: &str) {
    assert!(
        !message.is_empty() && message.len() <= MAX_ERROR_MESSAGE_LEN,
        "case {case:?}: a Message must hold 1..={MAX_ERROR_MESSAGE_LEN} characters, got \
         {} in {message:?}",
        message.len()
    );
}

/// RULE: `FileSystemNotFound`'s `Message` is built from the `FileSystemId` the request
/// named, in the template the operation uses — a quoted-id sentence for the operations that
/// report or update a file system, and the bare id for `DeleteFileSystem`.
///
/// The table spans the modelled `FileSystemId` shape (shortest legal id, longest legal id,
/// differing hex payloads), which is what the rule generalises over.
#[tokio::test]
async fn file_system_not_found_message_is_built_from_the_requested_id() {
    let router = emulator().await;
    let mut seen: Vec<String> = Vec::new();

    for id in ABSENT_FILE_SYSTEM_IDS {
        // A quoted-id sentence on the operations addressed by `FileSystemIds`/`FileSystemId`
        // that report or update.
        let sentence = format!("File system '{id}' does not exist.");
        for (operation, request) in [
            (
                "DescribeFileSystems",
                json!({ "FileSystemIds": [id] }),
            ),
            ("UpdateFileSystem", json!({ "FileSystemId": id })),
        ] {
            let case = format!("{operation} on absent {id}");
            let (status, code, message) = error_of(&router, operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "FileSystemNotFound"),
                "case {case:?}: expected 400 FileSystemNotFound, got {status} {code}"
            );
            assert_eq!(
                message, sentence,
                "case {case:?}: expected Message {sentence:?}, got {message:?}"
            );
            assert_message_length(&case, &message);
        }

        // `DeleteFileSystem`'s Message is the requested id alone.
        let case = format!("DeleteFileSystem on absent {id}");
        let (status, code, message) =
            error_of(&router, "DeleteFileSystem", &json!({ "FileSystemId": id })).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "FileSystemNotFound"),
            "case {case:?}: expected 400 FileSystemNotFound, got {status} {code}"
        );
        assert_eq!(
            message, *id,
            "case {case:?}: expected Message {id:?}, got {message:?}"
        );
        assert_message_length(&case, &message);
        seen.push(sentence);
    }

    // The Message is request-derived, so no two distinct ids produce the same one.
    let mut distinct = seen.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        seen.len(),
        "distinct ids must produce distinct Messages, got {seen:?}"
    );
}

/// RULE: the identifier in a not-found `Message` is the one the REQUEST named, whatever
/// else the request holds — which of several requested ids is missing, and whether the
/// file system exists at all.
#[tokio::test]
async fn describe_names_the_requested_id_that_is_missing() {
    let router = emulator().await;
    let (existing, _) = create_one(&router).await;
    let absent_a = "fs-00000000deadbeef";
    let absent_b = "fs-0123456789abcdef0";

    // (requested ids, the id the Message must name) — the selection count varies, the
    // missing id does not.
    let cases: &[(&[&str], &str)] = &[
        (&[absent_a], absent_a),
        (&[absent_b], absent_b),
        (&[existing.as_str(), absent_a], absent_a),
        (&[absent_b, existing.as_str()], absent_b),
    ];
    for case in cases {
        let (requested, missing) = *case;
        let (status, code, message) = error_of(
            &router,
            "DescribeFileSystems",
            &json!({ "FileSystemIds": requested }),
        )
        .await;
        let expected = format!("File system '{missing}' does not exist.");
        assert_eq!(
            (status, code.as_str()),
            (400, "FileSystemNotFound"),
            "case {case:?}: expected 400 FileSystemNotFound, got {status} {code}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected Message {expected:?}, got {message:?}"
        );
    }

    // Requesting only ids that exist is not a not-found at all.
    let listed = ok(
        &router,
        "DescribeFileSystems",
        &json!({ "FileSystemIds": [existing.clone()] }),
    )
    .await;
    assert_eq!(
        listed["FileSystems"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case only existing ids: expected one file system, got {:?}",
        listed["FileSystems"]
    );
}

/// RULE: `ResourceNotFound`'s `Message` is built from the resource id in the request's
/// `ResourceARN`, in the same template over whatever resource type the ARN names.
///
/// The table spans every resource type FSx owns (one per id shape the model declares) and
/// all three tagging operations.
#[tokio::test]
async fn tagging_not_found_message_is_built_from_the_requested_arn() {
    let router = emulator().await;

    for case in ABSENT_RESOURCES {
        let (resource_type, resource_id) = *case;
        let resource_arn = format!("arn:aws:fsx:{REGION}:{ACCOUNT}:{resource_type}/{resource_id}");
        let expected = format!("Resource '{resource_id}' does not exist");
        for operation in ["TagResource", "UntagResource", "ListTagsForResource"] {
            let request = match operation {
                "TagResource" => json!({
                    "ResourceARN": resource_arn,
                    "Tags": [{ "Key": "k", "Value": "v" }],
                }),
                "UntagResource" => json!({ "ResourceARN": resource_arn, "TagKeys": ["k"] }),
                _ => json!({ "ResourceARN": resource_arn }),
            };
            let label = format!("{operation} on absent {resource_type}/{resource_id}");
            let (status, code, message) = error_of(&router, operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ResourceNotFound"),
                "case {label:?}: expected 400 ResourceNotFound, got {status} {code}"
            );
            assert_eq!(
                message, expected,
                "case {label:?}: expected Message {expected:?}, got {message:?}"
            );
            assert_message_length(&label, &message);
        }
    }

    // The template reads the resource id out of the ARN, so an ARN for the same absent
    // resource in another region and account names the same resource.
    let resource_id = "fs-00000000deadbeef";
    let expected = format!("Resource '{resource_id}' does not exist");
    for resource_arn in [
        format!("arn:aws:fsx:eu-west-1:123456789012:file-system/{resource_id}"),
        format!("arn:aws-us-gov:fsx:us-gov-west-1:210987654321:file-system/{resource_id}"),
    ] {
        let (status, code, message) = error_of(
            &router,
            "ListTagsForResource",
            &json!({ "ResourceARN": resource_arn }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFound"),
            "case {resource_arn:?}: expected 400 ResourceNotFound, got {status} {code}"
        );
        assert_eq!(
            message, expected,
            "case {resource_arn:?}: expected Message {expected:?}, got {message:?}"
        );
    }

    // A tagging call on a resource that DOES exist is not a not-found, so the templated
    // Message is reached only when the lookup actually fails.
    let (_, arn) = create_one(&router).await;
    ok(
        &router,
        "TagResource",
        &json!({ "ResourceARN": arn, "Tags": [{ "Key": "k", "Value": "v" }] }),
    )
    .await;
}
