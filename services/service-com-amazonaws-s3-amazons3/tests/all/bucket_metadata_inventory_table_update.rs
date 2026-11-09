// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateBucketMetadataInventoryTableConfiguration`: enabling and
//! disabling the live inventory table of the S3 Metadata configuration a general purpose
//! bucket carries, read back through `GetBucketMetadataConfiguration`.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns, with the metadata-configuration suite's own
//! helpers for the create, the read and for slicing a response), so what is pinned is the
//! wire behaviour — status, body, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * The operation "enables or disables a live inventory table for an S3 Metadata
//!   configuration on a general purpose bucket" (its own documentation): the
//!   `ConfigurationState` the request carries replaces the one the configuration carried,
//!   whatever state it was in and whether or not it named an inventory table at all, and the
//!   call answers the modelled 200 with an empty body.
//! * A TABLE exists exactly while that state asks for one — which is why
//!   `InventoryTableConfigurationResult` requires its `ConfigurationState` and leaves its
//!   `TableName`, `TableStatus` and `TableArn` optional: an enable creates the table under
//!   the name derived from the general purpose bucket's name, a disable leaves the state
//!   reported with no table behind it, and an enable of a table that is already there leaves
//!   that table exactly as it is rather than creating a second one.
//! * `TableStatus` is "`CREATING` - the journal table is in the process of being created"
//!   until the table falls due to be in service and "`ACTIVE` - the journal table has been
//!   created successfully" from then on (`JournalTableConfigurationResult$TableStatus`), so
//!   a table this call creates is reported as `CREATING` straight afterwards.
//! * The update is scoped to the inventory table: the journal table, its record expiration
//!   and the annotation table are reported exactly as they were before the call.
//! * "If server-side encryption with Key Management Service (KMS) keys (SSE-KMS) is
//!   specified, you must also specify the KMS key Amazon Resource Name (ARN)"
//!   (`MetadataTableEncryptionConfiguration$KmsKeyArn`): a keyless KMS algorithm is refused
//!   with `InvalidArgument` (HTTP 400) and records nothing.
//! * A bucket carrying no metadata configuration has no inventory table to enable or
//!   disable, so the call answers `MetadataConfigurationNotFound` (HTTP 404); a bucket this
//!   emulator does not hold `NoSuchBucket` (HTTP 404), and an `x-amz-expected-bucket-owner`
//!   that is not the account's HTTP 403 `AccessDenied` — each with nothing recorded.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};
use super::bucket_metadata_configuration::{
    create_metadata_configuration, get_metadata_configuration, inside, table_arn, text,
    wait_for_the_tables, ANNOTATION_ROLE, INVENTORY_TABLE, KMS_KEY_ARN, NOT_THE_OWNER, XMLNS,
};

/// `PUT /<bucket>?metadataInventoryTable` — `UpdateBucketMetadataInventoryTableConfiguration`,
/// with `payload` as the `InventoryTableConfiguration` container and whatever headers the
/// case carries.
///
/// `pub(crate)`, like [`updates`] below it: the managed-namespace suite
/// ([`super::bucket_metadata_inventory_table_namespace`]) drives the same subresource through
/// the same harness instead of standing up a second copy of it.
pub(crate) async fn update_inventory_table(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
    payload: &str,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?metadataInventoryTable"),
        headers,
        payload,
    )
    .await
}

/// The `InventoryTableConfigurationUpdates` document naming `state`, plus an encryption
/// configuration naming `algorithm` when the case carries one — with the KMS key ARN beside
/// it exactly when `with_key` asks for it.
pub(crate) fn updates(state: &str, algorithm: Option<&str>, with_key: bool) -> String {
    let mut body = format!("<InventoryTableConfiguration xmlns=\"{XMLNS}\">");
    body.push_str(&format!("<ConfigurationState>{state}</ConfigurationState>"));
    if let Some(algorithm) = algorithm {
        body.push_str("<EncryptionConfiguration>");
        body.push_str(&format!("<SseAlgorithm>{algorithm}</SseAlgorithm>"));
        if with_key {
            body.push_str(&format!("<KmsKeyArn>{KMS_KEY_ARN}</KmsKeyArn>"));
        }
        body.push_str("</EncryptionConfiguration>");
    }
    body.push_str("</InventoryTableConfiguration>");
    body
}

/// The `MetadataConfiguration` a case starts from: a journal table retaining records for a
/// week, an annotation table managed by an IAM role, and an inventory table configuration in
/// `inventory` — or none at all when the case starts from a configuration that names none.
fn starting_configuration(inventory: Option<&str>) -> String {
    let inventory = match inventory {
        None => String::new(),
        Some(state) => format!(
            "<InventoryTableConfiguration>\
             <ConfigurationState>{state}</ConfigurationState>\
             </InventoryTableConfiguration>"
        ),
    };
    format!(
        "<MetadataConfiguration xmlns=\"{XMLNS}\">\
         <JournalTableConfiguration><RecordExpiration>\
         <Expiration>ENABLED</Expiration><Days>7</Days>\
         </RecordExpiration></JournalTableConfiguration>\
         {inventory}\
         <AnnotationTableConfiguration>\
         <ConfigurationState>ENABLED</ConfigurationState>\
         <Role>{ANNOTATION_ROLE}</Role>\
         </AnnotationTableConfiguration>\
         </MetadataConfiguration>"
    )
}

/// One row of the enable-and-disable table.
#[derive(Debug, Clone)]
struct Case {
    /// The case name every assertion carries.
    name: &'static str,
    /// The general purpose bucket the configuration lives on.
    bucket: &'static str,
    /// The inventory `ConfigurationState` the configuration was CREATED with, or `None` when
    /// the create named no inventory table configuration at all.
    start: Option<&'static str>,
    /// The `ConfigurationState` the update names.
    update: &'static str,
    /// The `SseAlgorithm` the update's encryption configuration names, when it carries one;
    /// the KMS algorithm is named beside the key ARN.
    encryption: Option<&'static str>,
    /// Whether the update leaves an inventory table behind — the table is then the one S3
    /// Metadata names every configuration's inventory table.
    table: bool,
    /// The namespace the row's tables live in, as a table ARN's id carries it (the bucket's
    /// name written in the character set an S3 Tables name is written in) — spelled out, so
    /// what the id is made of is pinned by hand rather than by the code under test.
    namespace: &'static str,
    /// The `TableStatus` the read reports straight after the update, when it leaves a table
    /// behind: `CREATING` for a table this update created, and `ACTIVE` for one the create
    /// already brought into service and the update left alone.
    status_after_the_update: Option<&'static str>,
}

/// Every row: they differ along the state the configuration started in (none named, disabled,
/// enabled), the state the update names, whether the update carries an encryption
/// configuration and which algorithm it names, and the FORM of the bucket's name the
/// namespace is folded from.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a configuration naming no inventory table, enabled",
            bucket: "inventory-update-unnamed-enable",
            start: None,
            update: "ENABLED",
            encryption: None,
            table: true,
            namespace: "b_inventory_update_unnamed_enable",
            status_after_the_update: Some("CREATING"),
        },
        Case {
            name: "a disabled inventory table, enabled with S3 managed encryption",
            bucket: "inventory-update-disabled-enable",
            start: Some("DISABLED"),
            update: "ENABLED",
            encryption: Some("AES256"),
            table: true,
            namespace: "b_inventory_update_disabled_enable",
            status_after_the_update: Some("CREATING"),
        },
        Case {
            name: "an enabled inventory table, disabled",
            bucket: "inventory-update-enabled-disable",
            start: Some("ENABLED"),
            update: "DISABLED",
            encryption: None,
            table: false,
            namespace: "b_inventory_update_enabled_disable",
            status_after_the_update: None,
        },
        Case {
            name: "an enabled inventory table, enabled again with a KMS key",
            bucket: "inventory-update-enabled-again",
            start: Some("ENABLED"),
            update: "ENABLED",
            encryption: Some("aws:kms"),
            table: true,
            namespace: "b_inventory_update_enabled_again",
            // The table the create brought into service is left exactly as it is, so a
            // repeated enable does not put it back into `CREATING`.
            status_after_the_update: Some("ACTIVE"),
        },
        Case {
            name: "a configuration naming no inventory table, disabled",
            bucket: "inventory-update-unnamed-disable",
            start: None,
            update: "DISABLED",
            encryption: None,
            table: false,
            namespace: "b_inventory_update_unnamed_disable",
            status_after_the_update: None,
        },
        Case {
            name: "a bucket name carrying dots and digits",
            bucket: "inventory.update.2026",
            start: Some("DISABLED"),
            update: "ENABLED",
            encryption: None,
            table: true,
            namespace: "b_inventory_update_2026",
            status_after_the_update: Some("CREATING"),
        },
    ]
}

/// What a read reports for the inventory table container of `case`'s configuration, with the
/// table (when the row leaves one) in `expected_status`.
fn assert_inventory(case: &Case, body: &str, expected_status: Option<&str>) {
    let container = inside(body, "InventoryTableConfigurationResult").unwrap_or_else(|| {
        panic!(
            "case {:?}: the update named an inventory table configuration, so the read must \
             report one, got {body:?}",
            case.name
        )
    });
    assert_eq!(
        text(container, "ConfigurationState").as_deref(),
        Some(case.update),
        "case {:?}: expected ConfigurationState {:?}, got {:?}",
        case.name,
        case.update,
        text(container, "ConfigurationState")
    );
    let expected_name = case.table.then_some(INVENTORY_TABLE);
    assert_eq!(
        text(container, "TableName").as_deref(),
        expected_name,
        "case {:?}: expected TableName {expected_name:?}, got {:?}",
        case.name,
        text(container, "TableName")
    );
    assert_eq!(
        text(container, "TableArn"),
        expected_name.map(|table| table_arn(case.namespace, table)),
        "case {:?}: expected TableArn {:?}, got {:?}",
        case.name,
        expected_name.map(|table| table_arn(case.namespace, table)),
        text(container, "TableArn")
    );
    assert_eq!(
        text(container, "TableStatus").as_deref(),
        expected_status,
        "case {:?}: expected TableStatus {expected_status:?}, got {:?}",
        case.name,
        text(container, "TableStatus")
    );
}

/// That the journal and annotation halves of the configuration read back exactly as they did
/// before the update — the update is scoped to the inventory table.
fn assert_the_rest_is_untouched(case: &Case, before: &str, after: &str) {
    for container in [
        "JournalTableConfigurationResult",
        "AnnotationTableConfigurationResult",
        "DestinationResult",
    ] {
        assert_eq!(
            inside(after, container),
            inside(before, container),
            "case {:?}: the update must leave {container} exactly as it was, got {:?} where \
             the read before it reported {:?}",
            case.name,
            inside(after, container),
            inside(before, container)
        );
    }
}

#[tokio::test]
async fn an_update_enables_or_disables_the_inventory_table_and_nothing_else() {
    let router = router();

    for case in cases() {
        create_bucket(&router, case.bucket, None, None).await;
        let created = create_metadata_configuration(
            &router,
            case.bucket,
            &[],
            &starting_configuration(case.start),
        )
        .await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: the configuration this row starts from must be created, got {} with \
             body {:?}",
            case.name,
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        // The tables the CREATE asked for are allowed to reach their terminal status first,
        // so a `CREATING` after the update is this update's own doing and not the create's
        // window still being open.
        wait_for_the_tables();
        let before = get_metadata_configuration(&router, case.bucket, &[]).await;
        let before = String::from_utf8_lossy(&before.body).to_string();

        let answer = update_inventory_table(
            &router,
            case.bucket,
            &[],
            &updates(case.update, case.encryption, case.encryption == Some("aws:kms")),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}: expected the modelled 200, got {} with body {:?}",
            case.name,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.body.is_empty(),
            "case {:?}: the update models a Unit output, so its body must be empty, got {:?}",
            case.name,
            String::from_utf8_lossy(&answer.body)
        );

        let reading = get_metadata_configuration(&router, case.bucket, &[]).await;
        assert_eq!(
            reading.status,
            200,
            "case {:?}: expected the read to answer 200, got {} with body {:?}",
            case.name,
            reading.status,
            String::from_utf8_lossy(&reading.body)
        );
        let after = String::from_utf8_lossy(&reading.body).to_string();
        assert_inventory(&case, &after, case.status_after_the_update);
        assert_the_rest_is_untouched(&case, &before, &after);
    }

    // Once the span the store recorded has passed, a table the update created is reported in
    // service, and a row that disabled its table still has none.
    wait_for_the_tables();
    for case in cases() {
        let reading = get_metadata_configuration(&router, case.bucket, &[]).await;
        let after = String::from_utf8_lossy(&reading.body).to_string();
        assert_inventory(&case, &after, case.table.then_some("ACTIVE"));
    }
}

#[tokio::test]
async fn a_keyless_kms_algorithm_is_refused_and_records_nothing() {
    let router = router();
    let bucket = "inventory-update-kms";
    create_bucket(&router, bucket, None, None).await;
    create_metadata_configuration(&router, bucket, &[], &starting_configuration(Some("DISABLED")))
        .await;
    wait_for_the_tables();
    let before = get_metadata_configuration(&router, bucket, &[]).await;
    let before = String::from_utf8_lossy(&before.body).to_string();

    let refused =
        update_inventory_table(&router, bucket, &[], &updates("ENABLED", Some("aws:kms"), false))
            .await;
    assert_eq!(
        refused.status,
        400,
        "SSE-KMS without a key ARN must be refused, got {} with body {:?}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    assert_eq!(
        refused.error_code(),
        "InvalidArgument",
        "a KMS algorithm with no key ARN beside it is an InvalidArgument, got {:?}",
        String::from_utf8_lossy(&refused.body)
    );
    let unchanged = get_metadata_configuration(&router, bucket, &[]).await;
    assert_eq!(
        String::from_utf8_lossy(&unchanged.body).to_string(),
        before,
        "a refused update must leave the stored configuration exactly as it was"
    );

    // The same update naming the key ARN is accepted, so what is pinned is the missing key
    // and not the algorithm.
    let accepted =
        update_inventory_table(&router, bucket, &[], &updates("ENABLED", Some("aws:kms"), true))
            .await;
    assert_eq!(
        accepted.status,
        200,
        "the same update naming a key ARN must be accepted, got {} with body {:?}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
    let reading = get_metadata_configuration(&router, bucket, &[]).await;
    let body = String::from_utf8_lossy(&reading.body).to_string();
    let container = inside(&body, "InventoryTableConfigurationResult")
        .expect("the accepted update must leave an inventory table configuration behind");
    assert_eq!(
        text(container, "ConfigurationState").as_deref(),
        Some("ENABLED"),
        "the accepted update must enable the inventory table, got {container:?}"
    );
}

#[tokio::test]
async fn an_update_naming_an_address_or_an_owner_this_emulator_does_not_hold_says_which() {
    let router = router();
    let enable = updates("ENABLED", None, false);

    // A bucket this emulator does not hold.
    let absent_bucket =
        update_inventory_table(&router, "inventory-update-no-such-bucket", &[], &enable).await;
    assert_eq!(
        absent_bucket.status,
        404,
        "NoSuchBucket is a 404, got {} with body {:?}",
        absent_bucket.status,
        String::from_utf8_lossy(&absent_bucket.body)
    );
    assert_eq!(
        absent_bucket.error_code(),
        "NoSuchBucket",
        "an update naming no bucket of this emulator must say so, got {:?}",
        String::from_utf8_lossy(&absent_bucket.body)
    );

    // A bucket carrying no metadata configuration: there is no configuration to enable an
    // inventory table for, and the refused update records none.
    let bucket = "inventory-update-no-configuration";
    create_bucket(&router, bucket, None, None).await;
    let absent_configuration = update_inventory_table(&router, bucket, &[], &enable).await;
    assert_eq!(
        absent_configuration.status,
        404,
        "a bucket carrying no metadata configuration answers 404, got {} with body {:?}",
        absent_configuration.status,
        String::from_utf8_lossy(&absent_configuration.body)
    );
    assert_eq!(
        absent_configuration.error_code(),
        "MetadataConfigurationNotFound",
        "an update of a configuration the bucket does not carry must say the metadata \
         configuration is not there, got {:?}",
        String::from_utf8_lossy(&absent_configuration.body)
    );
    assert_eq!(
        absent_configuration.error_message(),
        "The metadata configuration was not found",
        "a refusal carries the text S3 publishes for that code"
    );
    let still_absent = get_metadata_configuration(&router, bucket, &[]).await;
    assert_eq!(
        still_absent.error_code(),
        "MetadataConfigurationNotFound",
        "a refused update must record nothing, got {:?}",
        String::from_utf8_lossy(&still_absent.body)
    );

    // An expected bucket owner that is not this account's: "403 Forbidden (access denied)",
    // with the configuration left exactly as it was.
    create_metadata_configuration(&router, bucket, &[], &starting_configuration(Some("DISABLED")))
        .await;
    wait_for_the_tables();
    let before = get_metadata_configuration(&router, bucket, &[]).await;
    let before = String::from_utf8_lossy(&before.body).to_string();
    let denied = update_inventory_table(
        &router,
        bucket,
        &[("x-amz-expected-bucket-owner", NOT_THE_OWNER)],
        &enable,
    )
    .await;
    assert_eq!(
        denied.status,
        403,
        "an owner mismatch is a 403, got {} with body {:?}",
        denied.status,
        String::from_utf8_lossy(&denied.body)
    );
    assert_eq!(
        denied.error_code(),
        "AccessDenied",
        "an owner mismatch is reported as AccessDenied, got {:?}",
        String::from_utf8_lossy(&denied.body)
    );
    let unchanged = get_metadata_configuration(&router, bucket, &[]).await;
    assert_eq!(
        String::from_utf8_lossy(&unchanged.body).to_string(),
        before,
        "a denied update must leave the stored configuration exactly as it was"
    );
}
