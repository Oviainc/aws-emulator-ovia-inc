// SPDX-License-Identifier: CC0-1.0

//! The encryption key a repository's metadata reports, through every projection
//! that reports one, and the `tagrisId` the tag listing carries — driven as
//! awsJson1_1 requests through the router that serves the real emulator.
//!
//! Declared in `tests/all.rs`; the wire harness (`emulator`, `ok`, `error`,
//! `string_at`, `is_uuid`) comes from the sibling `repository_lifecycle` suite and
//! the `GetRepository` projection, the managed-key alias and the key-ARN form from
//! the shared `harness` module, rather than a second copy.
//!
//! The rule under test: a repository is always encrypted, so `kmsKeyId` is
//! always reported — the service's managed key for a request that names no
//! selector of its own, and otherwise the one canonical form of the selector the
//! caller set: a bare key id as the ARN of that key in the repository's own
//! account and region, every other form KMS documents for `KeyId` as it was set.
//! `CreateRepository`, `GetRepository` and `BatchGetRepositories` project one
//! stored repository, so all three report that same key, and a selector that
//! names no key at all is refused and stores nothing.
//!
//! The tag listing reports the id of the tagging record it read beside the tags
//! themselves: an id of the form this service mints every opaque id in, stable
//! across reads and across a later tag write, and distinct per repository.
//!
//! The table below spans what the rule generalises over — every selector form
//! KMS documents for `KeyId` plus the multi-region key id, a request naming no
//! selector at all, repositories with and without tags, and the modelled name
//! bounds — rather than the one recorded request.

use serde_json::{json, Value};

use super::harness::{kms_key_arn_of, metadata_of, DEFAULT_ENCRYPTION_KEY_ALIAS};
use super::repository_lifecycle::{emulator, error, is_uuid, ok, string_at, REGION};

/// The account every ARN this emulator mints is scoped to.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// One row: the repository, the `kmsKeyId` its creation carries (`None` for a
/// request that names none), the key every projection of it must report, and the
/// tags it is created with.
struct Case {
    name: String,
    requested: Option<String>,
    reported: String,
    tags: Vec<(&'static str, &'static str)>,
}

/// The rows. The selector forms are the four the Key Management Service
/// documents for `KeyId` — the values `kmsKeyId`'s own documentation sends a
/// caller to ("see KeyId in the Decrypt API description in the Key Management
/// Service API Reference",
/// `smithy_query(query="doc",shape="CreateRepositoryInput",member="kmsKeyId")`):
/// a key id, a key ARN, an alias name and an alias ARN — plus the multi-region
/// key id (`mrk-` and 32 hexadecimal digits), which is the second shape KMS
/// mints a key id in, and a request that names no key at all.
fn cases() -> Vec<Case> {
    let key_id = "3f9e4b2a-11cd-4e6f-98ab-0123456789ab";
    let other_key_id = "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9";
    let multi_region_key_id = format!("mrk-{}", "0f1e2d3c".repeat(4));
    vec![
        Case {
            // No selector at all: the managed key, in its alias form.
            name: "key-default".to_string(),
            requested: None,
            reported: DEFAULT_ENCRYPTION_KEY_ALIAS.to_string(),
            tags: vec![("team", "builders")],
        },
        Case {
            // A bare key id leaves the key's account and region unsaid, so it
            // resolves to that key's ARN.
            name: "key-by-id".to_string(),
            requested: Some(key_id.to_string()),
            reported: kms_key_arn_of(key_id),
            tags: Vec::new(),
        },
        Case {
            // The multi-region shape of a key id resolves the same way.
            name: "key.by_multi-region-id".to_string(),
            requested: Some(multi_region_key_id.clone()),
            reported: kms_key_arn_of(&multi_region_key_id),
            tags: vec![("env", "test"), ("owner", "team-a")],
        },
        Case {
            // A key ARN already names the key in full and is kept as it was set.
            name: "k".to_string(),
            requested: Some(kms_key_arn_of(other_key_id)),
            reported: kms_key_arn_of(other_key_id),
            tags: Vec::new(),
        },
        Case {
            // An alias names a key this emulator resolves no further, so the
            // alias is what the repository carries.
            name: "key-by-alias".to_string(),
            requested: Some("alias/codecommit-repository-key".to_string()),
            reported: "alias/codecommit-repository-key".to_string(),
            tags: vec![("a", "1")],
        },
        Case {
            // The modelled maximum `RepositoryName` length (100), created with
            // an alias ARN.
            name: "x".repeat(100),
            requested: Some(format!(
                "arn:aws:kms:{REGION}:{}:alias/codecommit-repository-key",
                account()
            )),
            reported: format!(
                "arn:aws:kms:{REGION}:{}:alias/codecommit-repository-key",
                account()
            ),
            tags: Vec::new(),
        },
    ]
}

/// A `CreateRepository` body for one row.
fn create_body(case: &Case) -> Value {
    let mut body = json!({ "repositoryName": case.name });
    if let Some(requested) = &case.requested {
        body["kmsKeyId"] = json!(requested);
    }
    if !case.tags.is_empty() {
        body["tags"] = Value::Object(
            case.tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect(),
        );
    }
    body
}

#[tokio::test]
async fn every_projection_reports_the_repositorys_key_in_its_canonical_form() {
    let router = emulator();
    let cases = cases();

    for case in &cases {
        let label = &case.name;
        let created = ok(&router, "CreateRepository", &create_body(case)).await;
        let created_metadata = created
            .get("repositoryMetadata")
            .unwrap_or_else(|| panic!("case {label:?}: no repositoryMetadata in {created}"))
            .clone();
        assert_eq!(
            created_metadata.get("kmsKeyId").and_then(Value::as_str),
            Some(case.reported.as_str()),
            "case {label:?}: CreateRepository must report the repository's key, got {created_metadata}"
        );

        // The key is stored, not just answered: the read path reports it too,
        // and reports the whole projection identically.
        let read_back = metadata_of(&router, &case.name).await;
        assert_eq!(
            read_back.get("kmsKeyId").and_then(Value::as_str),
            Some(case.reported.as_str()),
            "case {label:?}: GetRepository must report the repository's key, got {read_back}"
        );
        assert_eq!(
            read_back, created_metadata,
            "case {label:?}: the create and the read must project one repository identically"
        );
    }

    // The batch read projects the same repositories: one entry per requested
    // name, in the order the request lists them, each carrying its own key.
    let requested: Vec<String> = cases.iter().rev().map(|case| case.name.clone()).collect();
    let payload = ok(
        &router,
        "BatchGetRepositories",
        &json!({ "repositoryNames": requested }),
    )
    .await;
    let entries = payload
        .get("repositories")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no repositories in {payload}"))
        .clone();
    assert_eq!(
        entries
            .iter()
            .map(|entry| string_at(entry, "/repositoryName"))
            .collect::<Vec<String>>(),
        requested,
        "the batch must report the requested names in request order, got {payload}"
    );
    for case in &cases {
        let label = &case.name;
        let entry = entries
            .iter()
            .find(|entry| string_at(entry, "/repositoryName") == case.name)
            .unwrap_or_else(|| panic!("case {label:?}: not reported in {payload}"));
        assert_eq!(
            entry.get("kmsKeyId").and_then(Value::as_str),
            Some(case.reported.as_str()),
            "case {label:?}: the batch must report the repository's key, got {entry}"
        );
        assert_eq!(
            entry,
            &metadata_of(&router, &case.name).await,
            "case {label:?}: the batch and the single read must project one repository identically"
        );
    }
}

/// The `tagrisId` a tag listing reports for a resource ARN.
async fn tagris_id_of(router: &smithy_server_runtime::MultiServiceRouter, arn: &str) -> String {
    let payload = ok(
        router,
        "ListTagsForResource",
        &json!({ "resourceArn": arn }),
    )
    .await;
    payload
        .get("tagrisId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no tagrisId in {payload}"))
        .to_string()
}

#[tokio::test]
async fn the_tag_listing_reports_the_resources_own_tagging_id() {
    let router = emulator();
    let cases = cases();
    let mut seen: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();

    for case in &cases {
        let label = &case.name;
        let created = ok(&router, "CreateRepository", &create_body(case)).await;
        let arn = string_at(&created, "/repositoryMetadata/Arn");

        let id = tagris_id_of(&router, &arn).await;
        assert!(
            is_uuid(&id),
            "case {label:?}: tagrisId must be an id of the form this service mints, got {id:?}"
        );
        // An id, not a per-call nonce: a second read of the same resource
        // reports the same one, and so does a read after a tag write.
        assert_eq!(
            tagris_id_of(&router, &arn).await,
            id,
            "case {label:?}: a second read must report the same tagrisId"
        );
        ok(
            &router,
            "TagResource",
            &json!({ "resourceArn": arn, "tags": { "written": "later" } }),
        )
        .await;
        assert_eq!(
            tagris_id_of(&router, &arn).await,
            id,
            "case {label:?}: a tag write must not change the resource's tagrisId"
        );

        if let Some(other) = seen.iter().find(|(_, other_id)| **other_id == id) {
            panic!(
                "case {label:?}: tagrisId {id} is already the id of {:?}",
                other.0
            );
        }
        seen.insert(case.name.clone(), id);
    }
    assert_eq!(
        seen.len(),
        cases.len(),
        "every repository must report a tagging id of its own, got {seen:?}"
    );
}

#[tokio::test]
async fn a_kms_key_id_that_selects_no_key_is_refused_and_stores_nothing() {
    let router = emulator();

    // Each row is a value the modelled pattern `^[a-zA-Z0-9:/_-]+$` admits but
    // which names a key in none of the forms KMS documents for `KeyId`, so it
    // selects no key at all.
    let refused: Vec<(&str, String)> = vec![
        ("a value that is no key id at all", "not-a-kms-key".to_string()),
        (
            "a key id with too few groups",
            "3f9e4b2a-11cd-4e6f-98ab".to_string(),
        ),
        (
            "a key id group that is not hexadecimal",
            "3f9e4b2a-11cd-4e6f-98ab-0123456789zz".to_string(),
        ),
        (
            "a multi-region key id one digit short",
            format!("mrk-{}", "0".repeat(31)),
        ),
        ("an alias with no name", "alias/".to_string()),
        (
            "an ARN of another vendor",
            format!(
                "arn:aws:s3:{REGION}:{}:key/3f9e4b2a-11cd-4e6f-98ab-0123456789ab",
                account()
            ),
        ),
        (
            "a KMS ARN naming no key or alias resource",
            format!(
                "arn:aws:kms:{REGION}:{}:cmk/3f9e4b2a-11cd-4e6f-98ab-0123456789ab",
                account()
            ),
        ),
    ];

    for (index, (why, requested)) in refused.iter().enumerate() {
        let name = format!("refused-{index}");
        let code = error(
            &router,
            "CreateRepository",
            &json!({ "repositoryName": name, "kmsKeyId": requested }),
            400,
        )
        .await;
        assert_eq!(
            code, "EncryptionKeyInvalidIdException",
            "case {why:?}: kmsKeyId {requested:?} must be refused as an invalid key id"
        );

        // The refusal stored nothing: the name it carried names no repository.
        let code = error(
            &router,
            "GetRepository",
            &json!({ "repositoryName": name }),
            400,
        )
        .await;
        assert_eq!(
            code, "RepositoryDoesNotExistException",
            "case {why:?}: the refused create must not have stored {name:?}"
        );
    }

    let listed = ok(&router, "ListRepositories", &json!({})).await;
    assert_eq!(
        listed
            .get("repositories")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "no refused create may leave a repository behind, got {listed}"
    );
}
