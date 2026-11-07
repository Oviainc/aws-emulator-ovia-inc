// SPDX-License-Identifier: CC0-1.0

//! The storage class of a table bucket and of the tables created in it.
//!
//! The rule under test: a table bucket's storage-class configuration is the
//! DEFAULT storage class for the tables created in it, so a put sets or replaces
//! it, a read of the bucket reports what was last set (the service's own class
//! for a bucket no caller ever configured), and a table reports the class
//! settled when it was created — the create's own configuration when it carried
//! one, else the bucket's default at that moment — which is why a later put is
//! the default for the tables created after it and moves no table that already
//! exists.
//!
//! The tables below span what the rule generalises over: every modeled storage
//! class, where the class came from (the bucket create, a put, the table create,
//! nothing at all), whether the put set, replaced or repeated the bucket's
//! class, whether a table was created before or after the put, and the refusals
//! — a bucket or a table the store does not hold, and a class this service has
//! no storage for.

use serde_json::{json, Value};
use service_com_amazonaws_s3tables_s3tablebuckets as s3tables;

use s3tables::state::{OpenTableFormat, StorageClass};

use super::harness::{expected_bucket_arn, Emulator};

/// The namespace every case in this suite creates its tables in.
const NAMESPACE: &str = "warehouse";

/// A `storageClassConfiguration` naming `storage_class`.
fn configuration(storage_class: &str) -> Value {
    json!({ "storageClass": storage_class })
}

/// Create the table bucket `name` in `region`, with a storage class of its own
/// when the case names one, and answer its ARN.
async fn bucket(
    emulator: &Emulator,
    region: &str,
    name: &str,
    created_with: Option<&str>,
    case: &str,
) -> String {
    let mut request = json!({ "name": name });
    if let Some(storage_class) = created_with {
        request["storageClassConfiguration"] = configuration(storage_class);
    }
    let created = emulator.create_table_bucket(region, &request).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateTableBucket must succeed, got {status} {body}",
        status = created.status,
        body = created.body
    );
    let arn = created.text("arn");
    let namespace = emulator
        .create_namespace(region, &arn, &json!({ "namespace": [NAMESPACE] }))
        .await;
    assert_eq!(
        namespace.status, 200,
        "case {case:?}: CreateNamespace must succeed, got {status} {body}",
        status = namespace.status,
        body = namespace.body
    );
    arn
}

/// Create the table `name`, with a storage class of its own when the case names
/// one.
async fn table(
    emulator: &Emulator,
    region: &str,
    arn: &str,
    name: &str,
    created_with: Option<&str>,
    case: &str,
) {
    // `format` is required and `ICEBERG` is the only value the modeled
    // `OpenTableFormat` enum declares.
    let mut request = json!({ "name": name, "format": OpenTableFormat::Iceberg.as_wire() });
    if let Some(storage_class) = created_with {
        request["storageClassConfiguration"] = configuration(storage_class);
    }
    let created = emulator
        .create_table(region, arn, NAMESPACE, &request)
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateTable must succeed, got {status} {body}",
        status = created.status,
        body = created.body
    );
}

/// The storage class `GetTableBucketStorageClass` reports for `arn`.
async fn bucket_storage_class(emulator: &Emulator, region: &str, arn: &str, case: &str) -> String {
    let read = emulator.get_table_bucket_storage_class(region, arn).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: GetTableBucketStorageClass must succeed, got {status} {body}",
        status = read.status,
        body = read.body
    );
    read.body
        .get("storageClassConfiguration")
        .and_then(|configuration| configuration.get("storageClass"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the read must carry storageClassConfiguration.storageClass: {body}",
                body = read.body
            )
        })
        .to_string()
}

/// The storage class `GetTableStorageClass` reports for one table.
async fn table_storage_class(
    emulator: &Emulator,
    region: &str,
    arn: &str,
    name: &str,
    case: &str,
) -> String {
    let read = emulator
        .get_table_storage_class(region, arn, NAMESPACE, name)
        .await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: GetTableStorageClass must succeed, got {status} {body}",
        status = read.status,
        body = read.body
    );
    read.body
        .get("storageClassConfiguration")
        .and_then(|configuration| configuration.get("storageClass"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the read must carry storageClassConfiguration.storageClass: {body}",
                body = read.body
            )
        })
        .to_string()
}

/// One row of the bucket-default table.
struct DefaultCase {
    label: &'static str,
    region: &'static str,
    bucket: &'static str,
    /// The `storageClassConfiguration` the bucket create carried, when any.
    created_with: Option<&'static str>,
    /// The class a `PutTableBucketStorageClass` then set, when the case puts
    /// one.
    put: Option<&'static str>,
    /// The class the bucket, and a table created after the put, must report.
    expected: &'static str,
}

/// The table: where the bucket's default came from (nothing, the create, a put
/// that configured it, a put that replaced the create's, a put that repeated the
/// current class) crossed with both modeled classes.
fn default_cases() -> Vec<DefaultCase> {
    vec![
        DefaultCase {
            label: "a bucket no caller ever configured",
            region: "us-east-1",
            bucket: "storage-class-unconfigured",
            created_with: None,
            put: None,
            expected: "STANDARD",
        },
        DefaultCase {
            label: "the create named standard storage",
            region: "us-east-1",
            bucket: "storage-class-created-standard",
            created_with: Some("STANDARD"),
            put: None,
            expected: "STANDARD",
        },
        DefaultCase {
            label: "the create named intelligent tiering",
            region: "eu-west-1",
            bucket: "storage-class-created-tiering",
            created_with: Some("INTELLIGENT_TIERING"),
            put: None,
            expected: "INTELLIGENT_TIERING",
        },
        DefaultCase {
            label: "a put configures a bucket that had no class of its own",
            region: "eu-west-1",
            bucket: "storage-class-put-tiering",
            created_with: None,
            put: Some("INTELLIGENT_TIERING"),
            expected: "INTELLIGENT_TIERING",
        },
        DefaultCase {
            label: "a put replaces the class the create named",
            region: "us-west-2",
            bucket: "storage-class-put-replaces",
            created_with: Some("INTELLIGENT_TIERING"),
            put: Some("STANDARD"),
            expected: "STANDARD",
        },
        DefaultCase {
            label: "a put repeats the class the bucket already defaults to",
            region: "us-west-2",
            bucket: "storage-class-put-repeats",
            created_with: Some("INTELLIGENT_TIERING"),
            put: Some("INTELLIGENT_TIERING"),
            expected: "INTELLIGENT_TIERING",
        },
    ]
}

#[tokio::test]
async fn the_bucket_default_round_trips_and_a_new_table_inherits_it() {
    let emulator = Emulator::new();

    for case in default_cases() {
        let label = case.label;
        let arn = bucket(
            &emulator,
            case.region,
            case.bucket,
            case.created_with,
            label,
        )
        .await;

        if let Some(storage_class) = case.put {
            let put = emulator
                .put_table_bucket_storage_class(
                    case.region,
                    &arn,
                    &configuration(storage_class),
                )
                .await;
            assert_eq!(
                put.status, 200,
                "case {label:?}: PutTableBucketStorageClass must succeed, got {status} {body}",
                status = put.status,
                body = put.body
            );
        }

        let reported = bucket_storage_class(&emulator, case.region, &arn, label).await;
        assert_eq!(
            reported, case.expected,
            "case {label:?}: expected the bucket to default to {expected}, got {reported:?}",
            expected = case.expected
        );

        // A table created with no configuration of its own is created in the
        // bucket's default, which is what that default is FOR.
        table(&emulator, case.region, &arn, "inheriting", None, label).await;
        let inherited = table_storage_class(&emulator, case.region, &arn, "inheriting", label).await;
        assert_eq!(
            inherited, case.expected,
            "case {label:?}: expected the table to inherit {expected}, got {inherited:?}",
            expected = case.expected
        );

        // A table whose create names a class of its own overrides the bucket's
        // default, whatever that default is.
        for overriding in StorageClass::ALL {
            let name = format!("overriding_{}", overriding.as_wire().to_lowercase());
            table(
                &emulator,
                case.region,
                &arn,
                &name,
                Some(overriding.as_wire()),
                label,
            )
            .await;
            let reported = table_storage_class(&emulator, case.region, &arn, &name, label).await;
            assert_eq!(
                reported,
                overriding.as_wire(),
                "case {label:?}: a table that names {named:?} must be created in it, got \
                 {reported:?}",
                named = overriding.as_wire()
            );
        }
    }
}

#[tokio::test]
async fn a_put_is_the_default_for_later_tables_only() {
    let emulator = Emulator::new();
    let region = "us-east-1";

    // Both orderings of (the modeled classes), so the assertion does not rest on
    // one direction of the change.
    for (before, after) in [
        ("STANDARD", "INTELLIGENT_TIERING"),
        ("INTELLIGENT_TIERING", "STANDARD"),
    ] {
        let label = format!("{before} then {after}");
        let name = format!("storage-class-{}", after.to_lowercase().replace('_', "-"));
        let arn = bucket(&emulator, region, &name, Some(before), &label).await;

        // One table created under the first default with nothing of its own,
        // and one that named the class it wanted.
        table(&emulator, region, &arn, "earlier", None, &label).await;
        table(&emulator, region, &arn, "earlier_own", Some(after), &label).await;

        let put = emulator
            .put_table_bucket_storage_class(region, &arn, &configuration(after))
            .await;
        assert_eq!(
            put.status, 200,
            "case {label:?}: PutTableBucketStorageClass must succeed, got {status} {body}",
            status = put.status,
            body = put.body
        );

        table(&emulator, region, &arn, "later", None, &label).await;

        for (table_name, expected, why) in [
            ("earlier", before, "a table created before the put keeps its class"),
            ("earlier_own", after, "a table that named its own class keeps it"),
            ("later", after, "a table created after the put takes the new default"),
        ] {
            let reported = table_storage_class(&emulator, region, &arn, table_name, &label).await;
            assert_eq!(
                reported, expected,
                "case {label:?}: {why}: expected {expected} for {table_name}, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_modeled_storage_class_can_be_put_and_read_back() {
    let emulator = Emulator::new();
    let region = "ap-southeast-2";
    // The catalog is the modeled `StorageClass` enum
    // (`smithy_query(query="shape", shape="StorageClass")`), so a value added to
    // the model is covered here without this suite listing it.
    let arn = bucket(
        &emulator,
        region,
        "storage-class-catalog",
        None,
        "the catalog",
    )
    .await;

    for storage_class in StorageClass::ALL {
        let wire = storage_class.as_wire();
        let put = emulator
            .put_table_bucket_storage_class(region, &arn, &configuration(wire))
            .await;
        assert_eq!(
            put.status, 200,
            "case {wire:?}: PutTableBucketStorageClass must succeed, got {status} {body}",
            status = put.status,
            body = put.body
        );
        let reported = bucket_storage_class(&emulator, region, &arn, wire).await;
        assert_eq!(
            reported, wire,
            "case {wire:?}: expected the bucket to default to {wire}, got {reported:?}"
        );

        // A second put of the same class is idempotent: the read is unchanged.
        let repeat = emulator
            .put_table_bucket_storage_class(region, &arn, &configuration(wire))
            .await;
        assert_eq!(
            repeat.status, 200,
            "case {wire:?}: a repeated put must succeed, got {status} {body}",
            status = repeat.status,
            body = repeat.body
        );
        let reported = bucket_storage_class(&emulator, region, &arn, wire).await;
        assert_eq!(
            reported, wire,
            "case {wire:?}: a repeated put must leave the class as it was, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_storage_class_call_changes_nothing() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let arn = bucket(
        &emulator,
        region,
        "storage-class-refusals",
        Some("INTELLIGENT_TIERING"),
        "the refusals",
    )
    .await;
    table(&emulator, region, &arn, "resident", None, "the refusals").await;
    let absent = expected_bucket_arn(region, "storage-class-absent");

    // A class outside the modeled enum is a client-input error, and the bucket
    // keeps the class it had.
    for unsupported in ["GLACIER", "standard", ""] {
        let refused = emulator
            .put_table_bucket_storage_class(region, &arn, &configuration(unsupported))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {unsupported:?}: a class this service has no storage for must be refused as a \
             bad request, got {status} {body}",
            status = refused.status,
            body = refused.body
        );
    }
    let reported = bucket_storage_class(&emulator, region, &arn, "after the refused puts").await;
    assert_eq!(
        reported, "INTELLIGENT_TIERING",
        "case \"after the refused puts\": a refused put must change nothing, got {reported:?}"
    );

    // A table bucket the store does not hold: both bucket-level calls answer the
    // one sentence this service states that class with.
    for (label, reply) in [
        (
            "put against an absent bucket",
            emulator
                .put_table_bucket_storage_class(region, &absent, &configuration("STANDARD"))
                .await,
        ),
        (
            "read of an absent bucket",
            emulator.get_table_bucket_storage_class(region, &absent).await,
        ),
        (
            "table read under an absent bucket",
            emulator
                .get_table_storage_class(region, &absent, NAMESPACE, "resident")
                .await,
        ),
    ] {
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected 404, got {status} {body}",
            status = reply.status,
            body = reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("NotFoundException"),
            "case {label:?}: expected the modeled error code, got {actual:?}",
            actual = reply.error_code
        );
        assert_eq!(
            reply.message(),
            s3tables::state::BUCKET_NOT_FOUND_MESSAGE,
            "case {label:?}: the refusal must state the bucket class sentence"
        );
    }

    // A table the addressed bucket does not hold, whether the namespace is
    // resident or not.
    for (label, namespace, name) in [
        ("a table name nothing holds", NAMESPACE, "absent_table"),
        ("a namespace nothing holds", "no_such_namespace", "resident"),
    ] {
        let refused = emulator
            .get_table_storage_class(region, &arn, namespace, name)
            .await;
        assert_eq!(
            refused.status, 404,
            "case {label:?}: expected 404, got {status} {body}",
            status = refused.status,
            body = refused.body
        );
        assert_eq!(
            refused.message(),
            s3tables::state::TABLE_NOT_FOUND_MESSAGE,
            "case {label:?}: the refusal must state the table class sentence"
        );
    }
}
