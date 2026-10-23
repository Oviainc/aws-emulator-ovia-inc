// SPDX-License-Identifier: CC0-1.0

//! `UpdateRepositoryEncryptionKey` over the wire, driven as awsJson1_1 requests
//! through the router that serves the real emulator.
//!
//! The rule under test: a rotation writes the requested key onto the STORED
//! repository, so every later read of its metadata reports that key and the call
//! answers with the repository's id, the key now in effect and the key it left
//! behind (absent when the repository held none); nothing else about the
//! repository — its id, name, description, default branch, ARN, clone URLs,
//! creation date or content — moves with it, the last-modified instant records
//! the change, and a `kmsKeyId` that selects no KMS key, or a repository the
//! account does not hold, is refused with the modelled error and changes nothing.
//!
//! The tables below span the class the rule covers — every form KMS documents for
//! `KeyId` (key id, key ARN, alias name, alias ARN, multi-region key id),
//! repositories created with a key and without one, rotations chained and
//! repeated, and repositories with and without history — rather than the one
//! recorded request. The harness (`emulator`, `ok`, `error`, `string_at`,
//! `is_uuid`) is the shared one in [`crate::repository_lifecycle`].

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_lifecycle::{emulator, error, is_uuid, ok, string_at, REGION};

/// The account every ARN this emulator mints is scoped to.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The four forms the Key Management Service documents for `KeyId` — the values
/// `kmsKeyId`'s own documentation sends a caller to ("see KeyId in the Decrypt API
/// description in the Key Management Service API Reference",
/// `smithy_query(query="doc",shape="UpdateRepositoryEncryptionKeyInput",member="kmsKeyId")`):
/// a key id, a key ARN, an alias name and an alias ARN. The multi-region key id
/// (`mrk-` and 32 hexadecimal digits) is the second shape KMS mints a key id in.
fn key_id() -> String {
    "1234abcd-12ab-34cd-56ef-1234567890ab".to_string()
}

fn second_key_id() -> String {
    "fedcba98-7654-3210-fedc-ba9876543210".to_string()
}

fn multi_region_key_id() -> String {
    format!("mrk-{}", "a1b2c3d4".repeat(4))
}

fn key_arn() -> String {
    format!("arn:aws:kms:{REGION}:{}:key/{}", account(), key_id())
}

fn alias_name() -> String {
    "alias/codecommit-repository-key".to_string()
}

fn alias_arn() -> String {
    format!("arn:aws:kms:{REGION}:{}:alias/aws_codecommit", account())
}

/// The `repositoryMetadata` a `GetRepository` read reports for a name.
async fn metadata_of(router: &MultiServiceRouter, name: &str) -> Value {
    let got = ok(router, "GetRepository", &json!({ "repositoryName": name })).await;
    got.get("repositoryMetadata")
        .unwrap_or_else(|| panic!("no repositoryMetadata for {name:?} in {got}"))
        .clone()
}

fn as_f64(metadata: &Value, member: &str) -> f64 {
    metadata
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no {member} in {metadata}"))
}

/// Every metadata member a rotation must leave exactly as it was: a rotation
/// moves which key the repository depends on, not the repository.
const CARRIED_OVER: &[&str] = &[
    "accountId",
    "repositoryId",
    "repositoryName",
    "repositoryDescription",
    "defaultBranch",
    "creationDate",
    "cloneUrlHttp",
    "cloneUrlSsh",
    "Arn",
];

/// Write one file so a rotated repository has a branch and a history to carry
/// over.
async fn put_file(router: &MultiServiceRouter, repository: &str, branch: &str, path: &str) {
    ok(
        router,
        "PutFile",
        &json!({
            "repositoryName": repository,
            "branchName": branch,
            "filePath": path,
            "fileContent": aws_smithy_types::base64::encode(path.as_bytes()),
            "commitMessage": "init",
            "name": "Test Author",
            "email": "author@example.com",
        }),
    )
    .await;
}

/// One row of the rotation table: the repository, the key it was created with (or
/// none at all), the keys it is rotated through in order, and whether it carries
/// a file.
#[derive(Debug, Clone)]
struct Case {
    name: String,
    created_with: Option<String>,
    rotations: Vec<String>,
    content: Option<(&'static str, &'static str)>,
}

/// The table: rows differ along what the rule generalises over — the form of the
/// key (every form KMS documents for `KeyId`, plus the multi-region key id), the
/// repository's starting state (created with a key, created with none), the number
/// of rotations (one, chained, and one repeating the key already in effect), and
/// whether the repository holds history.
fn rotation_cases() -> Vec<Case> {
    vec![
        Case {
            // Created with a key ARN, then walked through every other form.
            name: "rotate-every-form".to_string(),
            created_with: Some(key_arn()),
            rotations: vec![
                key_id(),
                alias_name(),
                alias_arn(),
                multi_region_key_id(),
                second_key_id(),
            ],
            content: Some(("main", "README.md")),
        },
        Case {
            // Created with no key of its own: the first rotation leaves none
            // behind, so `originalKmsKeyId` is absent for it.
            name: "rotate.from_none-1".to_string(),
            created_with: None,
            rotations: vec![key_id(), key_arn()],
            content: None,
        },
        Case {
            // A rotation to the key already in effect: accepted, and it changes
            // nothing — not even the last-modified instant.
            name: "r".to_string(),
            created_with: Some(alias_name()),
            rotations: vec![alias_name(), alias_name(), key_arn()],
            content: Some(("release-1.0", "/src/main/app.sh")),
        },
        Case {
            // The modelled maximum `RepositoryName` length (100), rotated once.
            name: "x".repeat(100),
            created_with: Some(multi_region_key_id()),
            rotations: vec![alias_arn()],
            content: None,
        },
    ]
}

#[tokio::test]
async fn a_rotation_moves_the_stored_repositorys_key_and_reports_the_one_it_left() {
    let router = emulator();

    for case in rotation_cases() {
        let mut create = json!({ "repositoryName": case.name, "repositoryDescription": "rotated" });
        if let Some(key) = &case.created_with {
            create["kmsKeyId"] = json!(key);
        }
        ok(&router, "CreateRepository", &create).await;
        if let Some((branch, path)) = case.content {
            put_file(&router, &case.name, branch, path).await;
        }

        let stored_file = match case.content {
            Some((_, path)) => Some(
                ok(
                    &router,
                    "GetFile",
                    &json!({ "repositoryName": case.name, "filePath": path }),
                )
                .await,
            ),
            None => None,
        };

        let mut in_effect = case.created_with.clone();
        for requested in &case.rotations {
            let label = format!("{} -> {requested}", case.name);
            let before = metadata_of(&router, &case.name).await;
            assert_eq!(
                before.get("kmsKeyId").and_then(Value::as_str),
                in_effect.as_deref(),
                "case {label:?}: the key in effect before the rotation, got {before}"
            );

            // Separate the modification instants so lastModifiedDate orders.
            std::thread::sleep(Duration::from_millis(5));

            let rotated = ok(
                &router,
                "UpdateRepositoryEncryptionKey",
                &json!({ "repositoryName": case.name, "kmsKeyId": requested }),
            )
            .await;

            // The response reports the repository, the key now in effect and the
            // key it left behind.
            let repository_id = string_at(&rotated, "/repositoryId");
            assert!(
                is_uuid(&repository_id),
                "case {label:?}: repositoryId must be a uuid, got {repository_id:?}"
            );
            assert_eq!(
                repository_id,
                string_at(&before, "/repositoryId"),
                "case {label:?}: the rotation must report the repository's own id"
            );
            assert_eq!(
                rotated.get("kmsKeyId").and_then(Value::as_str),
                Some(requested.as_str()),
                "case {label:?}: kmsKeyId must be the requested key, got {rotated}"
            );
            assert_eq!(
                rotated.get("originalKmsKeyId").and_then(Value::as_str),
                in_effect.as_deref(),
                "case {label:?}: originalKmsKeyId must be the key formerly in effect, got {rotated}"
            );

            // The dependency moved, not just the response field: the stored
            // repository reports the new key to every later reader.
            let after = metadata_of(&router, &case.name).await;
            assert_eq!(
                after.get("kmsKeyId").and_then(Value::as_str),
                Some(requested.as_str()),
                "case {label:?}: GetRepository must report the rotated key, got {after}"
            );

            // Nothing else about the repository moved.
            for member in CARRIED_OVER {
                assert_eq!(
                    after.get(*member),
                    before.get(*member),
                    "case {label:?}: member {member} must carry over, got {after}"
                );
            }
            if let (Some((_, path)), Some(stored_file)) = (case.content, stored_file.as_ref()) {
                let read_back = ok(
                    &router,
                    "GetFile",
                    &json!({ "repositoryName": case.name, "filePath": path }),
                )
                .await;
                assert_eq!(
                    &read_back, stored_file,
                    "case {label:?}: the repository's content must read back identically"
                );
            }

            // The last-modified instant records a rotation that changed the key,
            // and a rotation to the key already in effect changes nothing.
            let modified = as_f64(&after, "lastModifiedDate");
            let was_modified = as_f64(&before, "lastModifiedDate");
            if in_effect.as_deref() == Some(requested.as_str()) {
                assert_eq!(
                    modified, was_modified,
                    "case {label:?}: rotating to the key already in effect must change nothing"
                );
            } else {
                assert!(
                    modified > was_modified,
                    "case {label:?}: the rotation must move lastModifiedDate forward, got {modified}"
                );
            }
            assert!(
                modified >= as_f64(&after, "creationDate"),
                "case {label:?}: lastModifiedDate must not precede creationDate, got {after}"
            );

            in_effect = Some(requested.clone());
        }
    }
}

/// The names `ListRepositories` reports, in the order it reports them.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    let listed = ok(router, "ListRepositories", &json!({})).await;
    listed
        .get("repositories")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no repositories in {listed}"))
        .iter()
        .map(|entry| string_at(entry, "/repositoryName"))
        .collect()
}

#[tokio::test]
async fn a_rotation_the_rules_refuse_changes_nothing() {
    let router = emulator();
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "rotation-source", "kmsKeyId": key_arn() }),
    )
    .await;
    ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "rotation-other" }),
    )
    .await;
    let source_before = metadata_of(&router, "rotation-source").await;
    let other_before = metadata_of(&router, "rotation-other").await;

    // Each row: why the rotation is refused, the repository and key sent, and the
    // modelled error. The invalid-id rows are values the modelled pattern
    // `^[a-zA-Z0-9:/_-]+$` admits but which select no KMS key in any of the forms
    // KMS documents for `KeyId`.
    let refusals: Vec<(&str, &str, String, &str)> = vec![
        (
            "a repository the account does not hold",
            "rotation-absent",
            key_arn(),
            "RepositoryDoesNotExistException",
        ),
        (
            "a key id that is not an id in either shape KMS mints",
            "rotation-source",
            "not-a-kms-key".to_string(),
            "EncryptionKeyInvalidIdException",
        ),
        (
            "a key id with too few groups",
            "rotation-source",
            "1234abcd-12ab-34cd-56ef".to_string(),
            "EncryptionKeyInvalidIdException",
        ),
        (
            "a key id group that is not hexadecimal",
            "rotation-source",
            "1234abcd-12ab-34cd-56ef-1234567890zz".to_string(),
            "EncryptionKeyInvalidIdException",
        ),
        (
            "a key id whose groups are not the widths KMS mints",
            "rotation-source",
            "1234abc-12ab-34cd-56ef-1234567890abc".to_string(),
            "EncryptionKeyInvalidIdException",
        ),
        (
            "a multi-region key id one digit short",
            "rotation-source",
            format!("mrk-{}", "a".repeat(31)),
            "EncryptionKeyInvalidIdException",
        ),
        (
            "an alias with no name",
            "rotation-source",
            "alias/".to_string(),
            "EncryptionKeyInvalidIdException",
        ),
        (
            "an ARN of another vendor",
            "rotation-source",
            format!("arn:aws:s3:{REGION}:{}:key/{}", account(), key_id()),
            "EncryptionKeyInvalidIdException",
        ),
        (
            "a KMS ARN naming no key or alias resource",
            "rotation-source",
            format!("arn:aws:kms:{REGION}:{}:cmk/{}", account(), key_id()),
            "EncryptionKeyInvalidIdException",
        ),
        (
            "a KMS key ARN whose account is not an account id",
            "rotation-source",
            format!("arn:aws:kms:{REGION}:not-an-account:key/{}", key_id()),
            "EncryptionKeyInvalidIdException",
        ),
        (
            "a KMS key ARN with no region",
            "rotation-source",
            format!("arn:aws:kms::{}:key/{}", account(), key_id()),
            "EncryptionKeyInvalidIdException",
        ),
    ];

    for (why, repository, requested, expected) in refusals {
        let code = error(
            &router,
            "UpdateRepositoryEncryptionKey",
            &json!({ "repositoryName": repository, "kmsKeyId": requested }),
            400,
        )
        .await;
        assert_eq!(
            code, expected,
            "case {why:?}: repository {repository:?} kmsKeyId {requested:?} must be refused with {expected}"
        );
        assert_eq!(
            metadata_of(&router, "rotation-source").await,
            source_before,
            "case {why:?}: a refused rotation must leave rotation-source exactly as it was"
        );
        assert_eq!(
            metadata_of(&router, "rotation-other").await,
            other_before,
            "case {why:?}: a refused rotation must leave rotation-other exactly as it was"
        );
    }

    // A `kmsKeyId` that carries no character at all is refused by the modelled
    // `pattern` before the handler runs, so the refusal is the boundary
    // validator's rather than a modelled CodeCommit error; what matters to the
    // rule is that it is refused and the repository keeps its key.
    let code = error(
        &router,
        "UpdateRepositoryEncryptionKey",
        &json!({ "repositoryName": "rotation-source", "kmsKeyId": "" }),
        400,
    )
    .await;
    assert_eq!(
        code, "ValidationException",
        "case {:?}: a blank kmsKeyId is outside the modelled pattern",
        "a blank key"
    );
    assert_eq!(
        metadata_of(&router, "rotation-source").await,
        source_before,
        "case {:?}: a refused rotation must leave rotation-source exactly as it was",
        "a blank key"
    );

    // The same for a request that names no key at all: `kmsKeyId` is modelled
    // required, so the boundary validator refuses it.
    let code = error(
        &router,
        "UpdateRepositoryEncryptionKey",
        &json!({ "repositoryName": "rotation-source" }),
        400,
    )
    .await;
    assert_eq!(
        code, "ValidationException",
        "case {:?}: a request naming no kmsKeyId is outside the modelled input",
        "no key at all"
    );
    assert_eq!(
        metadata_of(&router, "rotation-source").await,
        source_before,
        "case {:?}: a refused rotation must leave rotation-source exactly as it was",
        "no key at all"
    );

    // No refused rotation minted a repository under the name it was refused for.
    let code = error(
        &router,
        "GetRepository",
        &json!({ "repositoryName": "rotation-absent" }),
        400,
    )
    .await;
    assert_eq!(
        code, "RepositoryDoesNotExistException",
        "a refused rotation must not create the repository it named"
    );
    assert_eq!(
        listed_names(&router).await,
        vec!["rotation-other".to_string(), "rotation-source".to_string()],
        "a refused rotation must not change which repositories the account holds"
    );
}

/// A rotation is addressed by repository, so one repository's rotation is not
/// another's: two repositories sharing a key part ways when only one is rotated.
#[tokio::test]
async fn a_rotation_moves_only_the_repository_it_names() {
    let router = emulator();
    for name in ["shared-a", "shared-b"] {
        ok(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": name, "kmsKeyId": key_arn() }),
        )
        .await;
    }

    let rotated = ok(
        &router,
        "UpdateRepositoryEncryptionKey",
        &json!({ "repositoryName": "shared-a", "kmsKeyId": second_key_id() }),
    )
    .await;
    assert_eq!(
        rotated.get("originalKmsKeyId").and_then(Value::as_str),
        Some(key_arn().as_str()),
        "the rotation reports the key shared-a left, got {rotated}"
    );

    let rotated_metadata = metadata_of(&router, "shared-a").await;
    let untouched = metadata_of(&router, "shared-b").await;
    assert_eq!(
        rotated_metadata.get("kmsKeyId").and_then(Value::as_str),
        Some(second_key_id().as_str()),
        "case {:?}: the rotated repository holds the new key, got {rotated_metadata}",
        "shared-a"
    );
    assert_eq!(
        untouched.get("kmsKeyId").and_then(Value::as_str),
        Some(key_arn().as_str()),
        "case {:?}: the repository that was not named keeps its key, got {untouched}",
        "shared-b"
    );
    assert_ne!(
        string_at(&rotated_metadata, "/repositoryId"),
        string_at(&untouched, "/repositoryId"),
        "the two repositories are distinct resources"
    );
}
