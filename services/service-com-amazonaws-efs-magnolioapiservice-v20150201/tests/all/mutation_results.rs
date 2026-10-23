// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the rule that the RESULT of an accepted EFS mutation is what the
//! next response reporting it serves — and for the one `FileSystemDescription` serializer
//! every path that reports a file system goes through.
//!
//! Three rules, each asserted over a TABLE rather than on one call:
//!
//! 1. A change `UpdateFileSystem` accepts (HTTP 202) is reported only once it has been
//!    applied: the accepting response and every read until the transition completes carry
//!    the file system's CURRENT throughput in the `updating` phase, and the requested
//!    throughput afterwards. The rows vary the starting mode, the requested mode and the
//!    provisioned amount.
//! 2. A `FileSystemDescription` reports every `FileSystemSize` member, with `Timestamp`
//!    only where a metering time exists, and reports `KmsKeyId` as the KEY the requested
//!    key identifier resolves to. The rows vary the configuration and walk every
//!    `KmsKeyId` form the modelled pattern admits. Create, describe and update are
//!    asserted to answer the same description, since they share one serializer.
//! 3. The deprecated tag paths (`CreateTags`, `DeleteTags`) write and remove the ONE
//!    stored tag set the current paths (`TagResource`, `UntagResource`) write, and that
//!    both tag reads (`DescribeTags`, `ListTagsForResource`) and the description's `Tags`
//!    report. The rows vary the tag count, the key and value forms, whether a key is new
//!    or overwritten, and the form of the id (bare id or file-system ARN).
//!
//! The harness (`Emulator`, `Reply`, `settle`) and the `Tags` helpers are the ones
//! `file_system_lifecycle.rs` and `tags.rs` already drive this service with.

use serde_json::{json, Value};
use service_com_amazonaws_efs_magnolioapiservice_v20150201::state;

use crate::file_system_lifecycle::{settle, Emulator, Reply};
use crate::tags::{as_set, numbered_tags, reported, tags_json};

// ---------------------------------------------------------------------------
// The HTTP bindings this suite adds, and the readers every rule is checked through.
// ---------------------------------------------------------------------------

/// `PUT /2015-02-01/file-systems/{FileSystemId}` — the `UpdateFileSystem` binding
/// (`smithy_query(query="operation",operation="UpdateFileSystem")`).
async fn update_file_system(efs: &Emulator, file_system_id: &str, body: Value) -> Reply {
    efs.send(
        "PUT",
        &format!("/2015-02-01/file-systems/{file_system_id}"),
        Some(body),
    )
    .await
}

/// `POST /2015-02-01/create-tags/{FileSystemId}` — the deprecated `CreateTags` binding,
/// whose modelled success status is 204
/// (`smithy_query(query="operation",operation="CreateTags")`).
async fn create_tags(efs: &Emulator, file_system_id: &str, tags: &[(String, String)]) -> Reply {
    efs.send(
        "POST",
        &format!("/2015-02-01/create-tags/{file_system_id}"),
        Some(json!({ "Tags": tags_json(tags) })),
    )
    .await
}

/// `POST /2015-02-01/delete-tags/{FileSystemId}` — the deprecated `DeleteTags` binding,
/// also a modelled 204 (`smithy_query(query="operation",operation="DeleteTags")`).
async fn delete_tags(efs: &Emulator, file_system_id: &str, keys: &[&str]) -> Reply {
    efs.send(
        "POST",
        &format!("/2015-02-01/delete-tags/{file_system_id}"),
        Some(json!({ "TagKeys": keys })),
    )
    .await
}

/// `GET /2015-02-01/tags/{FileSystemId}` — the deprecated `DescribeTags` read.
async fn describe_tags(efs: &Emulator, file_system_id: &str) -> Reply {
    efs.send("GET", &format!("/2015-02-01/tags/{file_system_id}"), None)
        .await
}

/// `GET /2015-02-01/resource-tags/{ResourceId}` — the current `ListTagsForResource` read.
async fn list_tags(efs: &Emulator, resource_id: &str) -> Reply {
    efs.send(
        "GET",
        &format!("/2015-02-01/resource-tags/{resource_id}"),
        None,
    )
    .await
}

/// `POST /2015-02-01/resource-tags/{ResourceId}` — the current `TagResource` write.
async fn tag_resource(efs: &Emulator, resource_id: &str, tags: &[(String, String)]) -> Reply {
    efs.send(
        "POST",
        &format!("/2015-02-01/resource-tags/{resource_id}"),
        Some(json!({ "Tags": tags_json(tags) })),
    )
    .await
}

/// The stored description of one file system, read back through `DescribeFileSystems` —
/// so an assertion sees what the NEXT caller sees, not what a mutation echoed.
async fn stored(efs: &Emulator, file_system_id: &str) -> Value {
    let listed = efs
        .describe_file_systems(&format!("FileSystemId={file_system_id}"))
        .await;
    assert_eq!(
        listed.status, 200,
        "DescribeFileSystems({file_system_id}): expected 200, got {} {:?}",
        listed.status, listed.body
    );
    listed.get("FileSystems")[0].clone()
}

/// Create one file system and wait out the clock-owed `creating` -> `available`
/// transition, so an update or a tag write can be applied to it.
async fn settled_file_system(efs: &Emulator, body: Value) -> String {
    let created = efs.create_file_system(body.clone()).await;
    assert_eq!(
        created.status, 201,
        "case {body:?}: CreateFileSystem expected 201, got {} {:?}",
        created.status, created.body
    );
    let id = created.string("FileSystemId");
    settle().await;
    id
}

/// `ProvisionedThroughputInMibps` as a number, absent member included (`None`).
fn mibps_of(description: &Value) -> Option<f64> {
    description["ProvisionedThroughputInMibps"].as_f64()
}

/// The `(Key, Value)` pairs of a file-system description's own `Tags` member.
fn description_tags(description: &Value) -> Vec<(String, String)> {
    description["Tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// An ARN as an SDK puts it in a non-greedy path label: the `/` percent-encoded.
fn as_label(arn: &str) -> String {
    arn.replace('/', "%2F")
}

// ---------------------------------------------------------------------------
// Rule 1 — an accepted throughput change is reported only once it is applied.
// ---------------------------------------------------------------------------

/// One row of the throughput-visibility table: what the file system is created as, the
/// change the update asks for, what it must still report while the transition runs, and
/// what it must report once the transition has applied the change.
struct ThroughputCase {
    case: &'static str,
    created_with: Value,
    requested: Value,
    mode_before: &'static str,
    mibps_before: Option<f64>,
    mode_after: &'static str,
    mibps_after: Option<f64>,
}

#[tokio::test]
async fn an_accepted_throughput_change_is_reported_only_once_it_is_applied() {
    // The modes are the modelled `ThroughputMode` closure
    // (`smithy_query(query="shape",shape="ThroughputMode")` -> bursting | elastic |
    // provisioned) and the amounts the ends of the documented 1-3414 MiBps range, so the
    // rows cover a mode change in each direction, an amount-only change up and down, and
    // both published bounds.
    let cases: Vec<ThroughputCase> = vec![
        ThroughputCase {
            case: "bursting -> elastic",
            created_with: json!({ "CreationToken": "vis-1" }),
            requested: json!({ "ThroughputMode": "elastic" }),
            mode_before: "bursting",
            mibps_before: None,
            mode_after: "elastic",
            mibps_after: None,
        },
        ThroughputCase {
            case: "elastic -> bursting",
            created_with: json!({ "CreationToken": "vis-2", "ThroughputMode": "elastic" }),
            requested: json!({ "ThroughputMode": "bursting" }),
            mode_before: "elastic",
            mibps_before: None,
            mode_after: "bursting",
            mibps_after: None,
        },
        ThroughputCase {
            case: "bursting -> provisioned at the lower bound",
            created_with: json!({ "CreationToken": "vis-3" }),
            requested: json!({
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": 1.0,
            }),
            mode_before: "bursting",
            mibps_before: None,
            mode_after: "provisioned",
            mibps_after: Some(1.0),
        },
        ThroughputCase {
            case: "provisioned -> elastic clears the amount",
            created_with: json!({
                "CreationToken": "vis-4",
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": 3414.0,
            }),
            requested: json!({ "ThroughputMode": "elastic" }),
            mode_before: "provisioned",
            mibps_before: Some(3414.0),
            mode_after: "elastic",
            mibps_after: None,
        },
        ThroughputCase {
            case: "an increase of the provisioned amount, with no mode member",
            created_with: json!({
                "CreationToken": "vis-5",
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": 100.0,
            }),
            requested: json!({ "ProvisionedThroughputInMibps": 512.0 }),
            mode_before: "provisioned",
            mibps_before: Some(100.0),
            mode_after: "provisioned",
            mibps_after: Some(512.0),
        },
        ThroughputCase {
            case: "a decrease of the provisioned amount",
            created_with: json!({
                "CreationToken": "vis-6",
                "ThroughputMode": "provisioned",
                "ProvisionedThroughputInMibps": 500.0,
            }),
            requested: json!({ "ProvisionedThroughputInMibps": 250.0 }),
            mode_before: "provisioned",
            mibps_before: Some(500.0),
            mode_after: "provisioned",
            mibps_after: Some(250.0),
        },
    ];

    let efs = Emulator::new();
    for row in cases {
        let ThroughputCase {
            case,
            created_with,
            requested,
            mode_before,
            mibps_before,
            mode_after,
            mibps_after,
        } = row;
        let id = settled_file_system(&efs, created_with).await;

        let accepted = update_file_system(&efs, &id, requested).await;
        assert_eq!(
            accepted.status, 202,
            "case {case:?}: expected HTTP 202, got {} {:?}",
            accepted.status, accepted.body
        );
        // The 202 is an ACCEPTANCE: the file system is moved into the transition and goes
        // on reporting the throughput it still has.
        assert_eq!(
            accepted.string("LifeCycleState"),
            "updating",
            "case {case:?}: expected LifeCycleState \"updating\", got {:?}",
            accepted.get("LifeCycleState")
        );
        assert_eq!(
            accepted.string("ThroughputMode"),
            mode_before,
            "case {case:?}: expected the current ThroughputMode {mode_before:?}, got {:?}",
            accepted.get("ThroughputMode")
        );
        assert_eq!(
            mibps_of(&accepted.body),
            mibps_before,
            "case {case:?}: expected the current ProvisionedThroughputInMibps \
             {mibps_before:?}, got {:?}",
            accepted.get("ProvisionedThroughputInMibps")
        );

        // A caller polling the file system sees the same, so a read-after-write loop
        // observes the transition instead of exiting before one exists.
        let polled = stored(&efs, &id).await;
        assert_eq!(
            (
                polled["LifeCycleState"].as_str(),
                polled["ThroughputMode"].as_str(),
                mibps_of(&polled)
            ),
            (Some("updating"), Some(mode_before), mibps_before),
            "case {case:?}: the transition is what a read observes, got {polled:?}"
        );

        // Completing the transition is what applies the change — and it is durable.
        settle().await;
        let applied = stored(&efs, &id).await;
        assert_eq!(
            (
                applied["LifeCycleState"].as_str(),
                applied["ThroughputMode"].as_str(),
                mibps_of(&applied)
            ),
            (Some("available"), Some(mode_after), mibps_after),
            "case {case:?}: expected the applied change, got {applied:?}"
        );
        let re_read = stored(&efs, &id).await;
        assert_eq!(
            re_read, applied,
            "case {case:?}: a second read reports the same file system, got {re_read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — one FileSystemDescription serializer: the FileSystemSize member set, and
// KmsKeyId as the key the request's key identifier resolves to.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_description_carries_the_full_size_member_set_and_no_metering_time() {
    // `FileSystemSize` (`smithy_query(query="shape",shape="FileSystemSize")`): `Value`
    // (required), `ValueInIA`, `ValueInStandard`, `ValueInArchive` — each "the latest
    // known metered size (in bytes) of data stored in" its storage class — and
    // `Timestamp`, "the time at which the size of data, returned in the Value field, was
    // determined". Nothing stores data in this emulator, so every class reports zero and
    // no metering time exists; a class holding nothing is still REPORTED.
    let cases: Vec<(&str, Value)> = vec![
        ("all defaults", json!({ "CreationToken": "size-1" })),
        (
            "elastic throughput",
            json!({ "CreationToken": "size-2", "ThroughputMode": "elastic" }),
        ),
        (
            "encrypted, One Zone, tagged",
            json!({
                "CreationToken": "size-3",
                "Encrypted": true,
                "AvailabilityZoneName": "us-east-1b",
                "Tags": [{ "Key": "Name", "Value": "sized" }],
            }),
        ),
    ];

    let efs = Emulator::new();
    for (case, create_body) in cases {
        let created = efs.create_file_system(create_body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        let id = created.string("FileSystemId");
        settle().await;

        // The update path answers the same shape through the same serializer, so the
        // 202's description is checked beside the create's and the read's.
        let accepted = update_file_system(&efs, &id, json!({ "ThroughputMode": "elastic" })).await;
        let descriptions = [
            ("CreateFileSystem", created.get("SizeInBytes").clone()),
            ("DescribeFileSystems", stored(&efs, &id).await["SizeInBytes"].clone()),
            ("UpdateFileSystem", accepted.get("SizeInBytes").clone()),
        ];
        for (path, size) in descriptions {
            assert_eq!(
                size,
                json!({
                    "Value": 0,
                    "ValueInIA": 0,
                    "ValueInStandard": 0,
                    "ValueInArchive": 0,
                }),
                "case {case:?} on {path}: expected the full member set with no metering \
                 time, got {size:?}"
            );
            assert_eq!(
                size["Timestamp"],
                Value::Null,
                "case {case:?} on {path}: SizeInBytes.Timestamp exists only once a \
                 metering time does, got {size:?}"
            );
        }
        settle().await;
    }
}

#[tokio::test]
async fn encryption_is_reported_on_every_file_system_either_way() {
    // `FileSystemDescription$Encrypted`: "A Boolean value that, if true, indicates that
    // the file system is encrypted" — a fact every file system has, so it is reported on
    // an unencrypted one as well; `KmsKeyId` is "the ID of an KMS key used to protect the
    // encrypted file system", so only an encrypted file system has one.
    // (case, CreateFileSystem body, expected Encrypted, whether a key is reported).
    let cases: Vec<(&str, Value, bool, bool)> = vec![
        (
            "no Encrypted member",
            json!({ "CreationToken": "enc-1" }),
            false,
            false,
        ),
        (
            "Encrypted false",
            json!({ "CreationToken": "enc-2", "Encrypted": false }),
            false,
            false,
        ),
        (
            "Encrypted true",
            json!({ "CreationToken": "enc-3", "Encrypted": true }),
            true,
            true,
        ),
    ];

    let efs = Emulator::new();
    for (case, create_body, encrypted, has_key) in cases {
        let created = efs.create_file_system(create_body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        let id = created.string("FileSystemId");
        settle().await;
        let read = stored(&efs, &id).await;
        for (path, description) in [
            ("CreateFileSystem", &created.body),
            ("DescribeFileSystems", &read),
        ] {
            assert_eq!(
                description["Encrypted"],
                json!(encrypted),
                "case {case:?} on {path}: expected Encrypted {encrypted}, got {description:?}"
            );
            assert_eq!(
                description["KmsKeyId"].is_string(),
                has_key,
                "case {case:?} on {path}: a key is reported only for an encrypted file \
                 system, got {description:?}"
            );
        }
    }
}

/// The `(region, account, key id)` of a KMS KEY ARN, or `None` when `value` is not one.
/// The form is the one the modelled `KmsKeyId` pattern admits beside the alias forms
/// (`smithy_query(query="shape",shape="KmsKeyId")`):
/// `arn:aws[-a-z]*:kms:[a-z0-9-]+:\d{12}:key/<key id>`.
fn key_arn_parts(value: &str) -> Option<(String, String, String)> {
    let fields: Vec<&str> = value.splitn(6, ':').collect();
    if fields.len() != 6 || fields[0] != "arn" || fields[2] != "kms" {
        return None;
    }
    let key_id = fields[5].strip_prefix("key/")?;
    if fields[4].len() != 12 || !fields[4].bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    is_key_id(key_id).then(|| {
        (
            fields[3].to_string(),
            fields[4].to_string(),
            key_id.to_string(),
        )
    })
}

/// Whether `value` is a KMS key id the modelled `KmsKeyId` pattern admits: the
/// `8-4-4-4-12` lowercase-hex form, or `mrk-` and 32 hex digits for a multi-Region key.
fn is_key_id(value: &str) -> bool {
    let hex = |group: &str, len: usize| {
        group.len() == len
            && group
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    if let Some(multi_region) = value.strip_prefix("mrk-") {
        return hex(multi_region, 32);
    }
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| hex(group, *len))
}

/// One row of the `KmsKeyId` table: the form the request names the key by, and what the
/// description must then report — the Region and account of the reported key ARN, and the
/// key id itself where the request already named a key rather than an alias.
struct KeyCase {
    case: &'static str,
    requested: Option<&'static str>,
    region: &'static str,
    account: &'static str,
    key_id: Option<&'static str>,
}

#[tokio::test]
async fn each_kms_key_form_is_reported_as_the_key_it_resolves_to() {
    // `CreateFileSystem$KmsKeyId` takes "the ID of the KMS key that you want to use to
    // protect the encrypted file system", in any of the forms the modelled pattern admits
    // (`smithy_query(query="shape",shape="KmsKeyId")`): a key id, `mrk-…`, `alias/NAME`,
    // or an ARN of any of those. `FileSystemDescription$KmsKeyId` is "the ID of an KMS
    // key used to protect the encrypted file system" — the KEY — so an alias, which is a
    // pointer to a key and not a key, is reported as the key it resolves to.
    let cases: Vec<KeyCase> = vec![
        KeyCase {
            // "If you don't specify a KMS key, then the default KMS key for Amazon EFS,
            // /aws/elasticfilesystem, is used" — an alias, so it resolves.
            case: "no key at all: the default EFS key",
            requested: None,
            region: "us-east-1",
            account: "000000000000",
            key_id: None,
        },
        KeyCase {
            case: "a bare alias",
            requested: Some("alias/projectKey1"),
            region: "us-east-1",
            account: "000000000000",
            key_id: None,
        },
        KeyCase {
            case: "an alias ARN in another Region",
            requested: Some("arn:aws:kms:eu-west-1:000000000000:alias/projectKey1"),
            region: "eu-west-1",
            account: "000000000000",
            key_id: None,
        },
        KeyCase {
            case: "a bare key id",
            requested: Some("1234abcd-12ab-34cd-56ef-1234567890ab"),
            region: "us-east-1",
            account: "000000000000",
            key_id: Some("1234abcd-12ab-34cd-56ef-1234567890ab"),
        },
        KeyCase {
            // The multi-Region key-id form of the same modelled pattern, which no
            // recording in this issue cited.
            case: "a multi-Region key id",
            requested: Some("mrk-1234abcd1234abcd1234abcd1234abcd"),
            region: "us-east-1",
            account: "000000000000",
            key_id: Some("mrk-1234abcd1234abcd1234abcd1234abcd"),
        },
        KeyCase {
            case: "a key ARN of another account",
            requested: Some(
                "arn:aws:kms:us-west-2:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            ),
            region: "us-west-2",
            account: "111122223333",
            key_id: Some("1234abcd-12ab-34cd-56ef-1234567890ab"),
        },
    ];

    let efs = Emulator::new();
    let mut reported_keys: Vec<(&str, String)> = Vec::new();
    for (index, row) in cases.into_iter().enumerate() {
        let KeyCase {
            case,
            requested,
            region,
            account,
            key_id,
        } = row;
        let mut body = json!({ "CreationToken": format!("key-{index}"), "Encrypted": true });
        if let Some(requested) = requested {
            body["KmsKeyId"] = json!(requested);
        }
        let created = efs.create_file_system(body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} {:?}",
            created.status, created.body
        );
        let id = created.string("FileSystemId");
        let key = created.string("KmsKeyId");

        let parts = key_arn_parts(&key);
        assert!(
            parts.is_some(),
            "case {case:?}: expected a KMS key ARN the modelled KmsKeyId pattern admits, \
             got {key:?}"
        );
        let (reported_region, reported_account, reported_key_id) = parts.expect("a key ARN");
        assert_eq!(
            reported_region, region,
            "case {case:?}: expected the key in {region}, got {key:?}"
        );
        assert_eq!(
            reported_account, account,
            "case {case:?}: expected the key in account {account}, got {key:?}"
        );
        if let Some(key_id) = key_id {
            assert_eq!(
                reported_key_id, key_id,
                "case {case:?}: a request naming a key reports that key, got {key:?}"
            );
        }

        // Every path that reports this file system reports the same key: one serializer,
        // one stored record.
        settle().await;
        let read = stored(&efs, &id).await;
        assert_eq!(
            read["KmsKeyId"].as_str(),
            Some(key.as_str()),
            "case {case:?}: DescribeFileSystems must report the same key, got {read:?}"
        );
        let accepted = update_file_system(&efs, &id, json!({ "ThroughputMode": "elastic" })).await;
        assert_eq!(
            accepted.string("KmsKeyId"),
            key,
            "case {case:?}: UpdateFileSystem must report the same key, got {:?}",
            accepted.get("KmsKeyId")
        );
        settle().await;
        reported_keys.push((case, key));
    }

    // One alias always names the same key, whichever form it was written in, and two
    // aliases never name the same one: a client may pin the reported key.
    let key_of = |case: &str| {
        reported_keys
            .iter()
            .find(|(name, _)| *name == case)
            .map(|(_, key)| key.clone())
            .unwrap_or_default()
    };
    let bare_alias = key_of("a bare alias");
    let alias_arn = key_of("an alias ARN in another Region");
    assert_eq!(
        key_arn_parts(&bare_alias).map(|parts| parts.2),
        key_arn_parts(&alias_arn).map(|parts| parts.2),
        "one alias resolves to one key however it is written, got {bare_alias:?} and \
         {alias_arn:?}"
    );
    assert_ne!(
        key_arn_parts(&bare_alias).map(|parts| parts.2),
        key_arn_parts(&key_of("no key at all: the default EFS key")).map(|parts| parts.2),
        "two distinct aliases resolve to distinct keys, got {bare_alias:?}"
    );

    // And the resolution is stable: a second file system under the same alias reports the
    // same key.
    let again = efs
        .create_file_system(json!({
            "CreationToken": "key-again",
            "Encrypted": true,
            "KmsKeyId": "alias/projectKey1",
        }))
        .await;
    assert_eq!(
        again.string("KmsKeyId"),
        bare_alias,
        "the same alias resolves to the same key on every call, got {:?}",
        again.get("KmsKeyId")
    );
}

// ---------------------------------------------------------------------------
// Rule 3 — the deprecated and the current tag paths are one stored tag set.
// ---------------------------------------------------------------------------

/// One row of the tag-write table: what `CreateTags` writes, and what the file system is
/// left holding.
struct TagWriteCase {
    case: &'static str,
    created_with: Vec<(String, String)>,
    written: Vec<(String, String)>,
    expected: Vec<(String, String)>,
}

/// The tag writes the rule is asserted over. The key and value forms come from the
/// modelled patterns (`smithy_query(query="shape",shape="Tag")`): `TagKey` is 1..128 of
/// `[\p{L}\p{Z}\p{N}_.:/=+\-@]` and `TagValue` the same class at 0..256, so an empty
/// value and a non-ASCII letter are both in the class; the largest row is the published
/// 50-tag quota.
fn tag_write_table() -> Vec<TagWriteCase> {
    let owned = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    };
    vec![
        TagWriteCase {
            case: "one tag onto an untagged file system",
            created_with: Vec::new(),
            written: owned(&[("team", "storage")]),
            expected: owned(&[("team", "storage")]),
        },
        TagWriteCase {
            case: "several tags beside the ones the create stored",
            created_with: owned(&[("Name", "probe"), ("env", "prod")]),
            written: owned(&[("team", "storage"), ("cost:center/eu", "team=platform+core")]),
            expected: owned(&[
                ("Name", "probe"),
                ("env", "prod"),
                ("team", "storage"),
                ("cost:center/eu", "team=platform+core"),
            ]),
        },
        TagWriteCase {
            case: "a key the file system already holds is overwritten, not duplicated",
            created_with: owned(&[("env", "dev"), ("team", "platform")]),
            written: owned(&[("env", "prod"), ("Équipe.de_test", "valeur naïve")]),
            expected: owned(&[
                ("env", "prod"),
                ("team", "platform"),
                ("Équipe.de_test", "valeur naïve"),
            ]),
        },
        TagWriteCase {
            case: "the Name tag, which the description reports as Name",
            created_with: Vec::new(),
            written: owned(&[("Name", "renamed-by-create-tags"), ("empty-value", "")]),
            expected: owned(&[("Name", "renamed-by-create-tags"), ("empty-value", "")]),
        },
        TagWriteCase {
            case: "up to the published 50-tag quota",
            created_with: numbered_tags(20),
            written: numbered_tags(50),
            expected: numbered_tags(50),
        },
    ]
}

#[tokio::test]
async fn create_tags_writes_the_one_stored_tag_set_every_read_path_reports() {
    let efs = Emulator::new();
    for (index, row) in tag_write_table().into_iter().enumerate() {
        let TagWriteCase {
            case,
            created_with,
            written,
            expected,
        } = row;
        let id = settled_file_system(
            &efs,
            json!({
                "CreationToken": format!("deprecated-tags-{index}"),
                "Tags": tags_json(&created_with),
            }),
        )
        .await;

        let wrote = create_tags(&efs, &id, &written).await;
        assert_eq!(
            wrote.status, 204,
            "case {case:?}: CreateTags expected HTTP 204, got {} {:?}",
            wrote.status, wrote.body
        );
        assert_eq!(
            wrote.body_len, 0,
            "case {case:?}: a 204 carries no body, got {} bytes",
            wrote.body_len
        );

        // Both tag reads and the description's own `Tags` report the ONE stored set.
        let description = stored(&efs, &id).await;
        let observed: Vec<(&str, Vec<(String, String)>)> = vec![
            ("DescribeTags", reported(&describe_tags(&efs, &id).await)),
            ("ListTagsForResource", reported(&list_tags(&efs, &id).await)),
            ("DescribeFileSystems.Tags", description_tags(&description)),
        ];
        for (path, tags) in observed {
            assert_eq!(
                as_set(&tags),
                as_set(&expected),
                "case {case:?} on {path}: expected the written tag set, got {tags:?}"
            );
        }
        // "If you add the Name tag to your file system, Amazon EFS returns it in the
        // response to the DescribeFileSystems operation."
        let name = expected
            .iter()
            .find(|(key, _)| key == "Name")
            .map(|(_, value)| value.clone());
        assert_eq!(
            description["Name"].as_str().map(str::to_string),
            name,
            "case {case:?}: Name comes from the Name tag of the stored set, got \
             {description:?}"
        );

        // And the deprecated removal path takes keys out of that same set, ignoring a key
        // the file system does not hold.
        let removed: Vec<&str> = expected
            .iter()
            .map(|(key, _)| key.as_str())
            .take(1)
            .chain(["no-such-key"])
            .collect();
        let deleted = delete_tags(&efs, &id, &removed).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: DeleteTags expected HTTP 204, got {} {:?}",
            deleted.status, deleted.body
        );
        let left: Vec<(String, String)> = expected
            .iter()
            .filter(|(key, _)| !removed.contains(&key.as_str()))
            .cloned()
            .collect();
        for (path, tags) in [
            ("DescribeTags", reported(&describe_tags(&efs, &id).await)),
            ("ListTagsForResource", reported(&list_tags(&efs, &id).await)),
        ] {
            assert_eq!(
                as_set(&tags),
                as_set(&left),
                "case {case:?} on {path}: expected the set the removal left, got {tags:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_deprecated_and_the_current_tag_paths_write_the_same_set() {
    // `CreateTags` is modelled `@deprecated` with "Use TagResource." and `DescribeTags`
    // with "Use ListTagsForResource.", so each pair is two views of ONE tag set: a write
    // through either path is visible through both reads, whichever form of the id either
    // call addressed the file system by.
    let efs = Emulator::new();
    let id = settled_file_system(&efs, json!({ "CreationToken": "both-paths" })).await;
    let arn = stored(&efs, &id).await["FileSystemArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, the id form the write addresses, the writing path).
    let writes: Vec<(&str, String, &str, (&str, &str))> = vec![
        (
            "CreateTags by bare id",
            id.clone(),
            "create-tags",
            ("team", "storage"),
        ),
        (
            "TagResource by bare id",
            id.clone(),
            "resource-tags",
            ("env", "prod"),
        ),
        (
            "CreateTags by ARN",
            as_label(&arn),
            "create-tags",
            ("owner", "data-platform"),
        ),
        (
            "TagResource by ARN",
            as_label(&arn),
            "resource-tags",
            ("Name", "shared"),
        ),
    ];

    let mut expected: Vec<(String, String)> = Vec::new();
    for (case, addressed_by, path, (key, value)) in writes {
        let tag = vec![(key.to_string(), value.to_string())];
        let reply = match path {
            "create-tags" => create_tags(&efs, &addressed_by, &tag).await,
            _ => tag_resource(&efs, &addressed_by, &tag).await,
        };
        let expected_status = if path == "create-tags" { 204 } else { 200 };
        assert_eq!(
            reply.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} {:?}",
            reply.status, reply.body
        );
        expected.push((key.to_string(), value.to_string()));

        // Both reads, addressed by both id forms, report every write so far.
        for (reader, tags) in [
            ("DescribeTags by id", reported(&describe_tags(&efs, &id).await)),
            (
                "DescribeTags by ARN",
                reported(&describe_tags(&efs, &as_label(&arn)).await),
            ),
            ("ListTagsForResource by id", reported(&list_tags(&efs, &id).await)),
            (
                "ListTagsForResource by ARN",
                reported(&list_tags(&efs, &as_label(&arn)).await),
            ),
        ] {
            assert_eq!(
                as_set(&tags),
                as_set(&expected),
                "case {case:?} on {reader}: expected every write so far, got {tags:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_refused_deprecated_tag_call_is_the_modelled_error_and_stores_nothing() {
    let efs = Emulator::new();
    let held = numbered_tags(state::MAX_TAGS_PER_RESOURCE);
    let id = settled_file_system(
        &efs,
        json!({ "CreationToken": "deprecated-refusals", "Tags": tags_json(&held) }),
    )
    .await;

    // (case, the file system addressed, the tag keys written, expected status, expected
    //  error type). Every key is written with one value, so the row varies only along what
    //  makes the write refusable.
    let refused: Vec<(&str, String, Vec<&str>, u16, &str)> = vec![
        (
            "a file system that does not exist",
            "fs-deadbeef".to_string(),
            vec!["team"],
            404,
            "FileSystemNotFound",
        ),
        (
            "a request repeating one key",
            id.clone(),
            vec!["dup", "dup"],
            400,
            "BadRequest",
        ),
        (
            "one key past the published quota",
            id.clone(),
            vec!["one-too-many"],
            400,
            "BadRequest",
        ),
    ];
    for (case, addressed_by, keys, status, error_type) in refused {
        let tags: Vec<(String, String)> = keys
            .iter()
            .enumerate()
            .map(|(index, key)| ((*key).to_string(), format!("value-{index}")))
            .collect();
        let reply = create_tags(&efs, &addressed_by, &tags).await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some(error_type),
            "case {case:?}: unexpected error type, got {:?}",
            reply.error_type
        );
    }

    // A removal on a file system that does not exist is the same 404, and the modelled
    // `TagKeys` bound (len 1..50) is refused before the handler runs.
    let missing = delete_tags(&efs, "fs-deadbeef", &["team"]).await;
    assert_eq!(
        missing.status, 404,
        "DeleteTags on an unknown file system: expected 404, got {} {:?}",
        missing.status, missing.body
    );
    assert_eq!(
        missing.error_type.as_deref(),
        Some("FileSystemNotFound"),
        "DeleteTags on an unknown file system: unexpected error type, got {:?}",
        missing.error_type
    );
    let empty = delete_tags(&efs, &id, &[]).await;
    assert_eq!(
        empty.status, 400,
        "DeleteTags with no key: expected 400, got {} {:?}",
        empty.status, empty.body
    );

    // Nothing any refusal asked for reached the store.
    assert_eq!(
        as_set(&reported(&describe_tags(&efs, &id).await)),
        as_set(&held),
        "a refused tag call must leave the stored set standing"
    );
}
