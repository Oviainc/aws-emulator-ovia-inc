// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of HOW this service states a refusal.
//!
//! The rule under test: every refusal this service states carries the ONE fixed
//! sentence that names its CLASS of failure — a taken name, a resource the store
//! does not hold, a container that still holds records, a stale version token —
//! and no name, ARN or token from the request appears in it. The same class
//! therefore reads identically whichever resource was addressed, which is what a
//! client branching on `message` (the only discriminator `ConflictException` and
//! `NotFoundException` carry) depends on.
//!
//! Each table spans the class the rule generalises over — the published name
//! bounds, the character classes the modelled patterns allow, how many records a
//! container holds, which form a presented version token takes — rather than one
//! recorded request, and each case asserts the whole sentence.

use serde_json::json;

use crate::harness::{expected_bucket_arn, Emulator, Reply};

const REGION: &str = "us-east-1";

/// The sentences this service states each refusal class with. They are the
/// service's own wording for the class, not this suite's paraphrase, and they
/// carry no variable part: that is exactly what is asserted below.
const BUCKET_EXISTS: &str = "The bucket that you tried to create already exists, and you own it.";
const NAMESPACE_EXISTS: &str = "A namespace with an identical name already exists in the bucket.";
const TABLE_EXISTS: &str = "A table with an identical name already exists in the namespace.";
const NO_BUCKET: &str = "The specified bucket does not exist.";
const NO_NAMESPACE: &str = "The specified namespace does not exist.";
const NO_TABLE: &str = "The specified table does not exist.";
const BUCKET_NOT_EMPTY: &str = "The bucket that you tried to delete is not empty.";
const NAMESPACE_NOT_EMPTY: &str = "The namespace that you tried to delete is not empty.";
const STALE_TABLE_VERSION_TOKEN: &str =
    "Provided version token does not match the table version token.";

/// One row of names to address the whole surface with.
struct Row {
    label: &'static str,
    /// `TableBucketName`: `@length(3, 63)`, `@pattern(^[0-9a-z-]*$)`.
    bucket: String,
    /// `NamespaceName`: `@length(1, 255)`, `@pattern(^[0-9a-z_]*$)`.
    namespace: String,
    /// `TableName`: `@length(1, 255)`, `@pattern(^[0-9a-z_]*$)`.
    table: String,
}

/// The rows every refusal below is asserted over: the shortest and the longest
/// names the modelled `@length` bounds publish, and one row of each character
/// class the modelled `@pattern`s allow (digits, and the separator each name
/// form takes — a hyphen for a bucket, an underscore for a namespace or table).
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "shortest published names",
            bucket: "ab7".to_string(),
            namespace: "q".to_string(),
            table: "z".to_string(),
        },
        Row {
            label: "longest published names",
            bucket: format!("x{}", "9".repeat(62)),
            namespace: format!("w{}", "4".repeat(254)),
            table: format!("v{}", "6".repeat(254)),
        },
        Row {
            label: "digits and separators",
            bucket: "7-probe-bucket-3".to_string(),
            namespace: "8_probe_namespace_2".to_string(),
            table: "5_probe_table_1".to_string(),
        },
    ]
}

/// Assert `reply` is the refusal `expected_message` states, carrying `status`
/// and `code`, and that the sentence took nothing from the request: none of
/// `values` appears in it.
///
/// A one- or two-character name occurs inside ordinary English words, so only
/// the values long enough to be distinctive are searched for.
fn assert_refusal(reply: &Reply, status: u16, code: &str, expected_message: &str, case: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected status {status}, got {actual} {body}",
        actual = reply.status,
        body = reply.body
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some(code),
        "case {case:?}: expected the modelled {code}, got {actual:?}",
        actual = reply.error_code
    );
    assert_eq!(
        reply.message(),
        expected_message,
        "case {case:?}: expected the service's fixed sentence {expected_message:?}, got \
         {actual:?}",
        actual = reply.message()
    );
}

/// Assert the sentence `reply` carries took nothing from the request.
fn assert_no_request_values(reply: &Reply, values: &[&str], case: &str) {
    let message = reply.message();
    for value in values {
        if value.chars().count() < 3 {
            continue;
        }
        assert!(
            !message.contains(value),
            "case {case:?}: the refusal must not carry the request's {value:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_name_already_taken_is_refused_with_the_services_fixed_conflict_sentence() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = row.label;
        let created = emulator
            .create_table_bucket(REGION, &json!({ "name": row.bucket }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the first create of a bucket must be served, got {status} {body}",
            status = created.status,
            body = created.body
        );
        let arn = created.text("arn");

        let repeat_bucket = emulator
            .create_table_bucket(REGION, &json!({ "name": row.bucket }))
            .await;
        assert_refusal(&repeat_bucket, 409, "ConflictException", BUCKET_EXISTS, case);
        assert_no_request_values(&repeat_bucket, &[&row.bucket, &arn], case);

        let namespace_request = json!({ "namespace": [row.namespace.clone()] });
        let created = emulator
            .create_namespace(REGION, &arn, &namespace_request)
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the first create of a namespace must be served, got {status} {body}",
            status = created.status,
            body = created.body
        );
        let repeat_namespace = emulator
            .create_namespace(REGION, &arn, &namespace_request)
            .await;
        assert_refusal(
            &repeat_namespace,
            409,
            "ConflictException",
            NAMESPACE_EXISTS,
            case,
        );
        assert_no_request_values(&repeat_namespace, &[&row.namespace, &arn], case);

        let table_request = json!({ "name": row.table, "format": "ICEBERG" });
        let created = emulator
            .create_table(REGION, &arn, &row.namespace, &table_request)
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the first create of a table must be served, got {status} {body}",
            status = created.status,
            body = created.body
        );
        let repeat_table = emulator
            .create_table(REGION, &arn, &row.namespace, &table_request)
            .await;
        assert_refusal(&repeat_table, 409, "ConflictException", TABLE_EXISTS, case);
        assert_no_request_values(
            &repeat_table,
            &[&row.table, &row.namespace, &arn],
            case,
        );
    }
}

#[tokio::test]
async fn a_resource_the_store_does_not_hold_is_refused_with_the_services_fixed_not_found_sentence() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = row.label;
        // An ARN that is well formed and names no bucket: every operation that
        // addresses a bucket states the one not-found sentence for it.
        let absent_arn = expected_bucket_arn(REGION, &row.bucket);
        let namespace_request = json!({ "namespace": [row.namespace.clone()] });
        let table_request = json!({ "name": row.table, "format": "ICEBERG" });

        let about_absent_bucket = vec![
            ("GetTableBucket", emulator.get_table_bucket(REGION, &absent_arn).await),
            (
                "DeleteTableBucket",
                emulator.delete_table_bucket(REGION, &absent_arn).await,
            ),
            (
                "CreateNamespace",
                emulator
                    .create_namespace(REGION, &absent_arn, &namespace_request)
                    .await,
            ),
            (
                "GetNamespace",
                emulator
                    .get_namespace(REGION, &absent_arn, &row.namespace)
                    .await,
            ),
            (
                "DeleteNamespace",
                emulator
                    .delete_namespace(REGION, &absent_arn, &row.namespace)
                    .await,
            ),
            (
                "CreateTable",
                emulator
                    .create_table(REGION, &absent_arn, &row.namespace, &table_request)
                    .await,
            ),
            (
                "ListTables",
                emulator.list_tables(REGION, &absent_arn, &[]).await,
            ),
        ];
        for (operation, reply) in &about_absent_bucket {
            let case = format!("{case} / {operation} on a bucket that was never created");
            assert_refusal(reply, 404, "NotFoundException", NO_BUCKET, &case);
            assert_no_request_values(reply, &[&row.bucket, &absent_arn], &case);
        }

        // With the bucket there but the namespace absent, the same operations
        // state the namespace's own not-found sentence.
        let arn = emulator
            .create_table_bucket(REGION, &json!({ "name": row.bucket }))
            .await
            .text("arn");
        let about_absent_namespace = vec![
            (
                "GetNamespace",
                emulator.get_namespace(REGION, &arn, &row.namespace).await,
            ),
            (
                "DeleteNamespace",
                emulator.delete_namespace(REGION, &arn, &row.namespace).await,
            ),
            (
                "CreateTable",
                emulator
                    .create_table(REGION, &arn, &row.namespace, &table_request)
                    .await,
            ),
            (
                "ListTables",
                emulator
                    .list_tables(REGION, &arn, &[("namespace", row.namespace.clone())])
                    .await,
            ),
        ];
        for (operation, reply) in &about_absent_namespace {
            let case = format!("{case} / {operation} on a namespace that was never created");
            assert_refusal(reply, 404, "NotFoundException", NO_NAMESPACE, &case);
            assert_no_request_values(reply, &[&row.namespace, &arn], &case);
        }

        // And with the namespace there but the table absent, the table's.
        emulator
            .create_namespace(REGION, &arn, &namespace_request)
            .await;
        let about_absent_table = vec![
            (
                "GetTable",
                emulator
                    .get_table(REGION, &arn, &row.namespace, &row.table)
                    .await,
            ),
            (
                "DeleteTable",
                emulator
                    .delete_table(REGION, &arn, &row.namespace, &row.table, &[])
                    .await,
            ),
        ];
        for (operation, reply) in &about_absent_table {
            let case = format!("{case} / {operation} on a table that was never created");
            assert_refusal(reply, 404, "NotFoundException", NO_TABLE, &case);
            assert_no_request_values(reply, &[&row.table, &row.namespace, &arn], &case);
        }

        // A bucket that WAS created and then deleted is not found either: the
        // sentence states the class, not how the bucket came to be absent.
        assert_eq!(
            emulator
                .delete_namespace(REGION, &arn, &row.namespace)
                .await
                .status,
            204,
            "case {case:?}: emptying the bucket must be served"
        );
        assert_eq!(
            emulator.delete_table_bucket(REGION, &arn).await.status,
            204,
            "case {case:?}: deleting the emptied bucket must be served"
        );
        let after_delete = emulator.get_table_bucket(REGION, &arn).await;
        let deleted_case = format!("{case} / GetTableBucket on a deleted bucket");
        assert_refusal(
            &after_delete,
            404,
            "NotFoundException",
            NO_BUCKET,
            &deleted_case,
        );
        assert_no_request_values(&after_delete, &[&row.bucket, &arn], &deleted_case);
    }
}

#[tokio::test]
async fn a_delete_of_a_container_that_still_holds_records_is_refused_with_the_fixed_sentence() {
    let emulator = Emulator::new();

    // How many tables the namespace holds varies the row: one record is enough
    // to refuse the delete, and more records do not change the sentence.
    for (row, held) in rows().into_iter().zip([1usize, 2, 3]) {
        let case = format!("{label} / {held} table(s) held", label = row.label);
        let arn = emulator
            .create_table_bucket(REGION, &json!({ "name": row.bucket }))
            .await
            .text("arn");
        emulator
            .create_namespace(REGION, &arn, &json!({ "namespace": [row.namespace.clone()] }))
            .await;
        // The names the held tables take, built from the row's own name but
        // kept inside `TableName`'s @length(1, 255).
        let base: String = row.table.chars().take(240).collect();
        for index in 0..held {
            let name = format!("{base}_{index}");
            let created = emulator
                .create_table(
                    REGION,
                    &arn,
                    &row.namespace,
                    &json!({ "name": name, "format": "ICEBERG" }),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the table must be created, got {status} {body}",
                status = created.status,
                body = created.body
            );
        }

        let namespace_refused = emulator
            .delete_namespace(REGION, &arn, &row.namespace)
            .await;
        // A namespace that still holds tables is refused as the modelled
        // conflict this service states it with.
        assert_refusal(
            &namespace_refused,
            409,
            "ConflictException",
            NAMESPACE_NOT_EMPTY,
            &case,
        );
        assert_no_request_values(&namespace_refused, &[&row.namespace, &arn], &case);

        // A bucket that still holds namespaces is the CLIENT's request error:
        // emptying it, not retrying the same request, is what makes the delete
        // possible.
        let bucket_refused = emulator.delete_table_bucket(REGION, &arn).await;
        assert_refusal(
            &bucket_refused,
            400,
            "BadRequestException",
            BUCKET_NOT_EMPTY,
            &case,
        );
        assert_no_request_values(&bucket_refused, &[&row.bucket, &arn], &case);

        // A refused delete persists nothing: both containers are still there.
        assert_eq!(
            emulator
                .get_namespace(REGION, &arn, &row.namespace)
                .await
                .status,
            200,
            "case {case:?}: a refused delete must leave the namespace in place"
        );
        assert_eq!(
            emulator.get_table_bucket(REGION, &arn).await.status,
            200,
            "case {case:?}: a refused delete must leave the bucket in place"
        );
    }
}

#[tokio::test]
async fn a_version_token_that_is_not_the_tables_own_is_refused_with_the_fixed_sentence() {
    let emulator = Emulator::new();

    for row in rows() {
        let arn = emulator
            .create_table_bucket(REGION, &json!({ "name": row.bucket }))
            .await
            .text("arn");
        emulator
            .create_namespace(REGION, &arn, &json!({ "namespace": [row.namespace.clone()] }))
            .await;
        let created = emulator
            .create_table(
                REGION,
                &arn,
                &row.namespace,
                &json!({ "name": row.table, "format": "ICEBERG" }),
            )
            .await;
        let token = created.text("versionToken");
        let sibling_base: String = row.table.chars().take(240).collect();
        // A sibling's token is another table's current one, so it is a token the
        // store holds and still not this table's.
        let sibling = emulator
            .create_table(
                REGION,
                &arn,
                &row.namespace,
                &json!({
                    // Inside `TableName`'s @length(1, 255), as above.
                    "name": format!("{base}_sibling", base = sibling_base),
                    "format": "ICEBERG",
                }),
            )
            .await;
        let sibling_token = sibling.text("versionToken");

        // The forms a presented token takes: one character longer, one
        // character shorter, a token of the same length made of other
        // characters, and another live table's token.
        let presented = [
            ("a longer token", format!("{token}0")),
            (
                "a shorter token",
                token.chars().take(token.chars().count() - 1).collect(),
            ),
            ("another token of the same length", "0".repeat(token.chars().count())),
            ("another table's token", sibling_token.clone()),
        ];
        for (form, value) in presented {
            let case = format!("{label} / {form}", label = row.label);
            let refused = emulator
                .delete_table(
                    REGION,
                    &arn,
                    &row.namespace,
                    &row.table,
                    &[("versionToken", value.clone())],
                )
                .await;
            assert_refusal(
                &refused,
                409,
                "ConflictException",
                STALE_TABLE_VERSION_TOKEN,
                &case,
            );
            assert_no_request_values(&refused, &[&row.table, &row.namespace, &value], &case);
            assert_eq!(
                emulator
                    .get_table(REGION, &arn, &row.namespace, &row.table)
                    .await
                    .status,
                200,
                "case {case:?}: a refused delete must leave the table in place"
            );
        }

        // The table's own token is what authorizes the delete.
        assert_eq!(
            emulator
                .delete_table(
                    REGION,
                    &arn,
                    &row.namespace,
                    &row.table,
                    &[("versionToken", token.clone())]
                )
                .await
                .status,
            204,
            "case {label:?}: the table's own token must authorize the delete",
            label = row.label
        );
    }
}
