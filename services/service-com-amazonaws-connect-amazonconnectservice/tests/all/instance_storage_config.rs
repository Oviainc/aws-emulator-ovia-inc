// SPDX-License-Identifier: CC0-1.0

//! Instance storage configurations — `AssociateInstanceStorageConfig` and the
//! `DescribeInstanceStorageConfig` read that proves it — end to end over HTTP through
//! the router the emulator serves with.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * an association STORES the configuration it was given under a freshly minted
//!   association id, and `DescribeInstanceStorageConfig` reads that same
//!   configuration back carrying that id — for every `StorageType` the model names
//!   and a spread of the resource types it can store;
//! * an instance holds at most ONE storage configuration per resource type, so a
//!   second association of a resource type is refused with the modelled
//!   `ResourceConflictException` (HTTP 409);
//! * a repeat of an association that carried a `ClientToken` reports the association
//!   that token already made, while the same token carrying a DIFFERENT configuration
//!   is not that repeat and is refused like any other second association;
//! * a configuration that does not carry the member its `StorageType` names is
//!   refused with the modelled `InvalidParameterException` (HTTP 400);
//! * an instance, or an association, that does not exist is refused with the modelled
//!   `ResourceNotFoundException` (HTTP 404) — never a 501.

use serde_json::{json, Value};
use service_com_amazonaws_connect_amazonconnectservice::ops::types::{
    INSTANCE_STORAGE_RESOURCE_TYPE_VALUES, STORAGE_TYPE_VALUES,
};
use service_com_amazonaws_connect_amazonconnectservice::state::STORAGE_TYPE_CONFIG_MEMBERS;

use super::harness::{connect_managed, looks_like_a_uuid, Answer, Connect};

/// One row of the table: a resource type to associate storage for, the storage type
/// that stores it, and whether this row's S3 configuration carries the OPTIONAL
/// `EncryptionConfig` — so the table spans both forms of it.
///
/// Every resource type appears at most once, so the whole table associates onto one
/// instance. The names come from `com.amazonaws.connect#InstanceStorageResourceType`
/// and `com.amazonaws.connect#StorageType`, which
/// [`every_case_names_a_modelled_resource_type_and_storage_type`] checks this table
/// against.
#[derive(Debug, Clone, Copy)]
struct Case {
    resource_type: &'static str,
    storage_type: &'static str,
    encrypted: bool,
}

/// The table every success case below is driven over: all four storage types, and a
/// spread of the resource types each of them stores.
const CASES: &[Case] = &[
    Case {
        resource_type: "CHAT_TRANSCRIPTS",
        storage_type: "S3",
        encrypted: true,
    },
    Case {
        resource_type: "ATTACHMENTS",
        storage_type: "S3",
        encrypted: false,
    },
    Case {
        resource_type: "SCREEN_RECORDINGS",
        storage_type: "S3",
        encrypted: true,
    },
    Case {
        resource_type: "EMAIL_MESSAGES",
        storage_type: "S3",
        encrypted: false,
    },
    Case {
        resource_type: "MEDIA_STREAMS",
        storage_type: "KINESIS_VIDEO_STREAM",
        encrypted: true,
    },
    Case {
        resource_type: "AGENT_EVENTS",
        storage_type: "KINESIS_STREAM",
        encrypted: false,
    },
    Case {
        resource_type: "REAL_TIME_CONTACT_ANALYSIS_CHAT_SEGMENTS",
        storage_type: "KINESIS_STREAM",
        encrypted: false,
    },
    Case {
        resource_type: "CONTACT_TRACE_RECORDS",
        storage_type: "KINESIS_FIREHOSE",
        encrypted: false,
    },
];

/// `AssociateInstanceStorageConfig`: `PUT /instance/{InstanceId}/storage-config`, the
/// resource type and the configuration in the body.
async fn associate(connect: &Connect, instance_id: &str, body: Value) -> Answer {
    connect
        .send_answer(
            "PUT",
            &format!("/instance/{instance_id}/storage-config"),
            Some(body),
        )
        .await
}

/// `DescribeInstanceStorageConfig`:
/// `GET /instance/{InstanceId}/storage-config/{AssociationId}?resourceType=…`.
async fn describe(
    connect: &Connect,
    instance_id: &str,
    association_id: &str,
    resource_type: &str,
) -> Answer {
    connect
        .send_answer(
            "GET",
            &format!(
                "/instance/{instance_id}/storage-config/{association_id}?resourceType={resource_type}"
            ),
            None,
        )
        .await
}

/// The `InstanceStorageConfig` of one row: its `StorageType` and the one member that
/// storage type names, with values derived from the row's resource type so no two
/// rows send the same configuration.
fn storage_config(case: Case, suffix: &str) -> Value {
    let slug = case.resource_type.to_lowercase().replace('_', "-");
    let encryption = json!({
        "EncryptionType": "KMS",
        "KeyId": format!("arn:aws:kms:us-east-1:000000000000:key/{slug}"),
    });
    match case.storage_type {
        "S3" => {
            let mut s3 = json!({
                "BucketName": format!("connect-{slug}-{suffix}"),
                "BucketPrefix": format!("{slug}/{suffix}"),
            });
            if case.encrypted {
                s3["EncryptionConfig"] = encryption;
            }
            json!({ "StorageType": "S3", "S3Config": s3 })
        }
        "KINESIS_VIDEO_STREAM" => json!({
            "StorageType": "KINESIS_VIDEO_STREAM",
            "KinesisVideoStreamConfig": {
                "Prefix": format!("{slug}-{suffix}"),
                "RetentionPeriodHours": 48,
                "EncryptionConfig": encryption,
            },
        }),
        "KINESIS_STREAM" => json!({
            "StorageType": "KINESIS_STREAM",
            "KinesisStreamConfig": {
                "StreamArn": format!("arn:aws:kinesis:us-east-1:000000000000:stream/{slug}-{suffix}"),
            },
        }),
        "KINESIS_FIREHOSE" => json!({
            "StorageType": "KINESIS_FIREHOSE",
            "KinesisFirehoseConfig": {
                "FirehoseArn":
                    format!("arn:aws:firehose:us-east-1:000000000000:deliverystream/{slug}-{suffix}"),
            },
        }),
        other => panic!("case {case:?}: the table names an unhandled storage type {other}"),
    }
}

/// The request body one row associates with.
fn request(case: Case, suffix: &str, client_token: Option<&str>) -> Value {
    let mut body = json!({
        "ResourceType": case.resource_type,
        "StorageConfig": storage_config(case, suffix),
    });
    if let Some(token) = client_token {
        body["ClientToken"] = json!(token);
    }
    body
}

/// The table is written by hand, so it is pinned to the model's own value lists: a
/// row naming anything else is a stale table, not a failing emulator.
#[test]
fn every_case_names_a_modelled_resource_type_and_storage_type() {
    for case in CASES {
        assert!(
            INSTANCE_STORAGE_RESOURCE_TYPE_VALUES.contains(&case.resource_type),
            "case {case:?}: InstanceStorageResourceType does not name {}",
            case.resource_type
        );
        assert!(
            STORAGE_TYPE_VALUES.contains(&case.storage_type),
            "case {case:?}: StorageType does not name {}",
            case.storage_type
        );
    }
    for (storage_type, _, _) in STORAGE_TYPE_CONFIG_MEMBERS {
        assert!(
            CASES.iter().any(|case| case.storage_type == storage_type),
            "the table leaves the storage type {storage_type} unexercised"
        );
    }
}

/// The association stores what it was given: every row round-trips through
/// `DescribeInstanceStorageConfig` unchanged but for the association id the
/// association minted, and no two rows share an id.
#[tokio::test]
async fn associate_then_describe_round_trips_every_storage_type() {
    let connect = Connect::new();
    let (instance_id, _) = connect
        .create_instance(connect_managed("storage-round-trip"))
        .await;

    let mut seen: Vec<String> = Vec::new();
    for case in CASES {
        let sent = storage_config(*case, "first");
        let answer = associate(&connect, &instance_id, request(*case, "first", None)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status, answer.body
        );
        let association_id = answer.body["AssociationId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected an AssociationId, got {}",
                    answer.body
                )
            })
            .to_string();
        assert!(
            looks_like_a_uuid(&association_id),
            "case {case:?}: expected a UUID-shaped AssociationId, got {association_id:?}"
        );
        assert!(
            !seen.contains(&association_id),
            "case {case:?}: expected a fresh AssociationId, got the already-minted \
             {association_id:?}"
        );
        seen.push(association_id.clone());

        let read = describe(&connect, &instance_id, &association_id, case.resource_type).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the association to be readable with HTTP 200, got {} {}",
            read.status, read.body
        );
        let mut expected = sent;
        expected["AssociationId"] = json!(association_id);
        assert_eq!(
            read.body["StorageConfig"], expected,
            "case {case:?}: expected the stored config {expected}, got {}",
            read.body["StorageConfig"]
        );
    }
}

/// One configuration per resource type: the second association of a resource type is
/// the modelled conflict, and it leaves the first one readable.
#[tokio::test]
async fn a_second_association_of_the_same_resource_type_conflicts() {
    let connect = Connect::new();
    let (instance_id, _) = connect
        .create_instance(connect_managed("storage-conflict"))
        .await;

    for case in CASES {
        let first = associate(&connect, &instance_id, request(*case, "first", None)).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected the first association to answer HTTP 200, got {} {}",
            first.status, first.body
        );
        let association_id = first.body["AssociationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an AssociationId, got {}", first.body))
            .to_string();

        let second = associate(&connect, &instance_id, request(*case, "second", None)).await;
        assert_eq!(
            second.status, 409,
            "case {case:?}: expected HTTP 409 for a second association, got {} {}",
            second.status, second.body
        );
        assert!(
            second.error_code.contains("ResourceConflictException"),
            "case {case:?}: expected ResourceConflictException, got {:?} {}",
            second.error_code,
            second.body
        );

        let read = describe(&connect, &instance_id, &association_id, case.resource_type).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the refused second association to leave the first \
             readable, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.body["StorageConfig"]["AssociationId"],
            json!(association_id),
            "case {case:?}: expected the first association to be unchanged, got {}",
            read.body["StorageConfig"]
        );
    }
}

/// A `ClientToken` makes the association idempotent: repeating it reports the
/// association that token already made, while the same resource type associated
/// WITHOUT that token — or with that token and a different configuration — is the
/// conflict above.
#[tokio::test]
async fn a_repeated_client_token_reports_the_association_it_already_made() {
    let connect = Connect::new();
    let (instance_id, _) = connect
        .create_instance(connect_managed("storage-idempotency"))
        .await;

    for case in CASES {
        let token = format!("token-{}", case.resource_type);
        let first = associate(
            &connect,
            &instance_id,
            request(*case, "first", Some(&token)),
        )
        .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            first.status, first.body
        );
        let repeat = associate(
            &connect,
            &instance_id,
            request(*case, "first", Some(&token)),
        )
        .await;
        assert_eq!(
            repeat.status, 200,
            "case {case:?}: expected the repeat to answer HTTP 200, got {} {}",
            repeat.status, repeat.body
        );
        assert_eq!(
            repeat.body["AssociationId"], first.body["AssociationId"],
            "case {case:?}: expected the repeat to report {}, got {}",
            first.body["AssociationId"], repeat.body["AssociationId"]
        );

        let other_token = associate(
            &connect,
            &instance_id,
            request(*case, "second", Some("a-different-token")),
        )
        .await;
        assert_eq!(
            other_token.status, 409,
            "case {case:?}: expected a different token to find the resource type taken \
             with HTTP 409, got {} {}",
            other_token.status, other_token.body
        );

        // The SAME token with a DIFFERENT configuration is not a retry of the
        // association it made, so it must not be answered with that association.
        let changed = associate(
            &connect,
            &instance_id,
            request(*case, "changed", Some(&token)),
        )
        .await;
        assert_eq!(
            changed.status, 409,
            "case {case:?}: expected the token reused for a different configuration to \
             answer HTTP 409, got {} {}",
            changed.status, changed.body
        );
        assert!(
            changed.error_code.contains("ResourceConflictException"),
            "case {case:?}: expected ResourceConflictException, got {:?} {}",
            changed.error_code,
            changed.body
        );

        // And the association that token did make still reads back as it was stored.
        let association_id = first.body["AssociationId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an AssociationId, got {}", first.body))
            .to_string();
        let read = describe(&connect, &instance_id, &association_id, case.resource_type).await;
        let mut expected = storage_config(*case, "first");
        expected["AssociationId"] = json!(association_id);
        assert_eq!(
            read.body["StorageConfig"], expected,
            "case {case:?}: expected the stored config {expected}, got {}",
            read.body["StorageConfig"]
        );
    }
}

/// A configuration has to carry the member its `StorageType` names: one that carries
/// none of them, and one that carries another storage type's member, are both refused
/// with the modelled bad-parameter error.
#[tokio::test]
async fn a_configuration_without_its_storage_type_member_is_refused() {
    let connect = Connect::new();
    let (instance_id, _) = connect
        .create_instance(connect_managed("storage-bad-config"))
        .await;

    for (index, (storage_type, member, _)) in STORAGE_TYPE_CONFIG_MEMBERS.iter().enumerate() {
        // The member of the NEXT storage type in the list: present, but not the one
        // this configuration's storage type names.
        let decoy = CASES
            .iter()
            .copied()
            .find(|case| {
                case.storage_type == STORAGE_TYPE_CONFIG_MEMBERS
                    [(index + 1) % STORAGE_TYPE_CONFIG_MEMBERS.len()]
                    .0
            })
            .expect("every storage type has a row in the table");
        let decoy_config = storage_config(decoy, "decoy");

        let mut wrong_member = json!({ "StorageType": storage_type });
        for (key, value) in decoy_config.as_object().expect("an object") {
            if key != "StorageType" {
                wrong_member[key] = value.clone();
            }
        }

        for (label, config) in [
            ("no configuration member", json!({ "StorageType": storage_type })),
            ("another storage type's member", wrong_member),
        ] {
            let answer = associate(
                &connect,
                &instance_id,
                json!({ "ResourceType": "CALL_RECORDINGS", "StorageConfig": config }),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {storage_type} with {label} (needs {member}): expected HTTP 400, got {} {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code.contains("InvalidParameterException"),
                "case {storage_type} with {label}: expected InvalidParameterException, got {:?} {}",
                answer.error_code,
                answer.body
            );
        }
    }
}

/// Nothing is found that was never stored: an unknown instance, an unknown
/// association, and an association read under another resource type are all the
/// modelled not-found error.
#[tokio::test]
async fn an_unknown_instance_or_association_is_not_found() {
    let connect = Connect::new();
    let (instance_id, _) = connect
        .create_instance(connect_managed("storage-not-found"))
        .await;
    let case = CASES[0];
    let unknown = "11111111-2222-3333-4444-555555555555";

    let associated = associate(&connect, &instance_id, request(case, "first", None)).await;
    let association_id = associated.body["AssociationId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an AssociationId, got {}", associated.body))
        .to_string();

    let other_resource_type = CASES[1].resource_type;
    let refusals = [
        (
            "an unknown instance associates nothing",
            associate(&connect, unknown, request(case, "first", None)).await,
        ),
        (
            "an unknown instance describes nothing",
            describe(&connect, unknown, &association_id, case.resource_type).await,
        ),
        (
            "an unknown association describes nothing",
            describe(&connect, &instance_id, unknown, case.resource_type).await,
        ),
        (
            "an association belongs to one resource type",
            describe(&connect, &instance_id, &association_id, other_resource_type).await,
        ),
    ];

    for (label, answer) in refusals {
        assert_eq!(
            answer.status, 404,
            "case {label:?}: expected HTTP 404, got {} {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_code.contains("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException, got {:?} {}",
            answer.error_code,
            answer.body
        );
    }
}
