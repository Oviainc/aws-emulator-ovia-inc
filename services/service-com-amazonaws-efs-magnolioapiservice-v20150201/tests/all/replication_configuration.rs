// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EFS replication slice: CreateReplicationConfiguration,
//! DescribeReplicationConfigurations, DeleteReplicationConfiguration and the
//! UpdateFileSystemProtection call that decides which file systems may be a
//! destination — all driven over HTTP through the same `MultiServiceRouter` the
//! production entrypoint serves.
//!
//! Every rule is asserted over a SET of inputs rather than on one recorded call: the
//! tables below vary what each rule generalises over (the destination form, the
//! Region, the Availability Zone, the KMS key, the destination count, the page size,
//! the protection status).

use serde_json::{json, Value};
use service_com_amazonaws_efs_magnolioapiservice_v20150201::state;

use super::harness::{matches_id_pattern, settle, Emulator};

/// The canonical emulator account every ARN and `OwnerId` carries.
const ACCOUNT: &str = "000000000000";

/// The one destination of a configuration, as the reply reports it.
fn destination(reply: &Value) -> Value {
    reply["Destinations"][0].clone()
}

/// The stored description of one file system, read back through DescribeFileSystems.
async fn read_file_system(efs: &Emulator, id: &str) -> Value {
    let reply = efs
        .describe_file_systems(&format!("FileSystemId={id}"))
        .await;
    assert_eq!(
        reply.status, 200,
        "DescribeFileSystems({id}): expected 200, got {} {:?}",
        reply.status, reply.body
    );
    reply.get("FileSystems")[0].clone()
}

// ---------------------------------------------------------------------------
// The whole lifecycle, in order, with every read observing the preceding write.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn replication_configuration_lifecycle_round_trips_through_state() {
    let efs = Emulator::new();
    let source_id = efs.available_file_system("replication-source").await;

    // CreateReplicationConfiguration with no destination FileSystemId: "Amazon EFS
    // creates a new, empty file system."
    let created = efs
        .create_replication_configuration(&source_id, json!({ "Destinations": [{}] }))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateReplicationConfiguration: expected 200, got {} {:?}",
        created.status, created.body
    );
    assert_eq!(
        created.string("SourceFileSystemId"),
        source_id,
        "CreateReplicationConfiguration: unexpected SourceFileSystemId, got {:?}",
        created.get("SourceFileSystemId")
    );
    assert_eq!(
        created.string("SourceFileSystemRegion"),
        "us-east-1",
        "CreateReplicationConfiguration: the source Region is the one it was signed for, got {:?}",
        created.get("SourceFileSystemRegion")
    );
    let source_arn =
        format!("arn:aws:elasticfilesystem:us-east-1:{ACCOUNT}:file-system/{source_id}");
    assert_eq!(
        created.string("SourceFileSystemArn"),
        source_arn,
        "CreateReplicationConfiguration: unexpected SourceFileSystemArn, got {:?}",
        created.get("SourceFileSystemArn")
    );
    assert_eq!(
        created.string("OriginalSourceFileSystemArn"),
        source_arn,
        "CreateReplicationConfiguration: a fresh configuration's original source is its \
         source, got {:?}",
        created.get("OriginalSourceFileSystemArn")
    );
    assert_eq!(
        created.string("SourceFileSystemOwnerId"),
        ACCOUNT,
        "CreateReplicationConfiguration: unexpected SourceFileSystemOwnerId, got {:?}",
        created.get("SourceFileSystemOwnerId")
    );
    assert!(
        created.get("CreationTime").is_number(),
        "CreateReplicationConfiguration: CreationTime must be reported, got {:?}",
        created.get("CreationTime")
    );

    let new_destination = destination(&created.body);
    let destination_id = new_destination["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        matches_id_pattern(&destination_id, "fs-") && destination_id != source_id,
        "CreateReplicationConfiguration: expected a freshly minted destination file system, \
         got {new_destination:?}"
    );
    assert_eq!(
        new_destination["Status"], "ENABLING",
        "CreateReplicationConfiguration: a new destination is reported while replication is \
         still being enabled, got {new_destination:?}"
    );
    assert_eq!(
        new_destination["Region"], "us-east-1",
        "CreateReplicationConfiguration: the destination defaults to the source's Region, \
         got {new_destination:?}"
    );
    assert_eq!(
        new_destination["OwnerId"], ACCOUNT,
        "CreateReplicationConfiguration: unexpected destination OwnerId, got {new_destination:?}"
    );
    assert_eq!(
        new_destination["LastReplicatedTimestamp"],
        Value::Null,
        "CreateReplicationConfiguration: nothing has been replicated yet, got {new_destination:?}"
    );

    // The destination file system is in the store, encrypted, still creating, and
    // read-only while it is replicated into.
    let destination_file_system = read_file_system(&efs, &destination_id).await;
    assert_eq!(
        destination_file_system["LifeCycleState"], "creating",
        "the destination file system is reported while still creating, got \
         {destination_file_system:?}"
    );
    assert_eq!(
        destination_file_system["Encrypted"], json!(true),
        "a destination file system is encrypted, got {destination_file_system:?}"
    );
    // "If you don't specify a KMS key, then the default KMS key for Amazon EFS,
    // /aws/elasticfilesystem, is used" — and the description carries the KEY that alias
    // resolves to, not the alias (see `mutation_results.rs`), so this asserts the form
    // rather than a literal: a key ARN in the destination's Region.
    let reported_key = destination_file_system["KmsKeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        reported_key.starts_with(&format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/")),
        "a destination file system with no KmsKeyId uses the default EFS key, resolved to \
         a key ARN, got {reported_key:?}"
    );
    assert_eq!(
        destination_file_system["FileSystemProtection"]["ReplicationOverwriteProtection"],
        state::PROTECTION_REPLICATING,
        "the destination of a replication configuration is REPLICATING, got \
         {destination_file_system:?}"
    );

    // DescribeReplicationConfigurations reads the stored record back, by source id, by
    // destination id and unfiltered.
    for selector in ["", &format!("FileSystemId={source_id}"), &format!("FileSystemId={destination_id}")] {
        let listed = efs.describe_replication_configurations(selector).await;
        assert_eq!(
            listed.status, 200,
            "DescribeReplicationConfigurations({selector:?}): expected 200, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.get("Replications"),
            &json!([created.body.clone()]),
            "DescribeReplicationConfigurations({selector:?}): must return the stored \
             description, got {:?}",
            listed.get("Replications")
        );
        assert_eq!(
            listed.get("NextToken"),
            &Value::Null,
            "DescribeReplicationConfigurations({selector:?}): one page holds every \
             configuration, got {:?}",
            listed.get("NextToken")
        );
    }

    // The clock owes the destination `ENABLED` and the destination file system
    // `available`; neither handler sleeps for it.
    settle().await;
    let settled = efs
        .describe_replication_configurations(&format!("FileSystemId={source_id}"))
        .await;
    let settled_destination = destination(&settled.get("Replications")[0]);
    assert_eq!(
        settled_destination["Status"], "ENABLED",
        "once the enabling span has elapsed the destination is ENABLED, got \
         {settled_destination:?}"
    );
    assert_eq!(
        read_file_system(&efs, &destination_id).await["LifeCycleState"],
        "available",
        "once the creating span has elapsed the destination file system is available"
    );

    // A file system can be part of only one replication configuration.
    let second = efs
        .create_replication_configuration(&source_id, json!({ "Destinations": [{}] }))
        .await;
    assert_eq!(
        second.status, 409,
        "a second configuration for the same source: expected 409, got {} {:?}",
        second.status, second.body
    );
    assert_eq!(
        second.error_type.as_deref(),
        Some("ConflictException"),
        "a second configuration for the same source: unexpected error type, got {:?}",
        second.error_type
    );

    // DeleteReplicationConfiguration removes the record and re-enables the
    // destination's replication overwrite protection.
    let deleted = efs.delete_replication_configuration(&source_id, "").await;
    assert_eq!(
        deleted.status, 204,
        "DeleteReplicationConfiguration: expected 204, got {} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.body_len, 0,
        "DeleteReplicationConfiguration: a 204 carries no body, got {} bytes",
        deleted.body_len
    );
    assert_eq!(
        read_file_system(&efs, &destination_id).await["FileSystemProtection"]
            ["ReplicationOverwriteProtection"],
        state::PROTECTION_ENABLED,
        "after the configuration is deleted the destination file system is writeable again"
    );
    let gone = efs
        .describe_replication_configurations(&format!("FileSystemId={source_id}"))
        .await;
    assert_eq!(
        gone.status, 404,
        "DescribeReplicationConfigurations after the delete: expected 404, got {} {:?}",
        gone.status, gone.body
    );
    assert_eq!(
        gone.error_type.as_deref(),
        Some("ReplicationNotFound"),
        "DescribeReplicationConfigurations after the delete: unexpected error type, got {:?}",
        gone.error_type
    );
    assert_eq!(
        gone.string("Message"),
        state::NO_REPLICATIONS_FOUND_MESSAGE,
        "DescribeReplicationConfigurations after the delete: unexpected Message construction, \
         got {:?}",
        gone.get("Message")
    );
    let empty = efs.describe_replication_configurations("").await;
    assert_eq!(
        empty.get("Replications"),
        &json!([]),
        "no configuration is left in the store, got {:?}",
        empty.get("Replications")
    );
    let deleted_twice = efs.delete_replication_configuration(&source_id, "").await;
    assert_eq!(
        deleted_twice.status, 404,
        "DeleteReplicationConfiguration twice: expected 404, got {} {:?}",
        deleted_twice.status, deleted_twice.body
    );
    assert_eq!(
        deleted_twice.string("Message"),
        state::replication_configuration_not_found_message(&source_id),
        "DeleteReplicationConfiguration twice: unexpected Message construction, got {:?}",
        deleted_twice.get("Message")
    );
}

// ---------------------------------------------------------------------------
// Every destination form a request may ask for.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_destination_form_mints_the_file_system_it_asks_for() {
    // The rows differ along the members `DestinationToCreate` carries: the Region, the
    // Availability Zone that makes the destination One Zone storage, the KMS key in
    // each of its accepted forms, and the cross-account role. The expectations are the
    // destination description's members, plus the `Destination` the configuration
    // reports. Published AZ-ID prefixes: us-east-1 -> use1, eu-central-1 -> euc1
    // ("Availability Zone IDs for your resources", AWS RAM User Guide).
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "no destination member at all",
            json!({}),
            json!({ "Region": "us-east-1", "RoleArn": null }),
            // The default EFS key is named by an ALIAS, which the description reports as
            // the key it resolves to; the `key/` prefix below is asserted for every row.
            json!({
                "AvailabilityZoneName": null,
                "PerformanceMode": "generalPurpose",
                "ThroughputMode": "bursting",
            }),
        ),
        (
            "a destination Region other than the source's",
            json!({ "Region": "us-west-2" }),
            json!({ "Region": "us-west-2" }),
            json!({}),
        ),
        (
            "One Zone storage in the source's Region",
            json!({ "AvailabilityZoneName": "us-east-1b" }),
            json!({ "Region": "us-east-1" }),
            json!({ "AvailabilityZoneName": "us-east-1b", "AvailabilityZoneId": "use1-az2" }),
        ),
        (
            "One Zone storage in another Region",
            json!({ "Region": "eu-central-1", "AvailabilityZoneName": "eu-central-1c" }),
            json!({ "Region": "eu-central-1" }),
            json!({ "AvailabilityZoneName": "eu-central-1c", "AvailabilityZoneId": "euc1-az3" }),
        ),
        (
            // An alias is a pointer to one key, so the description reports the key it
            // resolves to; the resolution is asserted by form below and by invariant in
            // `mutation_results.rs`.
            "a KMS key alias",
            json!({ "KmsKeyId": "alias/efs-replica" }),
            json!({ "Region": "us-east-1" }),
            json!({}),
        ),
        (
            "a bare KMS key id",
            json!({ "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab" }),
            json!({ "Region": "us-east-1" }),
            json!({
                "KmsKeyId":
                    format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/1234abcd-12ab-34cd-56ef-1234567890ab"),
            }),
        ),
        (
            "a KMS key ARN",
            json!({
                "KmsKeyId":
                    "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            }),
            json!({ "Region": "us-east-1" }),
            json!({
                "KmsKeyId":
                    "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            }),
        ),
        (
            "a cross-account replication role",
            json!({ "RoleArn": "arn:aws:iam::111122223333:role/efs-replication" }),
            json!({ "RoleArn": "arn:aws:iam::111122223333:role/efs-replication" }),
            json!({}),
        ),
    ];

    let efs = Emulator::new();
    for (index, (case, destination_request, expected_destination, expected_file_system)) in
        cases.into_iter().enumerate()
    {
        // One source per row: "A file system can be part of only one replication
        // configuration."
        let source_id = efs.available_file_system(&format!("row-{index}")).await;
        let created = efs
            .create_replication_configuration(
                &source_id,
                json!({ "Destinations": [destination_request.clone()] }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            created.status, created.body
        );
        let reported = destination(&created.body);
        for (member, value) in expected_destination
            .as_object()
            .expect("an object of expectations")
        {
            assert_eq!(
                &reported[member], value,
                "case {case:?}: expected destination {member} {value}, got {reported:?}"
            );
        }

        let destination_id = reported["FileSystemId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let stored = read_file_system(&efs, &destination_id).await;
        for (member, value) in expected_file_system
            .as_object()
            .expect("an object of expectations")
        {
            assert_eq!(
                &stored[member], value,
                "case {case:?}: expected destination file system {member} {value}, got {stored:?}"
            );
        }
        // Whatever form the request named its key in, the destination file system
        // reports the KEY it resolves to.
        let stored_key = stored["KmsKeyId"].as_str().unwrap_or_default();
        assert!(
            stored_key.contains(":key/") && !stored_key.contains(":alias/"),
            "case {case:?}: KmsKeyId must name a key, got {stored_key:?}"
        );
        assert_eq!(
            stored["FileSystemArn"],
            json!(format!(
                "arn:aws:elasticfilesystem:{}:{ACCOUNT}:file-system/{destination_id}",
                reported["Region"].as_str().unwrap_or_default()
            )),
            "case {case:?}: the destination ARN carries its Region, got {stored:?}"
        );
        // "A destination file system is encrypted", and the description names the KEY it
        // is encrypted under, in the destination's Region — whichever form the request
        // asked for it by (an alias resolves to the key it points at).
        let key = stored["KmsKeyId"].as_str().unwrap_or_default().to_string();
        let key_region = reported["Region"].as_str().unwrap_or_default();
        assert!(
            key.starts_with(&format!("arn:aws:kms:{key_region}:")) && key.contains(":key/"),
            "case {case:?}: expected a KMS key ARN in {key_region}, got {key:?}"
        );

        // And the configuration reads back exactly as it was answered.
        let listed = efs
            .describe_replication_configurations(&format!("FileSystemId={source_id}"))
            .await;
        assert_eq!(
            listed.get("Replications")[0],
            created.body,
            "case {case:?}: DescribeReplicationConfigurations must return the stored \
             description, got {:?}",
            listed.get("Replications")
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_replication_configuration_refuses_each_unsatisfiable_request() {
    let efs = Emulator::new();
    let source_id = efs.available_file_system("refusals").await;
    // One available file system whose replication overwrite protection is still the
    // default ENABLED, and one that has been disabled so it may be a destination.
    let protected_id = efs.available_file_system("protected").await;
    let writable_id = efs.available_file_system("unprotected").await;
    let disabled = efs
        .update_file_system_protection(
            &writable_id,
            json!({ "ReplicationOverwriteProtection": state::PROTECTION_DISABLED }),
        )
        .await;
    assert_eq!(
        disabled.status, 200,
        "UpdateFileSystemProtection(DISABLED): expected 200, got {} {:?}",
        disabled.status, disabled.body
    );

    // Each row is one documented rule, named by the error the model declares for it.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "no destination at all",
            json!({ "Destinations": [] }),
            400,
            "BadRequest",
        ),
        (
            "two destinations",
            json!({ "Destinations": [{}, {}] }),
            400,
            "BadRequest",
        ),
        (
            "the source as its own destination",
            json!({ "Destinations": [{ "FileSystemId": source_id }] }),
            400,
            "BadRequest",
        ),
        (
            "an existing destination that does not exist",
            json!({ "Destinations": [{ "FileSystemId": "fs-deadbeef" }] }),
            404,
            "FileSystemNotFound",
        ),
        (
            "an existing destination whose overwrite protection is still enabled",
            json!({ "Destinations": [{ "FileSystemId": protected_id }] }),
            409,
            "ConflictException",
        ),
        (
            "AvailabilityZoneName beside an existing destination",
            json!({
                "Destinations": [{
                    "FileSystemId": writable_id,
                    "AvailabilityZoneName": "us-east-1a",
                }],
            }),
            400,
            "BadRequest",
        ),
        (
            "KmsKeyId beside an existing destination",
            json!({
                "Destinations": [{ "FileSystemId": writable_id, "KmsKeyId": "alias/other" }],
            }),
            400,
            "BadRequest",
        ),
        (
            "a Region that does not hold the existing destination",
            json!({ "Destinations": [{ "FileSystemId": writable_id, "Region": "eu-west-1" }] }),
            400,
            "BadRequest",
        ),
        (
            "an Availability Zone outside the destination Region",
            json!({
                "Destinations": [{ "Region": "us-west-2", "AvailabilityZoneName": "us-east-1a" }],
            }),
            400,
            "UnsupportedAvailabilityZone",
        ),
        (
            "an Availability Zone name that names no zone",
            json!({ "Destinations": [{ "AvailabilityZoneName": "us-east-1z" }] }),
            400,
            "UnsupportedAvailabilityZone",
        ),
    ];

    for (case, body, status, error_type) in cases {
        let reply = efs.create_replication_configuration(&source_id, body).await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected {status}, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some(error_type),
            "case {case:?}: expected {error_type}, got {:?}",
            reply.error_type
        );
        let listed = efs.describe_replication_configurations("").await;
        assert_eq!(
            listed.get("Replications"),
            &json!([]),
            "case {case:?}: a refused call must store no configuration, got {:?}",
            listed.get("Replications")
        );
    }

    // A refused call also mints no destination file system: only the three created
    // above are in the store.
    let all = efs.describe_file_systems("").await;
    assert_eq!(
        all.get("FileSystems").as_array().map(Vec::len),
        Some(3),
        "a refused call must mint no destination file system, got {:?}",
        all.get("FileSystems")
    );

    // An unknown source file system, and a source that is not yet `available`.
    let unknown = efs
        .create_replication_configuration("fs-deadbeef", json!({ "Destinations": [{}] }))
        .await;
    assert_eq!(
        unknown.status, 404,
        "an unknown source: expected 404, got {} {:?}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_type.as_deref(),
        Some("FileSystemNotFound"),
        "an unknown source: unexpected error type, got {:?}",
        unknown.error_type
    );
    // The one not-found construction this service renders everywhere:
    // `<Noun> '<id>' does not exist.`
    assert_eq!(
        unknown.string("Message"),
        "File system 'fs-deadbeef' does not exist.",
        "an unknown source: unexpected Message construction, got {:?}",
        unknown.get("Message")
    );

    let creating = efs.create_file_system(json!({ "CreationToken": "still-creating" })).await;
    let creating_id = creating.string("FileSystemId");
    let too_early = efs
        .create_replication_configuration(&creating_id, json!({ "Destinations": [{}] }))
        .await;
    assert_eq!(
        too_early.status, 409,
        "a source that is still creating: expected 409, got {} {:?}",
        too_early.status, too_early.body
    );
    assert_eq!(
        too_early.error_type.as_deref(),
        Some("IncorrectFileSystemLifeCycleState"),
        "a source that is still creating: unexpected error type, got {:?}",
        too_early.error_type
    );

    // "Returned if the source file system in a replication is encrypted but the
    // destination file system is unencrypted."
    let encrypted = efs
        .create_file_system(json!({ "CreationToken": "encrypted-source", "Encrypted": true }))
        .await;
    let encrypted_id = encrypted.string("FileSystemId");
    settle().await;
    let mismatched = efs
        .create_replication_configuration(
            &encrypted_id,
            json!({ "Destinations": [{ "FileSystemId": writable_id }] }),
        )
        .await;
    assert_eq!(
        mismatched.status, 409,
        "an encrypted source and an unencrypted destination: expected 409, got {} {:?}",
        mismatched.status, mismatched.body
    );
    assert_eq!(
        mismatched.error_type.as_deref(),
        Some("ConflictException"),
        "an encrypted source and an unencrypted destination: unexpected error type, got {:?}",
        mismatched.error_type
    );
}

// ---------------------------------------------------------------------------
// Replicating to a file system that already exists.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_existing_file_system_becomes_a_destination_once_its_protection_is_disabled() {
    let efs = Emulator::new();
    let source_id = efs.available_file_system("existing-source").await;
    let destination_id = efs.available_file_system("existing-destination").await;

    // "The file system's replication overwrite protection must be disabled."
    let disabled = efs
        .update_file_system_protection(
            &destination_id,
            json!({ "ReplicationOverwriteProtection": state::PROTECTION_DISABLED }),
        )
        .await;
    assert_eq!(
        disabled.string("ReplicationOverwriteProtection"),
        state::PROTECTION_DISABLED,
        "UpdateFileSystemProtection must report what it stored, got {:?}",
        disabled.body
    );

    let created = efs
        .create_replication_configuration(
            &source_id,
            json!({ "Destinations": [{ "FileSystemId": destination_id }] }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "replicating to an existing file system: expected 200, got {} {:?}",
        created.status, created.body
    );
    let reported = destination(&created.body);
    assert_eq!(
        reported["FileSystemId"], json!(destination_id),
        "the destination must be the file system the request named, got {reported:?}"
    );
    assert_eq!(
        reported["Region"], "us-east-1",
        "the destination Region is the Region the named file system is in, got {reported:?}"
    );

    // No file system is minted for an existing destination: the store still holds the
    // two created above.
    let all = efs.describe_file_systems("").await;
    assert_eq!(
        all.get("FileSystems").as_array().map(Vec::len),
        Some(2),
        "an existing destination must not mint a file system, got {:?}",
        all.get("FileSystems")
    );
    assert_eq!(
        read_file_system(&efs, &destination_id).await["FileSystemProtection"]
            ["ReplicationOverwriteProtection"],
        state::PROTECTION_REPLICATING,
        "an existing destination becomes REPLICATING while it is replicated into"
    );

    // While it is part of a configuration, its protection cannot be moved by a caller.
    let refused = efs
        .update_file_system_protection(
            &destination_id,
            json!({ "ReplicationOverwriteProtection": state::PROTECTION_ENABLED }),
        )
        .await;
    assert_eq!(
        refused.status, 409,
        "UpdateFileSystemProtection on a replicated file system: expected 409, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ReplicationAlreadyExists"),
        "UpdateFileSystemProtection on a replicated file system: unexpected error type, got {:?}",
        refused.error_type
    );

    // Deleting the configuration re-enables it, in each documented DeletionMode.
    let deleted = efs
        .delete_replication_configuration(&source_id, "deletionMode=LOCAL_CONFIGURATION_ONLY")
        .await;
    assert_eq!(
        deleted.status, 204,
        "DeleteReplicationConfiguration(LOCAL_CONFIGURATION_ONLY): expected 204, got {} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        read_file_system(&efs, &destination_id).await["FileSystemProtection"]
            ["ReplicationOverwriteProtection"],
        state::PROTECTION_ENABLED,
        "the destination becomes writeable again once the configuration is deleted"
    );
}

// ---------------------------------------------------------------------------
// A file system in a replication configuration cannot be deleted.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn neither_side_of_a_replication_configuration_can_be_deleted_while_it_exists() {
    // "You cannot delete a file system that is part of an EFS replication
    // configuration. You need to delete the replication configuration first" — which
    // holds for the source and for the destination alike.
    let efs = Emulator::new();
    let source_id = efs.available_file_system("undeletable").await;
    let created = efs
        .create_replication_configuration(&source_id, json!({ "Destinations": [{}] }))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateReplicationConfiguration: expected 200, got {} {:?}",
        created.status, created.body
    );
    let destination_id = destination(&created.body)["FileSystemId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    settle().await;

    for (case, id) in [("the source", &source_id), ("the destination", &destination_id)] {
        let refused = efs.delete_file_system(id).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected 409, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("FileSystemInUse"),
            "case {case:?}: expected FileSystemInUse, got {:?}",
            refused.error_type
        );
        assert_eq!(
            read_file_system(&efs, id).await["FileSystemId"],
            json!(id),
            "case {case:?}: a refused delete must leave the file system in the store"
        );
    }

    // Once the configuration is gone both sides can be deleted.
    let deleted = efs.delete_replication_configuration(&source_id, "").await;
    assert_eq!(
        deleted.status, 204,
        "DeleteReplicationConfiguration: expected 204, got {} {:?}",
        deleted.status, deleted.body
    );
    for (case, id) in [("the source", &source_id), ("the destination", &destination_id)] {
        let accepted = efs.delete_file_system(id).await;
        assert_eq!(
            accepted.status, 204,
            "case {case:?} after the configuration is deleted: expected 204, got {} {:?}",
            accepted.status, accepted.body
        );
    }
}

// ---------------------------------------------------------------------------
// UpdateFileSystemProtection, over every value of the modelled enum.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn file_system_protection_accepts_each_caller_settable_status() {
    // The modelled `ReplicationOverwriteProtection` values
    // (`smithy_query(query="shape",shape="ReplicationOverwriteProtection")`): ENABLED
    // and DISABLED are a caller's to set; REPLICATING is EFS's own, set while a
    // replication configuration names the file system as its destination.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "DISABLED",
            json!({ "ReplicationOverwriteProtection": "DISABLED" }),
            200,
            "DISABLED",
        ),
        (
            "ENABLED",
            json!({ "ReplicationOverwriteProtection": "ENABLED" }),
            200,
            "ENABLED",
        ),
        (
            "back to DISABLED",
            json!({ "ReplicationOverwriteProtection": "DISABLED" }),
            200,
            "DISABLED",
        ),
        (
            "no status at all, which changes nothing",
            json!({}),
            200,
            "DISABLED",
        ),
        (
            "REPLICATING, which only EFS sets",
            json!({ "ReplicationOverwriteProtection": "REPLICATING" }),
            400,
            "DISABLED",
        ),
    ];

    let efs = Emulator::new();
    let file_system_id = efs.available_file_system("protection").await;
    assert_eq!(
        read_file_system(&efs, &file_system_id).await["FileSystemProtection"]
            ["ReplicationOverwriteProtection"],
        state::PROTECTION_ENABLED,
        "replication overwrite protection is ENABLED by default"
    );

    for (case, body, status, expected) in cases {
        let reply = efs
            .update_file_system_protection(&file_system_id, body)
            .await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected {status}, got {} {:?}",
            reply.status, reply.body
        );
        if status == 200 {
            assert_eq!(
                reply.string("ReplicationOverwriteProtection"),
                expected,
                "case {case:?}: expected {expected}, got {:?}",
                reply.body
            );
        }
        // Accepted or refused, the store and the description agree on one status.
        assert_eq!(
            read_file_system(&efs, &file_system_id).await["FileSystemProtection"]
                ["ReplicationOverwriteProtection"],
            expected,
            "case {case:?}: the stored status must be {expected}"
        );
    }

    let unknown = efs
        .update_file_system_protection("fs-deadbeef", json!({}))
        .await;
    assert_eq!(
        unknown.status, 404,
        "an unknown file system: expected 404, got {} {:?}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_type.as_deref(),
        Some("FileSystemNotFound"),
        "an unknown file system: unexpected error type, got {:?}",
        unknown.error_type
    );
}

// ---------------------------------------------------------------------------
// Pagination.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn describe_replication_configurations_pages_consistently_from_its_next_token() {
    let efs = Emulator::new();
    let mut sources = Vec::new();
    for index in 0..5 {
        let source_id = efs.available_file_system(&format!("page-{index}")).await;
        let created = efs
            .create_replication_configuration(&source_id, json!({ "Destinations": [{}] }))
            .await;
        assert_eq!(
            created.status, 200,
            "creating configuration {index}: expected 200, got {} {:?}",
            created.status, created.body
        );
        sources.push(source_id);
    }

    // Every page size walks the same five configurations, in creation order, with no
    // repeats and no gaps.
    for page_size in [1_usize, 2, 3, 5, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match &token {
                Some(value) => format!("MaxResults={page_size}&NextToken={value}"),
                None => format!("MaxResults={page_size}"),
            };
            let reply = efs.describe_replication_configurations(&query).await;
            assert_eq!(
                reply.status, 200,
                "case page size {page_size}: expected 200, got {} {:?}",
                reply.status, reply.body
            );
            let page = reply
                .get("Replications")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "case page size {page_size}: expected 1..={page_size} entries, got {}",
                page.len()
            );
            for replication in &page {
                seen.push(
                    replication["SourceFileSystemId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            token = reply
                .get("NextToken")
                .as_str()
                .map(std::string::ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, sources,
            "case page size {page_size}: the pages must walk every configuration in creation \
             order, got {seen:?}"
        );
    }

    // A token that names nothing in the result set is a refusal, not an empty page.
    let stray = efs
        .describe_replication_configurations("NextToken=fs-deadbeef")
        .await;
    assert_eq!(
        stray.status, 400,
        "a stray NextToken: expected 400, got {} {:?}",
        stray.status, stray.body
    );
    assert_eq!(
        stray.error_type.as_deref(),
        Some("BadRequest"),
        "a stray NextToken: unexpected error type, got {:?}",
        stray.error_type
    );

    // A file system that holds no configuration is a 404, and so is an id that names no
    // file system at all: this read reports replications, not file systems.
    let without = efs.available_file_system("no-replication").await;
    let missing = efs
        .describe_replication_configurations(&format!("FileSystemId={without}"))
        .await;
    assert_eq!(
        missing.status, 404,
        "a file system with no configuration: expected 404, got {} {:?}",
        missing.status, missing.body
    );
    assert_eq!(
        missing.error_type.as_deref(),
        Some("ReplicationNotFound"),
        "a file system with no configuration: unexpected error type, got {:?}",
        missing.error_type
    );
    let unknown = efs
        .describe_replication_configurations("FileSystemId=fs-deadbeef")
        .await;
    assert_eq!(
        unknown.error_type.as_deref(),
        Some("ReplicationNotFound"),
        "an unknown file system: unexpected error type, got {:?}",
        unknown.error_type
    );
}
