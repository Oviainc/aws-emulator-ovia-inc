// SPDX-License-Identifier: CC0-1.0

//! Data Catalog encryption at rest, end to end: the catalog's security configuration
//! wired to the KMS key it names.
//!
//! The rule under test: a catalog object (database, table, partition) written while its
//! catalog's `EncryptionAtRest` names a KMS key is encrypted under THAT key, and every
//! later read of that object answers it only after KMS has opened its data key again —
//! while the key cannot be used, the read is refused with HTTP 400
//! `GlueEncryptionException` carrying KMS's own refusal, and once the key can be used
//! again the very same metadata is answered. The key belongs to each WRITE, because the
//! configuration "is applied to every catalog write thereafter"
//! (`PutDataCatalogEncryptionSettings`): an object written before the key was configured,
//! or under an earlier key, is unaffected by what happens to a later one.
//!
//! KMS is mounted beside Glue on the SAME engine, as the assembled emulator mounts them,
//! because that is the only way the catalog's writes and reads reach the real key records
//! `CreateKey`, `DisableKey`, `EnableKey`, `ScheduleKeyDeletion` and `CancelKeyDeletion`
//! wrote. The refusal's WORDING is never written out here: it is held to what KMS itself
//! answers for the same key, which is what "relayed" means.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! encryption mode, the form the key is named by, how the key is made unusable (and
//! restored), the class of catalog object, and which reader answers it.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// An emulator with Glue AND KMS mounted on one shared engine, with request
/// authentication off (these tests send no real signature), driven through the same
/// router the emulator boots.
fn emulator_with_kms() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        engine,
    ));
    router
}

/// Make one KMS call through the router and answer its HTTP status and parsed body.
///
/// KMS is an awsJson1_1 service whose operations are addressed by the `TrentService`
/// target prefix; the router routes on the credential scope's service field, which is why
/// this one header differs from the Glue harness's.
async fn kms(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDGLUETEST/20260101/{REGION}/kms/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0000000000000000000000000000000000000000000\
         000000000000000000000"
    );
    let request = smithy_server_runtime::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header("authorization", authorization)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the KMS request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One KMS call that must succeed.
async fn kms_ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = kms(router, operation, body).await;
    assert_eq!(
        status, 200,
        "KMS {operation} must answer 200, got {status} with {parsed}"
    );
    parsed
}

/// A customer managed key minted through the mounted KMS, as `(KeyId, Arn)`.
async fn create_key(router: &MultiServiceRouter, description: &str) -> (String, String) {
    let created = kms_ok(router, "CreateKey", &json!({ "Description": description })).await;
    let metadata = &created["KeyMetadata"];
    (
        metadata["KeyId"]
            .as_str()
            .expect("CreateKey reports a KeyId")
            .to_string(),
        metadata["Arn"]
            .as_str()
            .expect("CreateKey reports the key ARN")
            .to_string(),
    )
}

/// The `Message` of a KMS failure body, whichever casing the envelope used — KMS models
/// its exception member as `message`.
fn kms_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// What KMS itself answers when asked to use `key_id` for a cryptographic operation right
/// now: the refusal the catalog has to relay, taken from KMS rather than written out here.
async fn kms_refusal_for(router: &MultiServiceRouter, key_id: &str) -> String {
    let (status, body) = kms(router, "GenerateDataKey", &json!({ "KeyId": key_id })).await;
    assert_eq!(
        status, 400,
        "a key that cannot be used must have KMS refuse a cryptographic operation on it, \
         got {status} with {body}"
    );
    kms_message(&body)
}

/// How a key is taken out of use and put back — the two ways KMS's own key states reach a
/// key that cannot be used in a cryptographic operation.
#[derive(Clone, Copy, Debug)]
enum KeyOutage {
    /// `DisableKey` leaves the key in `Disabled`; `EnableKey` puts it back.
    Disabled,
    /// `ScheduleKeyDeletion` leaves it in `PendingDeletion`; `CancelKeyDeletion` puts it
    /// back.
    PendingDeletion,
}

impl KeyOutage {
    /// Take the key out of use.
    async fn begin(self, router: &MultiServiceRouter, key_id: &str) {
        match self {
            Self::Disabled => {
                kms_ok(router, "DisableKey", &json!({ "KeyId": key_id })).await;
            }
            Self::PendingDeletion => {
                kms_ok(
                    router,
                    "ScheduleKeyDeletion",
                    // The shortest window the model accepts, so the row says nothing about
                    // how long a deletion waits.
                    &json!({ "KeyId": key_id, "PendingWindowInDays": 7 }),
                )
                .await;
            }
        }
    }

    /// Put it back in use.
    ///
    /// Cancelling a scheduled deletion leaves the key DISABLED — "when this operation is
    /// successful, the key state of the KMS key changes to `Disabled`"
    /// (`CancelKeyDeletion`) — so that row has to enable it as well before it can be used
    /// again.
    async fn end(self, router: &MultiServiceRouter, key_id: &str) {
        if matches!(self, Self::PendingDeletion) {
            kms_ok(router, "CancelKeyDeletion", &json!({ "KeyId": key_id })).await;
        }
        kms_ok(router, "EnableKey", &json!({ "KeyId": key_id })).await;
    }
}

/// How the catalog's setting names its key.
#[derive(Clone, Copy, Debug)]
enum KeyForm {
    /// The bare id KMS minted.
    Id,
    /// The key ARN.
    Arn,
}

/// One row of the main table.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// A `CatalogEncryptionMode` that encrypts catalog writes under the named key.
    mode: &'static str,
    key_form: KeyForm,
    outage: KeyOutage,
    /// Whether the request names its `CatalogId` or lets it default — the same catalog
    /// either way.
    name_catalog_id: bool,
}

/// The rows: both modes that name a key, both forms the key is named by, both ways a key
/// leaves use, with and without an explicit `CatalogId`.
///
/// The modes come from the modelled `CatalogEncryptionMode` enum (`smithy_query(query=
/// "shape",shape="CatalogEncryptionMode")`), whose values are `DISABLED`, `SSE-KMS` and
/// `SSE-KMS-WITH-SERVICE-ROLE`; the two that name a key are here, and `DISABLED` — the one
/// that names none — is covered by
/// [`an_object_written_under_no_key_is_unaffected_by_the_catalogs_later_key`].
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded case: server-side encryption enforced on a catalog, then the
            // CMK revoked.
            case: "SSE-KMS, the key named by id, disabled",
            mode: "SSE-KMS",
            key_form: KeyForm::Id,
            outage: KeyOutage::Disabled,
            name_catalog_id: false,
        },
        Row {
            case: "SSE-KMS, the key named by ARN, deletion scheduled",
            mode: "SSE-KMS",
            key_form: KeyForm::Arn,
            outage: KeyOutage::PendingDeletion,
            name_catalog_id: true,
        },
        Row {
            case: "SSE-KMS with a service role, the key named by ARN, disabled",
            mode: "SSE-KMS-WITH-SERVICE-ROLE",
            key_form: KeyForm::Arn,
            outage: KeyOutage::Disabled,
            name_catalog_id: false,
        },
        Row {
            case: "SSE-KMS with a service role, the key named by id, deletion scheduled",
            mode: "SSE-KMS-WITH-SERVICE-ROLE",
            key_form: KeyForm::Id,
            outage: KeyOutage::PendingDeletion,
            name_catalog_id: true,
        },
    ]
}

/// An IAM role ARN matching the modelled `IAMRoleArn` pattern, which is what
/// `EncryptionAtRest$CatalogEncryptionServiceRole` targets.
const SERVICE_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GlueCatalogEncryption";

/// The one partition value every table below is partitioned by, so a partition exists to
/// be read.
const PARTITION_VALUE: &str = "2026-01-01";

/// Configure `catalog` to encrypt its writes under `key`, and assert the write is served.
async fn enforce_encryption(
    router: &MultiServiceRouter,
    case: &str,
    mode: &str,
    key: &str,
    name_catalog_id: bool,
) {
    let mut at_rest = json!({ "CatalogEncryptionMode": mode, "SseAwsKmsKeyId": key });
    if mode == "SSE-KMS-WITH-SERVICE-ROLE" {
        at_rest["CatalogEncryptionServiceRole"] = json!(SERVICE_ROLE_ARN);
    }
    let mut request = json!({
        "DataCatalogEncryptionSettings": { "EncryptionAtRest": at_rest }
    });
    if name_catalog_id {
        request["CatalogId"] = json!(ACCOUNT_ID);
    }
    let (status, body) = call(router, "PutDataCatalogEncryptionSettings", &request).await;
    assert_eq!(
        status, 200,
        "case {case}: enforcing encryption under a usable key must be served, got {status} \
         with {body}"
    );
}

/// Create a database, a partitioned table in it and one partition of that table, each
/// through the wire, and assert every write is served.
async fn create_objects(router: &MultiServiceRouter, case: &str, database: &str, table: &str) {
    for (operation, request) in object_writes(database, table) {
        let (status, body) = call(router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case}: {operation} must be served while the catalog's key is usable, got \
             {status} with {body}"
        );
    }
}

/// The three writes that put one database, one table and one partition in the catalog.
fn object_writes(database: &str, table: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "CreateDatabase",
            json!({ "DatabaseInput": { "Name": database } }),
        ),
        (
            "CreateTable",
            json!({
                "DatabaseName": database,
                "TableInput": {
                    "Name": table,
                    "TableType": "EXTERNAL_TABLE",
                    "PartitionKeys": [{ "Name": "dt", "Type": "string" }],
                    "StorageDescriptor": {
                        "Location": format!("s3://{database}-nonexistent/data/"),
                        "Columns": [{ "Name": "id", "Type": "bigint" }]
                    }
                }
            }),
        ),
        (
            "CreatePartition",
            json!({
                "DatabaseName": database,
                "TableName": table,
                "PartitionInput": { "Values": [PARTITION_VALUE] }
            }),
        ),
    ]
}

/// Every implemented catalog read that answers a stored database, table or partition.
fn object_reads(database: &str, table: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("GetDatabase", json!({ "Name": database })),
        (
            "GetTable",
            json!({ "DatabaseName": database, "Name": table }),
        ),
        ("GetTables", json!({ "DatabaseName": database })),
        (
            "GetTableVersion",
            json!({ "DatabaseName": database, "TableName": table }),
        ),
        (
            "GetPartition",
            json!({
                "DatabaseName": database,
                "TableName": table,
                "PartitionValues": [PARTITION_VALUE]
            }),
        ),
        (
            "BatchGetPartition",
            json!({
                "DatabaseName": database,
                "TableName": table,
                "PartitionsToGet": [{ "Values": [PARTITION_VALUE] }]
            }),
        ),
    ]
}

/// The main rule: while the key a catalog object was written under cannot be used, every
/// read of that object is refused with KMS's own refusal relayed as
/// `GlueEncryptionException`; before and after, the same metadata is answered.
#[tokio::test]
async fn a_catalog_read_depends_on_the_key_its_write_was_encrypted_under() {
    for row in rows() {
        let router = emulator_with_kms();
        let (key_id, key_arn) = create_key(&router, "the catalog's CMK").await;
        let named_key = match row.key_form {
            KeyForm::Id => key_id.clone(),
            KeyForm::Arn => key_arn.clone(),
        };
        enforce_encryption(&router, row.case, row.mode, &named_key, row.name_catalog_id).await;

        let database = "catalog_encryption_db";
        let table = "catalog_encryption_tbl";
        create_objects(&router, row.case, database, table).await;

        // What each read answers while the key is usable, kept so the answer after the
        // key comes back can be held to it.
        let mut served: Vec<(&'static str, Value)> = Vec::new();
        for (operation, request) in object_reads(database, table) {
            let answer = ok(&router, operation, &request).await;
            served.push((operation, answer));
        }

        row.outage.begin(&router, &key_id).await;
        // The refusal the catalog must relay, as KMS words it for this key right now.
        let relayed = kms_refusal_for(&router, &key_id).await;
        assert!(
            relayed.contains(&key_arn),
            "case {}: KMS's own refusal names the key it is about, got {relayed:?}",
            row.case
        );

        for (operation, request) in object_reads(database, table) {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {}: {operation} must be refused while the catalog's key cannot be \
                 used, got {status} with {body}",
                row.case
            );
            assert!(
                error_code(&body).contains("GlueEncryptionException"),
                "case {}: {operation} must be refused as GlueEncryptionException, got {body}",
                row.case
            );
            assert_eq!(
                error_message(&body),
                relayed,
                "case {}: {operation} must relay KMS's own refusal, got {:?}",
                row.case,
                error_message(&body)
            );
        }

        row.outage.end(&router, &key_id).await;
        for (operation, answer) in served {
            let request = object_reads(database, table)
                .into_iter()
                .find(|(name, _)| *name == operation)
                .map(|(_, request)| request)
                .expect("every served read has its request");
            let again = ok(&router, operation, &request).await;
            assert_eq!(
                again, answer,
                "case {}: {operation} must answer the identical metadata once the key is \
                 usable again; expected {answer}, got {again}",
                row.case
            );
        }
    }
}

/// A catalog write is encrypted too, so while the catalog's key cannot be used no object
/// can be written — and nothing is left behind.
#[tokio::test]
async fn a_catalog_write_is_refused_while_the_catalogs_key_cannot_be_used() {
    for row in rows() {
        let router = emulator_with_kms();
        let (key_id, key_arn) = create_key(&router, "the catalog's CMK").await;
        let named_key = match row.key_form {
            KeyForm::Id => key_id.clone(),
            KeyForm::Arn => key_arn,
        };
        enforce_encryption(&router, row.case, row.mode, &named_key, row.name_catalog_id).await;

        // One database and table that exist before the outage, so the UPDATE writes have
        // something to address.
        let database = "write_refusal_db";
        let table = "write_refusal_tbl";
        create_objects(&router, row.case, database, table).await;

        row.outage.begin(&router, &key_id).await;
        let relayed = kms_refusal_for(&router, &key_id).await;

        let mut writes = object_writes("write_refusal_db_two", "write_refusal_tbl_two");
        // The creates above address a database that does not exist yet, so the table and
        // partition creates are retargeted at the existing pair: what is under test is the
        // encryption refusal, not an entity miss.
        writes[1].1 = json!({
            "DatabaseName": database,
            "TableInput": { "Name": "write_refusal_tbl_two" }
        });
        writes[2].1 = json!({
            "DatabaseName": database,
            "TableName": table,
            "PartitionInput": { "Values": ["2026-01-02"] }
        });
        writes.push((
            "BatchCreatePartition",
            json!({
                "DatabaseName": database,
                "TableName": table,
                "PartitionInputList": [{ "Values": ["2026-01-03"] }]
            }),
        ));
        writes.push((
            "UpdateDatabase",
            json!({
                "Name": database,
                "DatabaseInput": { "Name": database, "Description": "rewritten" }
            }),
        ));
        writes.push((
            "UpdateTable",
            json!({
                "DatabaseName": database,
                "TableInput": { "Name": table, "Description": "rewritten" }
            }),
        ));

        for (operation, request) in &writes {
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, 400,
                "case {}: {operation} must be refused while the catalog's key cannot be \
                 used, got {status} with {body}",
                row.case
            );
            assert!(
                error_code(&body).contains("GlueEncryptionException"),
                "case {}: {operation} must be refused as GlueEncryptionException, got {body}",
                row.case
            );
            assert_eq!(
                error_message(&body),
                relayed,
                "case {}: {operation} must relay KMS's own refusal, got {:?}",
                row.case,
                error_message(&body)
            );
        }

        // Nothing the refused writes named was stored, and nothing they would have
        // rewritten changed.
        row.outage.end(&router, &key_id).await;
        let (status, body) = call(
            &router,
            "GetTable",
            &json!({ "DatabaseName": database, "Name": "write_refusal_tbl_two" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {}: a refused create must leave no table behind, got {status} with {body}",
            row.case
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {}: a refused create must leave no table behind, got {body}",
            row.case
        );
        let table_now = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": database, "Name": table }),
        )
        .await;
        assert_eq!(
            table_now["Table"]["Description"],
            Value::Null,
            "case {}: a refused update must leave the stored table alone, got {table_now}",
            row.case
        );
    }
}

/// The key belongs to the WRITE: an object written while the catalog encrypted its writes
/// under no key stays readable whatever happens to a key configured afterwards.
#[tokio::test]
async fn an_object_written_under_no_key_is_unaffected_by_the_catalogs_later_key() {
    // (case, the mode the catalog is in while the objects are written)
    let cases: Vec<(&str, Option<&str>)> = vec![
        ("no setting at all", None),
        ("encryption at rest turned off explicitly", Some("DISABLED")),
    ];

    for (case, mode) in cases {
        let router = emulator_with_kms();
        if let Some(mode) = mode {
            let request = json!({
                "DataCatalogEncryptionSettings": {
                    "EncryptionAtRest": { "CatalogEncryptionMode": mode }
                }
            });
            let answer = ok(&router, "PutDataCatalogEncryptionSettings", &request).await;
            assert_eq!(
                answer,
                json!({}),
                "case {case}: the write reports no members, got {answer}"
            );
        }

        let database = "plain_db";
        let table = "plain_tbl";
        create_objects(&router, case, database, table).await;
        let before = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": database, "Name": table }),
        )
        .await;

        // Only NOW is a key configured, and then revoked: the objects above were not
        // written under it.
        let (key_id, key_arn) = create_key(&router, "configured after the writes").await;
        enforce_encryption(&router, case, "SSE-KMS", &key_arn, false).await;
        KeyOutage::Disabled.begin(&router, &key_id).await;

        for (operation, request) in object_reads(database, table) {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 200,
                "case {case}: {operation} must still be served for an object written under no \
                 key, got {status} with {body}"
            );
        }
        let after = ok(
            &router,
            "GetTable",
            &json!({ "DatabaseName": database, "Name": table }),
        )
        .await;
        assert_eq!(
            after, before,
            "case {case}: the metadata must not change; expected {before}, got {after}"
        );
    }
}

/// Each object is answered through the key ITS OWN write used: moving the catalog's
/// setting to another key leaves what is already stored under the first one, and the write
/// that rewrites an object moves it onto the key in force at that moment.
#[tokio::test]
async fn an_object_is_read_through_the_key_its_own_write_used() {
    let router = emulator_with_kms();
    let (first_id, first_arn) = create_key(&router, "the catalog's first CMK").await;
    let (second_id, second_arn) = create_key(&router, "the catalog's second CMK").await;

    let database = "rotation_db";
    enforce_encryption(&router, "under the first key", "SSE-KMS", &first_arn, false).await;
    create_objects(&router, "under the first key", database, "first_tbl").await;

    enforce_encryption(&router, "under the second key", "SSE-KMS", &second_arn, false).await;
    let (status, body) = call(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": { "Name": "second_tbl" }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a table created under the second key must be served, got {status} with {body}"
    );

    // Revoking the FIRST key refuses the table written under it and leaves the other one
    // readable.
    KeyOutage::Disabled.begin(&router, &first_id).await;
    let (status, body) = call(
        &router,
        "GetTable",
        &json!({ "DatabaseName": database, "Name": "first_tbl" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case the first key revoked: the table written under it must be refused, got {status} \
         with {body}"
    );
    assert!(
        error_code(&body).contains("GlueEncryptionException"),
        "case the first key revoked: expected GlueEncryptionException, got {body}"
    );
    let second = ok(
        &router,
        "GetTable",
        &json!({ "DatabaseName": database, "Name": "second_tbl" }),
    )
    .await;
    assert_eq!(
        second["Table"]["Name"],
        json!("second_tbl"),
        "case the first key revoked: a table written under the second key must still be \
         served, got {second}"
    );

    // A write while the second key is in force moves the table onto it, which is what
    // makes it readable again while the first key is still revoked.
    let (status, body) = call(
        &router,
        "UpdateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": { "Name": "first_tbl", "Description": "rewritten under the second key" }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an update under the usable second key must be served, got {status} with {body}"
    );
    let rewritten = ok(
        &router,
        "GetTable",
        &json!({ "DatabaseName": database, "Name": "first_tbl" }),
    )
    .await;
    assert_eq!(
        rewritten["Table"]["Description"],
        json!("rewritten under the second key"),
        "a table rewritten under a usable key must be readable again, got {rewritten}"
    );

    // The version that update ARCHIVED is still the definition the FIRST write encrypted,
    // so it is refused while the first key is revoked even though the live definition is
    // served: a version is answered through the key its own write used.
    let current = ok(
        &router,
        "GetTableVersion",
        &json!({ "DatabaseName": database, "TableName": "first_tbl" }),
    )
    .await;
    assert_eq!(
        current["TableVersion"]["Table"]["Description"],
        json!("rewritten under the second key"),
        "the current version must be served through the key its own write used, got {current}"
    );
    let (status, body) = call(
        &router,
        "GetTableVersion",
        // `FIRST_TABLE_VERSION_ID`: the version the create stored, which the update
        // archived.
        &json!({ "DatabaseName": database, "TableName": "first_tbl", "VersionId": "0" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case the archived version: a version written under the revoked key must be \
         refused, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("GlueEncryptionException"),
        "case the archived version: expected GlueEncryptionException, got {body}"
    );

    // And the converse: revoking the SECOND key now refuses both, since both were last
    // written under it.
    KeyOutage::Disabled.begin(&router, &second_id).await;
    for table in ["first_tbl", "second_tbl"] {
        let (status, body) = call(
            &router,
            "GetTable",
            &json!({ "DatabaseName": database, "Name": table }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {table} after the second key was revoked: expected a refusal, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).contains("GlueEncryptionException"),
            "case {table} after the second key was revoked: expected \
             GlueEncryptionException, got {body}"
        );
    }
}

/// A mode that encrypts catalog writes names the key it encrypts them under, so a key KMS
/// cannot resolve is refused as invalid input and stores nothing; a mode that names no key
/// is not resolved at all.
#[tokio::test]
async fn a_security_configuration_is_refused_when_its_key_resolves_to_nothing() {
    let router = emulator_with_kms();
    let (key_id, key_arn) = create_key(&router, "the only key this account holds").await;

    // (case, the EncryptionAtRest the write carries, whether the write must be served)
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "the key named by id",
            json!({ "CatalogEncryptionMode": "SSE-KMS", "SseAwsKmsKeyId": key_id }),
            true,
        ),
        (
            "the key named by ARN",
            json!({ "CatalogEncryptionMode": "SSE-KMS", "SseAwsKmsKeyId": key_arn }),
            true,
        ),
        (
            "with a service role, the key named by ARN",
            json!({
                "CatalogEncryptionMode": "SSE-KMS-WITH-SERVICE-ROLE",
                "SseAwsKmsKeyId": key_arn,
                "CatalogEncryptionServiceRole": SERVICE_ROLE_ARN
            }),
            true,
        ),
        (
            "an id of the shape KMS mints, for a key nothing created",
            json!({
                "CatalogEncryptionMode": "SSE-KMS",
                "SseAwsKmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab"
            }),
            false,
        ),
        (
            "a key ARN of this account naming no key",
            json!({
                "CatalogEncryptionMode": "SSE-KMS",
                "SseAwsKmsKeyId":
                    "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab"
            }),
            false,
        ),
        (
            "a key ARN of another Region",
            json!({
                "CatalogEncryptionMode": "SSE-KMS",
                "SseAwsKmsKeyId":
                    "arn:aws:kms:eu-west-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab"
            }),
            false,
        ),
        (
            "an alias nothing created",
            json!({ "CatalogEncryptionMode": "SSE-KMS", "SseAwsKmsKeyId": "alias/not-created" }),
            false,
        ),
        (
            "a service-role mode whose key does not exist either",
            json!({
                "CatalogEncryptionMode": "SSE-KMS-WITH-SERVICE-ROLE",
                "SseAwsKmsKeyId": "alias/not-created",
                "CatalogEncryptionServiceRole": SERVICE_ROLE_ARN
            }),
            false,
        ),
        (
            // The mode that names no key: nothing is resolved, so even a value naming no
            // key is stored as sent.
            "encryption turned off, with a key that names nothing",
            json!({
                "CatalogEncryptionMode": "DISABLED",
                "SseAwsKmsKeyId": "alias/not-created"
            }),
            true,
        ),
    ];

    for (case, at_rest, served) in cases {
        // Each case starts from a known stored record, so a refused write can be shown to
        // have stored nothing.
        let baseline = json!({
            "DataCatalogEncryptionSettings": {
                "EncryptionAtRest": { "CatalogEncryptionMode": "DISABLED" }
            }
        });
        ok(&router, "PutDataCatalogEncryptionSettings", &baseline).await;

        let request = json!({ "DataCatalogEncryptionSettings": { "EncryptionAtRest": at_rest } });
        let (status, body) = call(&router, "PutDataCatalogEncryptionSettings", &request).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case}: the write must be served, got {status} with {body}"
            );
            let reported = ok(&router, "GetDataCatalogEncryptionSettings", &json!({})).await;
            assert_eq!(
                reported["DataCatalogEncryptionSettings"]["EncryptionAtRest"],
                request["DataCatalogEncryptionSettings"]["EncryptionAtRest"],
                "case {case}: the stored setting must read back as written, got {reported}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case}: a key KMS cannot resolve must be refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );
        let reported = ok(&router, "GetDataCatalogEncryptionSettings", &json!({})).await;
        assert_eq!(
            reported["DataCatalogEncryptionSettings"]["EncryptionAtRest"]
                ["CatalogEncryptionMode"],
            json!("DISABLED"),
            "case {case}: a refused write must store nothing, got {reported}"
        );
    }
}
