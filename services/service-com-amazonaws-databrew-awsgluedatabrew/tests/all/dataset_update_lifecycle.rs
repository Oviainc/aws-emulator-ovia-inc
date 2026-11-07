// SPDX-License-Identifier: CC0-1.0

//! The dataset lifecycle a client drives: `CreateDataset` -> `UpdateDataset` ->
//! `DescribeDataset`/`ListDatasets`, over the real wire path.
//!
//! Two rules are pinned here, both properties of the stored record rather than of one
//! request:
//!
//! 1. A resource's `LastModifiedDate`/`LastModifiedBy` report an actual modification:
//!    both members are ABSENT from every response that carries them until an
//!    operation that mutates the resource has run, and reading the resource never
//!    sets them. The model makes both optional (`smithy_query(query="operation",
//!    operation="DescribeDataset")`: only `Name` and `Input` are required).
//! 2. `UpdateDataset` is the PUT that carries a dataset's whole definition
//!    (`smithy_query(query="operation", operation="UpdateDataset")`: `PUT
//!    /datasets/{Name}`, `Input` required, `Format`/`FormatOptions`/`PathOptions`
//!    optional), so each definition member it carries REPLACES the stored one, an
//!    optional member it omits is cleared, and the next read serves what it wrote.
//!
//! The table spans the definition shapes the rule generalises over — every
//! `InputFormat` the model declares, each `FormatOptions` variant (`Csv`, `Excel`,
//! `Json`), `PathOptions` present and absent, and every `Input` definition (so every
//! `Source`) — rather than the one recorded CSV request.

use serde_json::{json, Value};

use crate::databrew_harness::{create_dataset, label, s3_input, Emulator, CALLER_PRINCIPAL};

/// The definition a dataset is created with in this suite: a CSV dataset whose
/// options are deliberately NOT what any update below writes, so a read that still
/// serves these values is a lost update.
fn created_definition(name: &str) -> Value {
    json!({
        "Name": name,
        "Input": s3_input("in/created.csv"),
        "Format": "CSV",
        "FormatOptions": {"Csv": {"Delimiter": ",", "HeaderRow": true}},
        "PathOptions": {"FilesLimit": {"MaxFiles": 1}},
    })
}

/// One row of the update table: the definition `UpdateDataset` carries and the
/// `Source` the request's `Input` implies.
struct UpdateCase {
    case: &'static str,
    body: Value,
    source: &'static str,
}

/// Every update the table drives. `Format` spans every value of the modelled
/// `InputFormat` enum (`smithy_query(query="shape", shape="InputFormat")`: CSV, JSON,
/// PARQUET, EXCEL, ORC), `FormatOptions` every variant the shape declares, and `Input`
/// every definition it can carry (`smithy_query(query="shape", shape="Input")`).
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            // The recorded instance: a pipe delimiter and no header row.
            case: "csv, pipe delimiter, no header row",
            body: json!({
                "Input": s3_input("in/piped.csv"),
                "Format": "CSV",
                "FormatOptions": {"Csv": {"Delimiter": "|", "HeaderRow": false}},
            }),
            source: "S3",
        },
        UpdateCase {
            case: "csv, semicolon delimiter, header row",
            body: json!({
                "Input": s3_input("in/semi.csv"),
                "Format": "CSV",
                "FormatOptions": {"Csv": {"Delimiter": ";", "HeaderRow": true}},
            }),
            source: "S3",
        },
        UpdateCase {
            case: "csv, tab delimiter, path options kept",
            body: json!({
                "Input": s3_input("in/tabbed.tsv"),
                "Format": "CSV",
                "FormatOptions": {"Csv": {"Delimiter": "\t", "HeaderRow": false}},
                "PathOptions": {"FilesLimit": {"MaxFiles": 7}},
            }),
            source: "S3",
        },
        UpdateCase {
            case: "excel, one named sheet",
            body: json!({
                "Input": s3_input("in/book.xlsx"),
                "Format": "EXCEL",
                "FormatOptions": {"Excel": {"SheetNames": ["Q1"], "HeaderRow": true}},
            }),
            source: "S3",
        },
        UpdateCase {
            case: "json, multi-line",
            body: json!({
                "Input": s3_input("in/events.json"),
                "Format": "JSON",
                "FormatOptions": {"Json": {"MultiLine": true}},
            }),
            source: "S3",
        },
        UpdateCase {
            case: "parquet, no format options",
            body: json!({
                "Input": s3_input("in/part-0.parquet"),
                "Format": "PARQUET",
            }),
            source: "S3",
        },
        UpdateCase {
            case: "orc, no format options",
            body: json!({
                "Input": s3_input("in/part-0.orc"),
                "Format": "ORC",
            }),
            source: "S3",
        },
        UpdateCase {
            case: "data catalog input, no format at all",
            body: json!({
                "Input": {
                    "DataCatalogInputDefinition": {"DatabaseName": "sales", "TableName": "orders"},
                },
            }),
            source: "DATA-CATALOG",
        },
        UpdateCase {
            case: "database input, csv options",
            body: json!({
                "Input": {
                    "DatabaseInputDefinition": {
                        "GlueConnectionName": "conn",
                        "DatabaseTableName": "orders",
                    },
                },
                "Format": "CSV",
                "FormatOptions": {"Csv": {"Delimiter": ":", "HeaderRow": false}},
            }),
            source: "DATABASE",
        },
    ]
}

/// The `Dataset` entry `ListDatasets` reports for `name`, or `Value::Null`.
async fn listed(emulator: &Emulator, name: &str) -> Value {
    let listed = emulator.call("GET", "/datasets", None).await;
    let entries = listed.body["Datasets"].as_array().cloned().unwrap_or_default();
    entries
        .into_iter()
        .find(|entry| entry["Name"] == json!(name))
        .unwrap_or(Value::Null)
}

/// Rule 2: every definition member `UpdateDataset` carries replaces the stored one,
/// an optional member it omits is cleared, `Source` follows the new `Input`, and the
/// next `DescribeDataset`/`ListDatasets` serves exactly that.
#[tokio::test]
async fn update_dataset_replaces_the_definition_the_next_read_serves() {
    for case in update_cases() {
        let UpdateCase { case, body, source } = case;

        let emulator = Emulator::new();
        let name = "lifecycle ds";
        let created = emulator
            .call("POST", "/datasets", Some(created_definition(name)))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateDataset to answer 200, got {} {:?}",
            created.status, created.body
        );
        let before = emulator
            .call("GET", &format!("/datasets/{}", label(name)), None)
            .await;

        let updated = emulator
            .call(
                "PUT",
                &format!("/datasets/{}", label(name)),
                Some(body.clone()),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateDataset to answer 200, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.text("Name"),
            name,
            "case {case:?}: UpdateDataset must echo the name it updated, got {:?}",
            updated.body
        );

        let described = emulator
            .call("GET", &format!("/datasets/{}", label(name)), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeDataset to answer 200, got {} {:?}",
            described.status, described.body
        );

        // Every definition member the update carried, read back; one it omitted is
        // cleared rather than kept from create time.
        for member in ["Input", "Format", "FormatOptions", "PathOptions"] {
            let expected = body.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                described.member(member),
                expected,
                "case {case:?}: expected {member} {expected:?} after the update, got {:?}",
                described.member(member)
            );
        }
        assert_eq!(
            described.text("Source"),
            source,
            "case {case:?}: expected Source {source} for the updated Input, got {:?}",
            described.body
        );

        // The members the update is not allowed to touch are untouched.
        for member in ["Name", "ResourceArn", "CreateDate", "CreatedBy"] {
            assert_eq!(
                described.member(member),
                before.member(member),
                "case {case:?}: {member} must survive the update unchanged, was {:?}, got {:?}",
                before.member(member),
                described.member(member)
            );
        }

        // `ListDatasets` reads the same record, so it reports the same definition.
        let entry = listed(&emulator, name).await;
        for member in ["Input", "Format", "FormatOptions", "PathOptions"] {
            let expected = body.get(member).cloned().unwrap_or(Value::Null);
            let actual = entry.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the listed {member} to match the update \
                 ({expected:?}), got {actual:?}"
            );
        }
    }
}

/// Rule 1: `LastModifiedDate`/`LastModifiedBy` are absent until a mutating operation
/// runs, for every resource that reports them, and a read never sets them.
#[tokio::test]
async fn the_last_modified_pair_is_absent_until_a_mutation_runs() {
    let emulator = Emulator::new();
    let name = "fresh-ds";
    assert_eq!(
        create_dataset(&emulator, name, s3_input("in/data.csv"))
            .await
            .status,
        200,
        "the dataset must be created"
    );

    // Case "never updated dataset": described twice, so a read cannot be what sets
    // the pair.
    for read in ["first read", "second read"] {
        let described = emulator
            .call("GET", &format!("/datasets/{name}"), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {read:?}: expected DescribeDataset to answer 200, got {} {:?}",
            described.status, described.body
        );
        for member in ["LastModifiedDate", "LastModifiedBy"] {
            assert!(
                described.is_absent(member),
                "case {read:?}: a never-updated dataset must report no {member}, got {:?}",
                described.member(member)
            );
        }
        assert!(
            described.body["CreateDate"].as_f64().unwrap_or_default() > 0.0,
            "case {read:?}: the create pair is still reported, got {:?}",
            described.member("CreateDate")
        );
    }

    // Case "never updated dataset, listed": the same record, the same absence.
    let entry = listed(&emulator, name).await;
    for member in ["LastModifiedDate", "LastModifiedBy"] {
        assert!(
            entry.get(member).is_none(),
            "case \"never updated dataset, listed\": expected no {member}, got {:?}",
            entry.get(member)
        );
    }

    // Case "never updated job": a job carries the same pair and has had no update
    // either (`UpdateRecipeJob`/`UpdateProfileJob` are still their own stub issues).
    assert_eq!(
        emulator
            .call(
                "POST",
                "/recipeJobs",
                Some(json!({
                    "Name": "fresh-job",
                    "RoleArn": "arn:aws:iam::000000000000:role/brew",
                    "DatasetName": name,
                    "Outputs": [{"Location": {"Bucket": "brew-bucket", "Key": "out/"}}],
                })),
            )
            .await
            .status,
        200,
        "the job must be created"
    );
    let job = emulator.call("GET", "/jobs/fresh-job", None).await;
    assert_eq!(
        job.status, 200,
        "case \"never updated job\": expected DescribeJob to answer 200, got {} {:?}",
        job.status, job.body
    );
    for member in ["LastModifiedDate", "LastModifiedBy"] {
        assert!(
            job.is_absent(member),
            "case \"never updated job\": a never-updated job must report no {member}, got {:?}",
            job.member(member)
        );
    }

    // Case "updated dataset": the update is what sets the pair, and it names the
    // calling principal and an instant at or after the create.
    let updated = emulator
        .call(
            "PUT",
            &format!("/datasets/{name}"),
            Some(json!({"Input": s3_input("in/data.csv"), "Format": "CSV"})),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "case \"updated dataset\": expected UpdateDataset to answer 200, got {} {:?}",
        updated.status, updated.body
    );
    let described = emulator
        .call("GET", &format!("/datasets/{name}"), None)
        .await;
    let create_date = described.body["CreateDate"].as_f64().unwrap_or_default();
    let modified_date = described.body["LastModifiedDate"]
        .as_f64()
        .unwrap_or_default();
    assert!(
        modified_date >= create_date && create_date > 0.0,
        "case \"updated dataset\": expected LastModifiedDate at or after CreateDate \
         ({create_date}), got {modified_date}"
    );
    assert_eq!(
        described.text("LastModifiedBy"),
        CALLER_PRINCIPAL,
        "case \"updated dataset\": expected LastModifiedBy to name the calling \
         principal, got {:?}",
        described.member("LastModifiedBy")
    );

    // And the listed record agrees with the described one.
    let entry = listed(&emulator, name).await;
    for member in ["LastModifiedDate", "LastModifiedBy"] {
        assert_eq!(
            entry.get(member).cloned().unwrap_or(Value::Null),
            described.member(member),
            "case \"updated dataset, listed\": expected the listed {member} to match \
             the described one ({:?}), got {:?}",
            described.member(member),
            entry.get(member)
        );
    }

    // Case "read after the update": a read still changes nothing.
    let again = emulator
        .call("GET", &format!("/datasets/{name}"), None)
        .await;
    assert_eq!(
        again.member("LastModifiedDate"),
        described.member("LastModifiedDate"),
        "case \"read after the update\": a read must not move LastModifiedDate, was \
         {:?}, got {:?}",
        described.member("LastModifiedDate"),
        again.member("LastModifiedDate")
    );
}

/// `CreatedBy`/`LastModifiedBy` name the identity the request speaks for: the IAM user
/// ARN this emulator publishes for its long-term credentials (the one STS answers
/// `GetCallerIdentity` with for a key that names no session). An access key id is a
/// credential id, not a user name, so it never appears in the ARN — which makes the
/// reported principal the same whatever key signs the call.
#[tokio::test]
async fn the_reported_principal_is_the_emulator_identity_whatever_key_signs_the_call() {
    // Rows differ in the access key id presented: a long-term key, an SDK example
    // key, and a session-style key.
    let keys = [
        "AKIDEXAMPLE",
        "AKIAIOSFODNN7EXAMPLE",
        "ASIAEXAMPLESESSION01",
    ];

    let emulator = Emulator::new();
    for (index, key) in keys.iter().enumerate() {
        let name = format!("principal-ds-{index}");
        let created = emulator
            .call_as(
                key,
                "POST",
                "/datasets",
                Some(created_definition(&name)),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {key:?}: expected CreateDataset to answer 200, got {} {:?}",
            created.status, created.body
        );
        let updated = emulator
            .call_as(
                key,
                "PUT",
                &format!("/datasets/{name}"),
                Some(json!({"Input": s3_input("in/data.csv"), "Format": "CSV"})),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {key:?}: expected UpdateDataset to answer 200, got {} {:?}",
            updated.status, updated.body
        );

        let described = emulator.call("GET", &format!("/datasets/{name}"), None).await;
        for member in ["CreatedBy", "LastModifiedBy"] {
            assert_eq!(
                described.text(member),
                CALLER_PRINCIPAL,
                "case {key:?}: expected {member} to be the emulator's identity, got {:?}",
                described.member(member)
            );
        }
        assert!(
            !described.body.to_string().contains(key),
            "case {key:?}: no access key id may appear in the described dataset, got {:?}",
            described.body
        );
    }
}

// The message a lookup miss answers with is a property of the error rather than of
// the dataset lifecycle, so it is pinned across every operation and resource kind in
// `tests/all/resource_not_found_message.rs`.

/// An update this service cannot apply changes nothing: the stored definition and the
/// last-modified pair are exactly what they were before the refused call.
#[tokio::test]
async fn a_refused_update_leaves_the_stored_dataset_alone() {
    let emulator = Emulator::new();
    let name = "guarded-ds";
    assert_eq!(
        emulator
            .call("POST", "/datasets", Some(created_definition(name)))
            .await
            .status,
        200,
        "the dataset must be created"
    );
    let before = emulator
        .call("GET", &format!("/datasets/{name}"), None)
        .await;

    // (case, the name the update addresses, request body, expected status, code)
    let refusals = [
        (
            "Input carrying no definition",
            name,
            json!({"Input": {}, "Format": "CSV"}),
            400,
            "ValidationException",
        ),
        (
            // The documented character set is `CreateDataset`'s rule for the name it
            // mints, so an update addressing such a name is answered by the lookup,
            // not by a validation error: no dataset can hold it.
            "name outside the documented character set",
            "bad_name",
            json!({"Input": s3_input("in/other.csv")}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "no dataset of that name",
            "absent-ds",
            json!({"Input": s3_input("in/other.csv")}),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, target, body, status, code) in refusals {
        let refused = emulator
            .call("PUT", &format!("/datasets/{target}"), Some(body))
            .await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected {status}, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some(code),
            "case {case:?}: expected {code}, got {:?}",
            refused.error_type
        );
    }

    // Nothing the refusals named was created, and the stored dataset is untouched.
    for absent in ["bad_name", "absent-ds"] {
        let described = emulator
            .call("GET", &format!("/datasets/{absent}"), None)
            .await;
        assert_eq!(
            described.status, 404,
            "case {absent:?}: a refused update must create nothing, got {} {:?}",
            described.status, described.body
        );
    }
    let after = emulator
        .call("GET", &format!("/datasets/{name}"), None)
        .await;
    for member in ["Input", "Format", "FormatOptions", "PathOptions", "Source"] {
        assert_eq!(
            after.member(member),
            before.member(member),
            "case \"refused update\": {member} must be unchanged, was {:?}, got {:?}",
            before.member(member),
            after.member(member)
        );
    }
    for member in ["LastModifiedDate", "LastModifiedBy"] {
        assert!(
            after.is_absent(member),
            "case \"refused update\": a refused update must not stamp {member}, got {:?}",
            after.member(member)
        );
    }
}
