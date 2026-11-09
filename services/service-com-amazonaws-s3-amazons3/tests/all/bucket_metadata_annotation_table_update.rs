// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateBucketMetadataAnnotationTableConfiguration`: enabling and
//! disabling the annotation table of the S3 Metadata configuration a general purpose bucket
//! carries and re-pointing it at another IAM role, read back through
//! `GetBucketMetadataConfiguration`.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns, with the metadata-configuration suite's own
//! helpers for the create, the read and for slicing a response), so what is pinned is the
//! wire behaviour — status, body, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * The operation is used "to enable or disable the annotation table, or to update its
//!   associated IAM role" (its own documentation): the `ConfigurationState` the request
//!   carries replaces the one the configuration carried, whatever state it was in and
//!   whether or not it named an annotation table at all, and the call answers the modelled
//!   200 with an empty body.
//! * A TABLE exists exactly while that state asks for one — which is why
//!   `AnnotationTableConfigurationResult` requires its `ConfigurationState` and leaves its
//!   `TableName`, `TableStatus` and `TableArn` optional: an enable creates the table under
//!   the name derived from the general purpose bucket's name, a disable leaves the state
//!   reported with no table behind it, and an enable of a table that is already there leaves
//!   that table exactly as it is rather than creating a second one.
//! * `TableStatus` is the table's "provisioning status" with "possible values: `CREATING`,
//!   `BACKFILLING`, `ACTIVE`, `FAILED`" (`AnnotationTableConfigurationResult$TableStatus`),
//!   so a table this call creates is reported as `CREATING` until it falls due to be in
//!   service and `ACTIVE` from then on.
//! * `Role` is "the new IAM role ARN to apply"
//!   (`AnnotationTableConfigurationUpdates$Role`) and is optional: a call naming one
//!   replaces the role the configuration reports, and a call naming none leaves it exactly
//!   as it was.
//! * The update is scoped to the annotation table: the journal table, its record expiration
//!   and the inventory table are reported exactly as they were before the call.
//! * "The IAM role must have a trust policy that allows the Amazon S3 metadata service to
//!   assume it, and a permissions policy that grants the actions needed to read annotations
//!   from your bucket" (the same documentation): an enabled annotation table is read through
//!   such a role, so an enable that would leave none in effect is refused with
//!   `InvalidRequest` (HTTP 400) and records nothing.
//! * "If server-side encryption with Key Management Service (KMS) keys (SSE-KMS) is
//!   specified, you must also specify the KMS key Amazon Resource Name (ARN)"
//!   (`MetadataTableEncryptionConfiguration$KmsKeyArn`): a keyless KMS algorithm is refused
//!   with `InvalidArgument` (HTTP 400) and records nothing.
//! * A bucket carrying no metadata configuration has no annotation table to enable or
//!   disable, so the call answers `MetadataConfigurationNotFound` (HTTP 404); a bucket this
//!   emulator does not hold `NoSuchBucket` (HTTP 404), and an `x-amz-expected-bucket-owner`
//!   that is not the account's HTTP 403 `AccessDenied` — each with nothing recorded.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};
use super::bucket_metadata_configuration::{
    create_metadata_configuration, get_metadata_configuration, inside, table_arn, text,
    wait_for_the_tables, ANNOTATION_ROLE, ANNOTATION_TABLE, KMS_KEY_ARN, NOT_THE_OWNER, XMLNS,
};

/// A second IAM role ARN, of the same shape as [`ANNOTATION_ROLE`], so a row can re-point the
/// annotation table at another role and the read-back is pinned to the role the call named
/// rather than to any role being reported at all.
const ANOTHER_ANNOTATION_ROLE: &str = "arn:aws:iam::000000000000:role/s3-metadata-annotations-2";

/// `PUT /<bucket>?metadataAnnotationTable` —
/// `UpdateBucketMetadataAnnotationTableConfiguration`, with `payload` as the
/// `AnnotationTableConfiguration` container and whatever headers the case carries.
async fn update_annotation_table(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
    payload: &str,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?metadataAnnotationTable"),
        headers,
        payload,
    )
    .await
}

/// The `AnnotationTableConfigurationUpdates` document naming `state`, the `role` the call
/// applies when it names one, and an encryption configuration naming `algorithm` when the
/// case carries one — with the KMS key ARN beside it exactly when `with_key` asks for it.
fn updates(state: &str, role: Option<&str>, algorithm: Option<&str>, with_key: bool) -> String {
    let mut body = format!("<AnnotationTableConfiguration xmlns=\"{XMLNS}\">");
    body.push_str(&format!("<ConfigurationState>{state}</ConfigurationState>"));
    if let Some(algorithm) = algorithm {
        body.push_str("<EncryptionConfiguration>");
        body.push_str(&format!("<SseAlgorithm>{algorithm}</SseAlgorithm>"));
        if with_key {
            body.push_str(&format!("<KmsKeyArn>{KMS_KEY_ARN}</KmsKeyArn>"));
        }
        body.push_str("</EncryptionConfiguration>");
    }
    if let Some(role) = role {
        body.push_str(&format!("<Role>{role}</Role>"));
    }
    body.push_str("</AnnotationTableConfiguration>");
    body
}

/// The `MetadataConfiguration` a case starts from: a journal table retaining records for a
/// week, an enabled inventory table, and an annotation table configuration in `annotation`
/// (its state and the role the create named for it) — or none at all when the case starts
/// from a configuration that names none.
fn starting_configuration(annotation: Option<(&str, Option<&str>)>) -> String {
    let annotation = match annotation {
        None => String::new(),
        Some((state, role)) => {
            let role = role.map_or(String::new(), |role| format!("<Role>{role}</Role>"));
            format!(
                "<AnnotationTableConfiguration>\
                 <ConfigurationState>{state}</ConfigurationState>\
                 {role}\
                 </AnnotationTableConfiguration>"
            )
        }
    };
    format!(
        "<MetadataConfiguration xmlns=\"{XMLNS}\">\
         <JournalTableConfiguration><RecordExpiration>\
         <Expiration>ENABLED</Expiration><Days>7</Days>\
         </RecordExpiration></JournalTableConfiguration>\
         <InventoryTableConfiguration>\
         <ConfigurationState>ENABLED</ConfigurationState>\
         </InventoryTableConfiguration>\
         {annotation}\
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
    /// The annotation `ConfigurationState` the configuration was CREATED with and the role the
    /// create named for it, or `None` when the create named no annotation table configuration
    /// at all.
    start: Option<(&'static str, Option<&'static str>)>,
    /// The `ConfigurationState` the update names.
    update: &'static str,
    /// The `Role` the update names, when it names one.
    role: Option<&'static str>,
    /// The `SseAlgorithm` the update's encryption configuration names, when it carries one;
    /// the KMS algorithm is named beside the key ARN.
    encryption: Option<&'static str>,
    /// Whether the update leaves an annotation table behind — the table is then the one S3
    /// Metadata names every configuration's annotation table.
    table: bool,
    /// The namespace the row's tables live in, as a table ARN's id carries it (the bucket's
    /// name written in the character set an S3 Tables name is written in) — spelled out, so
    /// what the id is made of is pinned by hand rather than by the code under test.
    namespace: &'static str,
    /// The `TableStatus` the read reports straight after the update, when it leaves a table
    /// behind: `CREATING` for a table this update created, and `ACTIVE` for one the create
    /// already brought into service and the update left alone.
    status_after_the_update: Option<&'static str>,
    /// The `Role` the read reports after the update: the one the update named, or the one the
    /// configuration already carried when the update named none.
    expected_role: Option<&'static str>,
}

/// Every row: they differ along the state the configuration started in (none named, disabled,
/// enabled) and the role it started with, the state the update names, whether the update
/// names a role and whether that role is the one already in effect, whether it carries an
/// encryption configuration and which algorithm it names, and the FORM of the bucket's name
/// the namespace is folded from.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a configuration naming no annotation table, enabled with a role",
            bucket: "annotation-update-unnamed-enable",
            start: None,
            update: "ENABLED",
            role: Some(ANNOTATION_ROLE),
            encryption: None,
            table: true,
            namespace: "b_annotation_update_unnamed_enable",
            status_after_the_update: Some("CREATING"),
            expected_role: Some(ANNOTATION_ROLE),
        },
        Case {
            name: "a disabled annotation table, enabled at another role with S3 managed \
                   encryption",
            bucket: "annotation-update-disabled-enable",
            start: Some(("DISABLED", Some(ANNOTATION_ROLE))),
            update: "ENABLED",
            role: Some(ANOTHER_ANNOTATION_ROLE),
            encryption: Some("AES256"),
            table: true,
            namespace: "b_annotation_update_disabled_enable",
            status_after_the_update: Some("CREATING"),
            expected_role: Some(ANOTHER_ANNOTATION_ROLE),
        },
        Case {
            name: "an enabled annotation table, disabled",
            bucket: "annotation-update-enabled-disable",
            start: Some(("ENABLED", Some(ANNOTATION_ROLE))),
            update: "DISABLED",
            role: None,
            encryption: None,
            table: false,
            namespace: "b_annotation_update_enabled_disable",
            status_after_the_update: None,
            // The update named no role, so the one the configuration carried is still the one
            // it reports.
            expected_role: Some(ANNOTATION_ROLE),
        },
        Case {
            name: "an enabled annotation table, enabled again naming no role",
            bucket: "annotation-update-enabled-again",
            start: Some(("ENABLED", Some(ANNOTATION_ROLE))),
            update: "ENABLED",
            role: None,
            encryption: None,
            table: true,
            namespace: "b_annotation_update_enabled_again",
            // The table the create brought into service is left exactly as it is, so a
            // repeated enable does not put it back into `CREATING`.
            status_after_the_update: Some("ACTIVE"),
            expected_role: Some(ANNOTATION_ROLE),
        },
        Case {
            name: "a configuration naming no annotation table, disabled",
            bucket: "annotation-update-unnamed-disable",
            start: None,
            update: "DISABLED",
            role: None,
            encryption: None,
            table: false,
            namespace: "b_annotation_update_unnamed_disable",
            status_after_the_update: None,
            expected_role: None,
        },
        Case {
            name: "a bucket name carrying dots and digits, re-pointed with a KMS key",
            bucket: "annotation.update.2026",
            start: Some(("DISABLED", None)),
            update: "ENABLED",
            role: Some(ANOTHER_ANNOTATION_ROLE),
            encryption: Some("aws:kms"),
            table: true,
            namespace: "b_annotation_update_2026",
            status_after_the_update: Some("CREATING"),
            expected_role: Some(ANOTHER_ANNOTATION_ROLE),
        },
    ]
}

/// What a read reports for the annotation table container of `case`'s configuration, with the
/// table (when the row leaves one) in `expected_status`.
fn assert_annotation(case: &Case, body: &str, expected_status: Option<&str>) {
    let container = inside(body, "AnnotationTableConfigurationResult").unwrap_or_else(|| {
        panic!(
            "case {:?}: the update named an annotation table configuration, so the read must \
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
    assert_eq!(
        text(container, "Role").as_deref(),
        case.expected_role,
        "case {:?}: expected Role {:?}, got {:?}",
        case.name,
        case.expected_role,
        text(container, "Role")
    );
    let expected_name = case.table.then_some(ANNOTATION_TABLE);
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

/// That the journal and inventory halves of the configuration read back exactly as they did
/// before the update — the update is scoped to the annotation table.
fn assert_the_rest_is_untouched(case: &Case, before: &str, after: &str) {
    for container in [
        "JournalTableConfigurationResult",
        "InventoryTableConfigurationResult",
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
async fn an_update_enables_or_disables_the_annotation_table_and_nothing_else() {
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

        let answer = update_annotation_table(
            &router,
            case.bucket,
            &[],
            &updates(
                case.update,
                case.role,
                case.encryption,
                case.encryption == Some("aws:kms"),
            ),
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
        assert_annotation(&case, &after, case.status_after_the_update);
        assert_the_rest_is_untouched(&case, &before, &after);
    }

    // Once the span the store recorded has passed, a table the update created is reported in
    // service, and a row that disabled its table still has none.
    wait_for_the_tables();
    for case in cases() {
        let reading = get_metadata_configuration(&router, case.bucket, &[]).await;
        let after = String::from_utf8_lossy(&reading.body).to_string();
        assert_annotation(&case, &after, case.table.then_some("ACTIVE"));
    }
}

#[tokio::test]
async fn an_enable_that_would_leave_no_managing_role_is_refused_and_records_nothing() {
    let router = router();
    let bucket = "annotation-update-roleless";
    create_bucket(&router, bucket, None, None).await;
    create_metadata_configuration(&router, bucket, &[], &starting_configuration(None)).await;
    wait_for_the_tables();
    let before = get_metadata_configuration(&router, bucket, &[]).await;
    let before = String::from_utf8_lossy(&before.body).to_string();

    let refused =
        update_annotation_table(&router, bucket, &[], &updates("ENABLED", None, None, false)).await;
    assert_eq!(
        refused.status,
        400,
        "an enable with no role in effect must be refused, got {} with body {:?}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    assert_eq!(
        refused.error_code(),
        "InvalidRequest",
        "an enable S3 cannot carry out is an InvalidRequest, got {:?}",
        String::from_utf8_lossy(&refused.body)
    );
    assert_eq!(
        refused.error_message(),
        "A parameter or header in your request isn't valid. For details, see the description \
         of this API operation.",
        "a refusal carries the text the modelled shape states for that code"
    );
    let unchanged = get_metadata_configuration(&router, bucket, &[]).await;
    assert_eq!(
        String::from_utf8_lossy(&unchanged.body).to_string(),
        before,
        "a refused update must leave the stored configuration exactly as it was"
    );

    // A DISABLE of the same configuration needs no role: there is no table to read.
    let disabled =
        update_annotation_table(&router, bucket, &[], &updates("DISABLED", None, None, false))
            .await;
    assert_eq!(
        disabled.status,
        200,
        "a disable naming no role must be accepted, got {} with body {:?}",
        disabled.status,
        String::from_utf8_lossy(&disabled.body)
    );

    // The same enable naming a role is accepted, so what is pinned is the missing role and
    // not the enable; and once one is in effect, a later enable may name none.
    let accepted = update_annotation_table(
        &router,
        bucket,
        &[],
        &updates("ENABLED", Some(ANNOTATION_ROLE), None, false),
    )
    .await;
    assert_eq!(
        accepted.status,
        200,
        "the same enable naming a role must be accepted, got {} with body {:?}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
    let again =
        update_annotation_table(&router, bucket, &[], &updates("ENABLED", None, None, false)).await;
    assert_eq!(
        again.status,
        200,
        "an enable of a table whose role is already in effect must be accepted, got {} with \
         body {:?}",
        again.status,
        String::from_utf8_lossy(&again.body)
    );
    let reading = get_metadata_configuration(&router, bucket, &[]).await;
    let body = String::from_utf8_lossy(&reading.body).to_string();
    let container = inside(&body, "AnnotationTableConfigurationResult")
        .expect("the accepted update must leave an annotation table configuration behind");
    assert_eq!(
        text(container, "Role").as_deref(),
        Some(ANNOTATION_ROLE),
        "the role the accepted update named must be the one reported, got {container:?}"
    );
}

#[tokio::test]
async fn a_keyless_kms_algorithm_is_refused_and_records_nothing() {
    let router = router();
    let bucket = "annotation-update-kms";
    create_bucket(&router, bucket, None, None).await;
    create_metadata_configuration(
        &router,
        bucket,
        &[],
        &starting_configuration(Some(("DISABLED", Some(ANNOTATION_ROLE)))),
    )
    .await;
    wait_for_the_tables();
    let before = get_metadata_configuration(&router, bucket, &[]).await;
    let before = String::from_utf8_lossy(&before.body).to_string();

    let refused = update_annotation_table(
        &router,
        bucket,
        &[],
        &updates("ENABLED", Some(ANNOTATION_ROLE), Some("aws:kms"), false),
    )
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
    let accepted = update_annotation_table(
        &router,
        bucket,
        &[],
        &updates("ENABLED", Some(ANNOTATION_ROLE), Some("aws:kms"), true),
    )
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
    let container = inside(&body, "AnnotationTableConfigurationResult")
        .expect("the accepted update must leave an annotation table configuration behind");
    assert_eq!(
        text(container, "ConfigurationState").as_deref(),
        Some("ENABLED"),
        "the accepted update must enable the annotation table, got {container:?}"
    );
}

#[tokio::test]
async fn an_update_naming_an_address_or_an_owner_this_emulator_does_not_hold_says_which() {
    let router = router();
    let enable = updates("ENABLED", Some(ANNOTATION_ROLE), None, false);

    // A bucket this emulator does not hold.
    let absent_bucket =
        update_annotation_table(&router, "annotation-update-no-such-bucket", &[], &enable).await;
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
    // annotation table for, and the refused update records none.
    let bucket = "annotation-update-no-configuration";
    create_bucket(&router, bucket, None, None).await;
    let absent_configuration = update_annotation_table(&router, bucket, &[], &enable).await;
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
    create_metadata_configuration(
        &router,
        bucket,
        &[],
        &starting_configuration(Some(("DISABLED", Some(ANNOTATION_ROLE)))),
    )
    .await;
    wait_for_the_tables();
    let before = get_metadata_configuration(&router, bucket, &[]).await;
    let before = String::from_utf8_lossy(&before.body).to_string();
    let denied = update_annotation_table(
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
