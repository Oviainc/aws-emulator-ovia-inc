// SPDX-License-Identifier: CC0-1.0

//! `GetMLTransform`: the one machine learning transform a `TransformId` addresses.
//!
//! RULE: a transform read reports the record stored under the id the create minted —
//! "Gets an Glue machine learning transform artifact and all its corresponding metadata" —
//! member for member the same projection the transform listing serves for it, and refuses
//! an id this account holds no transform under with the operation's modelled
//! `EntityNotFoundException` rather than answering an empty transform. Reading changes
//! nothing a later caller sees.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler. The create helpers,
//! the role and the transform type are the ones [`crate::ml_transforms`] already defines,
//! so the two suites register a transform the same way.
//!
//! The rows differ along what the rule generalises over: which members the create carried
//! (none beyond the required four, a worker configuration, a DPU figure of its own, an
//! explicit version and timeout, every optional member, and input tables the catalog holds
//! a schema for), and — for the refusal — the form of the id that addresses no transform.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    DEFAULT_ML_TRANSFORM_DPUS, DEFAULT_ML_TRANSFORM_GLUE_VERSION,
    DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES, TRANSFORM_STATUS_NOT_READY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ml_transforms::{given_transform, listed, store_table, ROLE, TRANSFORM_TYPE};
use crate::wire::{assert_echoed, call, emulator, error_code, error_message, ok};

/// One row of the read-back table: the case, the transform's name, the members the create
/// sends beside the required ones, and the members the read must then report.
type ReadRow<'a> = (&'a str, &'a str, Value, Value);

/// One row of the refusal table: the case and the `TransformId` the read addresses.
type MissRow<'a> = (&'a str, String);

/// Read one transform back by the id its create minted.
async fn get_transform(router: &MultiServiceRouter, transform_id: &str) -> Value {
    ok(
        router,
        "GetMLTransform",
        &json!({ "TransformId": transform_id }),
    )
    .await
}

/// The entry the transform listing reports for `transform_id`, which is the projection a
/// single read has to agree with.
async fn listed_transform(router: &MultiServiceRouter, case: &str, transform_id: &str) -> Value {
    let page = ok(router, "GetMLTransforms", &json!({})).await;
    listed(&page)
        .into_iter()
        .find(|transform| transform["TransformId"].as_str() == Some(transform_id))
        .unwrap_or_else(|| panic!("case {case:?}: {transform_id} must be listed, got {page}"))
}

#[tokio::test]
async fn a_created_transform_reads_back_under_the_id_the_create_minted() {
    let router = emulator();
    store_table(
        &router,
        "sales",
        "orders",
        &[("order_id", "bigint"), ("total", "double")],
    )
    .await;

    // (case, name, the members the create sends, the members the read must report)
    let rows: &[ReadRow<'_>] = &[
        (
            "required members only",
            "bare",
            Value::Null,
            // "If the value is not set, the Glue compatibility defaults to Glue 0.9"
            // (`CreateMLTransform$GlueVersion`); "the default is 10" DPUs (`$MaxCapacity`);
            // "The default is 2,880 minutes (48 hours)" (`$Timeout`).
            json!({
                "GlueVersion": DEFAULT_ML_TRANSFORM_GLUE_VERSION,
                "MaxCapacity": DEFAULT_ML_TRANSFORM_DPUS,
                "Timeout": DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES,
                "WorkerType": Value::Null,
                "NumberOfWorkers": Value::Null,
                "MaxRetries": Value::Null,
                "Description": Value::Null,
            }),
        ),
        (
            "a worker configuration",
            "workers",
            json!({ "WorkerType": "G.1X", "NumberOfWorkers": 5 }),
            // "When the WorkerType field is set to a value other than Standard, the
            // MaxCapacity field is set automatically": a `G.1X` worker "maps to 1 DPU"
            // (`CreateMLTransform$WorkerType`), so five of them are 5.
            json!({ "WorkerType": "G.1X", "NumberOfWorkers": 5, "MaxCapacity": 5.0 }),
        ),
        (
            "a DPU figure of its own",
            "capacity",
            json!({ "MaxCapacity": 48.0 }),
            json!({ "MaxCapacity": 48.0, "WorkerType": Value::Null }),
        ),
        (
            "an explicit version, timeout and retry count",
            "versioned",
            json!({ "GlueVersion": "2.0", "Timeout": 15, "MaxRetries": 3 }),
            json!({ "GlueVersion": "2.0", "Timeout": 15, "MaxRetries": 3 }),
        ),
        (
            "every optional member the response reports",
            "described",
            json!({
                "Description": "dedupes the order records",
                "Tags": { "owner": "data-platform" },
                "TransformEncryption": {
                    "MlUserDataEncryption": {
                        "MlUserDataEncryptionMode": "SSE-KMS",
                        "KmsKeyId": "alias/glue-ml",
                    },
                    "TaskRunSecurityConfigurationName": "ml-security",
                },
            }),
            json!({
                "Description": "dedupes the order records",
                "TransformEncryption": {
                    "MlUserDataEncryption": {
                        "MlUserDataEncryptionMode": "SSE-KMS",
                        "KmsKeyId": "alias/glue-ml",
                    },
                    "TaskRunSecurityConfigurationName": "ml-security",
                },
            }),
        ),
        (
            "input tables the catalog holds a schema for",
            "over-orders",
            json!({ "InputRecordTables": [{ "DatabaseName": "sales", "TableName": "orders" }] }),
            json!({
                "InputRecordTables": [{ "DatabaseName": "sales", "TableName": "orders" }],
                "Schema": [
                    { "Name": "order_id", "DataType": "bigint" },
                    { "Name": "total", "DataType": "double" },
                ],
            }),
        ),
    ];

    for (case, name, sent, expected) in rows {
        let id = given_transform(&router, name, sent.clone()).await;
        let read = get_transform(&router, &id).await;

        assert_eq!(
            read["TransformId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the transform addressed by {id}, got {read}"
        );
        assert_eq!(
            read["Name"].as_str(),
            Some(*name),
            "case {case:?}: expected the stored name {name}, got {read}"
        );
        assert_echoed(case, expected, &read);
        // The members the create could not set are the server's: the role it named comes
        // back, the status is the one a transform nothing has taught is in, no labelling
        // file has been generated and nothing has evaluated the transform.
        assert_eq!(
            read["Role"].as_str(),
            Some(ROLE),
            "case {case:?}: expected the stored role, got {read}"
        );
        assert_eq!(
            read["Parameters"]["TransformType"].as_str(),
            Some(TRANSFORM_TYPE),
            "case {case:?}: expected the stored parameters, got {read}"
        );
        assert_eq!(
            read["Status"].as_str(),
            Some(TRANSFORM_STATUS_NOT_READY),
            "case {case:?}: expected a transform nothing has taught to be \
             {TRANSFORM_STATUS_NOT_READY}, got {read}"
        );
        assert_eq!(
            read["LabelCount"].as_i64(),
            Some(0),
            "case {case:?}: expected no labelling files, got {read}"
        );
        assert!(
            read.get("EvaluationMetrics").is_none(),
            "case {case:?}: nothing has evaluated this transform, so the read must report no \
             EvaluationMetrics, got {read}"
        );
        // `Tags` are not part of the transform shape, so a read reports none even where the
        // create sent them.
        assert!(
            read.get("Tags").is_none(),
            "case {case:?}: a transform read must report no Tags, got {read}"
        );
        let created_on = read["CreatedOn"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: CreatedOn must be a timestamp, got {read}"));
        assert!(
            created_on > 0.0,
            "case {case:?}: expected a creation instant, got {created_on}"
        );
        assert_eq!(
            read["LastModifiedOn"].as_f64(),
            Some(created_on),
            "case {case:?}: nothing has modified this transform, so LastModifiedOn is its \
             creation instant, got {read}"
        );

        // One record, one projection: the single read and the listing report the same
        // transform member for member.
        let entry = listed_transform(&router, case, &id).await;
        assert_eq!(
            read, entry,
            "case {case:?}: a single read and the listing must report the same transform"
        );

        // Reading is reporting only: two reads in a row agree, so the read changed nothing
        // a later caller sees.
        let again = get_transform(&router, &id).await;
        assert_eq!(
            again, read,
            "case {case:?}: read twice, a read must change nothing"
        );
    }
}

#[tokio::test]
async fn an_id_this_account_holds_no_transform_under_is_refused() {
    let router = emulator();
    let stored = given_transform(&router, "stored", Value::Null).await;

    // (case, the TransformId the read addresses)
    let rows: &[MissRow<'_>] = &[
        (
            "an id nothing has minted",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        (
            // "Names are not guaranteed unique and can be changed at any time"
            // (`MLTransform$Name`), so a name addresses no transform.
            "the stored transform's name used as an id",
            "stored".to_string(),
        ),
        (
            "the stored id with one character changed",
            format!("{}x", &stored[..stored.len() - 1]),
        ),
        ("the stored id in upper case", stored.to_uppercase()),
        (
            // The longest `TransformId` the model accepts (`HashString`, length 1..255),
            // so the refusal is the answer at the published bound too.
            "the longest id the model accepts",
            "z".repeat(255),
        ),
        ("the shortest id the model accepts", "z".to_string()),
    ];

    for (case, transform_id) in rows {
        let (status, body) = call(
            &router,
            "GetMLTransform",
            &json!({ "TransformId": transform_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled EntityNotFoundException's 400, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {}",
            error_code(&body)
        );
        // The sentence follows this crate's per-resource template for a resource addressed
        // by a minted id, and its one variable part is the id the request itself sent.
        assert_eq!(
            error_message(&body),
            format!("Machine learning transform with id {transform_id} does not exist"),
            "case {case:?}: expected the refusal to name the requested id, got {body}"
        );
    }

    // A refused read stored nothing: the one transform this account holds still reads back.
    let read = get_transform(&router, &stored).await;
    assert_eq!(
        read["TransformId"].as_str(),
        Some(stored.as_str()),
        "case a refused read: the stored transform must still read back, got {read}"
    );
    assert_eq!(
        listed(&ok(&router, "GetMLTransforms", &json!({})).await).len(),
        1,
        "case a refused read: a missed read must register no transform"
    );
}

#[tokio::test]
async fn each_id_addresses_its_own_transform() {
    let router = emulator();
    // Every transform is addressed by the id its own create minted, so a read reports the
    // definition that create stored and never a sibling's.
    let first = given_transform(&router, "first-transform", json!({ "Timeout": 10 })).await;
    let second = given_transform(&router, "second-transform", json!({ "Timeout": 20 })).await;
    assert_ne!(
        first, second,
        "case two transforms: each create must mint its own TransformId, got {first} twice"
    );

    for (case, id, name, timeout) in [
        ("the first transform", &first, "first-transform", 10),
        ("the second transform", &second, "second-transform", 20),
    ] {
        let read = get_transform(&router, id).await;
        assert_eq!(
            read["TransformId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the transform addressed by {id}, got {read}"
        );
        assert_eq!(
            read["Name"].as_str(),
            Some(name),
            "case {case:?}: expected the name its own create stored, got {read}"
        );
        assert_eq!(
            read["Timeout"].as_i64(),
            Some(timeout),
            "case {case:?}: expected the timeout its own create stored, got {read}"
        );
    }
}
