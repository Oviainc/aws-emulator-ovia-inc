// SPDX-License-Identifier: CC0-1.0

//! The resources of OTHER SERVICES a submitted export definition names, resolved against the
//! services that hold them, over the awsJson1_1 wire with Amazon S3 and AWS Billing beside
//! this service on one engine.
//!
//! The rule under test: an operation that accepts an export definition resolves every
//! resource of another service the definition names — the destination bucket and the account
//! stated as its owner ("The name of the Amazon S3 bucket used as the destination of a data
//! export file" / "The Amazon Web Services account ID that owns the S3 bucket used as the
//! destination for the data export", the modelled docs for `S3Destination$S3Bucket` and
//! `$S3BucketOwner`), and the billing view a table's `BILLING_VIEW_ARN` table property scopes
//! its data to — against the service that OWNS it, before anything is persisted, and answers
//! the modelled `ValidationException` (HTTP 400) naming the member at fault when that service
//! reports the resource is not there. Both operations that accept a definition are judged the
//! same way, `UpdateExport` because it "[updates] an existing data export by overwriting all
//! export parameters", and a refused call persists nothing: the stored definition of an
//! update, and the absence of an export for a create, survive it.
//!
//! Why a client cares: an export is delivered by the SERVICE, on the cadence the definition
//! states. A create that succeeded against a destination that does not exist, or that is
//! scoped to a billing view that does not exist, would leave the client holding an ARN for an
//! export that silently never delivers, with no error branch ever taken and nothing to retry.
//!
//! The rows differ along what the rule generalises over: WHICH referenced resource the
//! definition names (the bucket, its owner, the billing view) and which service owns it;
//! whether that resource was ever created; the bucket name's length (Amazon S3 documents
//! 3..=63 characters) and form (hyphens, dots, digits), including a name that merely HAS an
//! existing bucket's name as a prefix — so the resolution is exact-name, not prefix, matching
//! — and the empty name the modelled `@length(0, 1024)` admits; the `S3Prefix` the export is
//! delivered under; the table whose property names the billing view; and the operation the
//! definition is submitted to. The recorded single absent-bucket case is one row.
//!
//! Every expectation about what the owning service holds is read from THAT SERVICE: a bucket
//! exists because Amazon S3's own `CreateBucket` made it, and the billing view a row states
//! is the one AWS Billing's own `ListBillingViews` reports — so the suite asserts the two
//! readings AGREE rather than restating either service's records here.

use serde_json::{json, Map, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::BUCKET_PERMISSION_VALIDATION_FAILED;

use crate::delivery_grant::{published_grant, Deliveries};
use crate::export_lifecycle::{error_code, PINNED_REGION};

/// The modelled input member a refused destination BUCKET names
/// (`Export$DestinationConfigurations` -> `DestinationConfigurations$S3Destination` ->
/// `S3Destination$S3Bucket`). Written out here rather than read from the service, so a change
/// to the implementation's own constant makes this assertion fail.
const S3_BUCKET_FIELD: &str = "Export.DestinationConfigurations.S3Destination.S3Bucket";

/// The modelled input member a refusal of the stated bucket OWNER names
/// (`S3Destination$S3BucketOwner`).
const S3_BUCKET_OWNER_FIELD: &str =
    "Export.DestinationConfigurations.S3Destination.S3BucketOwner";

/// The modelled input member a refused table-property value names
/// (`DataQuery$TableConfigurations`).
const TABLE_CONFIGURATIONS_FIELD: &str = "Export.DataQuery.TableConfigurations";

/// The `ValidationExceptionReason` a refusal of one named input member reports — one of the
/// modelled values (smithy_query shape `ValidationExceptionReason`).
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The table property that names the billing view a table's data is scoped to, as the
/// catalog publishes it (`ListTables` reports it among `COST_AND_USAGE_REPORT`'s and
/// `FOCUS_1_0_AWS`'s `TableProperties`).
const BILLING_VIEW_ARN: &str = "BILLING_VIEW_ARN";

/// The `BillingViewType` of the view every account holds, as AWS Billing reports it.
const PRIMARY: &str = "PRIMARY";

/// A `CreateExport` request for `name`, reading `table`, delivered to `bucket` under
/// `prefix`, with the bucket owner and the table configuration a row states.
fn create_request(
    name: &str,
    table: &str,
    bucket: &str,
    prefix: &str,
    owner: Option<&str>,
    table_configurations: Option<Value>,
) -> Value {
    let mut s3 = Map::new();
    s3.insert("S3Bucket".to_string(), json!(bucket));
    if let Some(owner) = owner {
        s3.insert("S3BucketOwner".to_string(), json!(owner));
    }
    s3.insert("S3Prefix".to_string(), json!(prefix));
    s3.insert("S3Region".to_string(), json!(PINNED_REGION));
    s3.insert(
        "S3OutputConfigurations".to_string(),
        json!({
            "OutputType": "CUSTOM",
            "Format": "TEXT_OR_CSV",
            "Compression": "GZIP",
            "Overwrite": "CREATE_NEW_REPORT",
        }),
    );
    let mut data_query = Map::new();
    data_query.insert(
        "QueryStatement".to_string(),
        json!(format!("SELECT {} FROM {table}", anchor_column(table))),
    );
    if let Some(configurations) = table_configurations {
        data_query.insert("TableConfigurations".to_string(), configurations);
    }
    json!({
        "Export": {
            "Name": name,
            "DataQuery": Value::Object(data_query),
            "DestinationConfigurations": { "S3Destination": Value::Object(s3) },
            "RefreshCadence": { "Frequency": "SYNCHRONOUS" },
        },
    })
}

/// A column the row's table publishes, which carries the statement while the row varies
/// something else entirely. Each is one the service's own `GetTable` reports for that table
/// (asserted in `tests/all/catalog_schema.rs`).
fn anchor_column(table: &str) -> &'static str {
    match table {
        "FOCUS_1_0_AWS" => "BilledCost",
        _ => "identity_line_item_id",
    }
}

/// Assert `body` is the modelled refusal of a referenced resource the owning service reports
/// is not there: HTTP 400, `ValidationException`, the recorded sentence for the destination
/// check or the request-built sentence for a billing view, the field-failure reason, and
/// `field` as the one named member.
fn assert_refused(case: &str, status: u16, body: &Value, field: &str, message: &str) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(message),
        "case {case:?}: expected the refusal's own sentence, got {body}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some(FIELD_VALIDATION_FAILED),
        "case {case:?}: expected the field-failure reason, got {body}"
    );
    let named: Vec<&str> = body["Fields"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the refusal must name a field, got {body}"))
        .iter()
        .filter_map(|entry| entry["Name"].as_str())
        .collect();
    assert_eq!(
        named,
        vec![field],
        "case {case:?}: expected the refusal to name {field}, got {named:?}"
    );
}

/// How many exports the store holds, so a refused call can be shown to have persisted
/// nothing.
async fn export_count(emulator: &Deliveries) -> usize {
    let listed = emulator.ok("ListExports", &json!({})).await;
    listed["Exports"].as_array().map(Vec::len).unwrap_or_default()
}

/// The ARN of the account's PRIMARY billing view, as AWS BILLING itself reports it — the
/// value this service's `BILLING_VIEW_ARN` property defaults to, read from the service that
/// owns the view rather than restated here.
async fn primary_billing_view(emulator: &Deliveries) -> String {
    let (status, listed) = emulator.billing("ListBillingViews", &json!({})).await;
    assert_eq!(
        status, 200,
        "ListBillingViews must report the account's views, got {status} with {listed}"
    );
    let views = listed["billingViews"]
        .as_array()
        .unwrap_or_else(|| panic!("ListBillingViews reports a list, got {listed}"));
    views
        .iter()
        .find(|view| view["billingViewType"].as_str() == Some(PRIMARY))
        .and_then(|view| view["arn"].as_str())
        .unwrap_or_else(|| panic!("every account holds a {PRIMARY} billing view, got {listed}"))
        .to_string()
}

/// One row of the destination-bucket table.
struct BucketRow {
    label: &'static str,
    /// The bucket the definition is delivered to.
    bucket: &'static str,
    /// The `S3Prefix` it is delivered under.
    prefix: &'static str,
    /// Whether the bucket is created in Amazon S3 (with the published delivery grant on it)
    /// before the create is made.
    created: bool,
}

/// The rows: bucket names of different lengths and forms, created and not, plus the two
/// prefix-collision rows and the empty name the model admits.
fn bucket_rows() -> Vec<BucketRow> {
    vec![
        BucketRow {
            label: "a bucket Amazon S3 holds",
            bucket: "referenced-destination",
            prefix: "exports/daily",
            created: true,
        },
        BucketRow {
            label: "the shortest bucket name Amazon S3 admits, held",
            bucket: "abc",
            prefix: "",
            created: true,
        },
        BucketRow {
            label: "a dotted, digit-carrying bucket name, held",
            bucket: "referenced.destination.2026",
            prefix: "exports/deep/prefix",
            created: true,
        },
        BucketRow {
            label: "the longest bucket name Amazon S3 admits, held",
            bucket: "referenced-destination-with-a-name-sixty-three-characters-longx",
            prefix: "exports/daily",
            created: true,
        },
        BucketRow {
            label: "a bucket that was never created",
            bucket: "referenced-destination-absent",
            prefix: "exports/daily",
            created: false,
        },
        BucketRow {
            label: "a name that HAS a held bucket's name as a prefix",
            bucket: "referenced-destination-extra",
            prefix: "exports/daily",
            created: false,
        },
        BucketRow {
            label: "a name that IS a prefix of a held bucket's name",
            bucket: "referenced",
            prefix: "exports/daily",
            created: false,
        },
        BucketRow {
            label: "the dotted name's prefix, never created",
            bucket: "referenced.destination",
            prefix: "exports/daily",
            created: false,
        },
        BucketRow {
            label: "the empty name the model admits",
            bucket: "",
            prefix: "exports/daily",
            created: false,
        },
    ]
}

/// RULE: a create is served exactly when Amazon S3 reports it holds the destination bucket,
/// and a refused create persists nothing.
#[tokio::test]
async fn a_create_resolves_its_destination_bucket_against_the_object_store() {
    let emulator = Deliveries::new();
    let rows = bucket_rows();
    // Every bucket a row holds is created first, so the two prefix-collision rows are asked
    // about names that are neighbours of a bucket that really is there.
    for row in rows.iter().filter(|row| row.created) {
        emulator
            .bucket_with(row.bucket, Some(&published_grant(row.bucket)))
            .await;
    }

    let mut stored = 0;
    for (index, row) in rows.iter().enumerate() {
        let case = row.label;
        let name = format!("referenced_bucket_{index}");
        let request = create_request(
            &name,
            "COST_AND_USAGE_REPORT",
            row.bucket,
            row.prefix,
            None,
            None,
        );
        let (status, body) = emulator.call("CreateExport", &request).await;
        if row.created {
            assert_eq!(
                status, 200,
                "case {case:?}: a destination Amazon S3 holds must be served, got {status} with \
                 {body}"
            );
            stored += 1;
            let arn = body["ExportArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a served create returns an ExportArn"))
                .to_string();
            let fetched = emulator.ok("GetExport", &json!({ "ExportArn": arn })).await;
            assert_eq!(
                fetched["Export"]["DestinationConfigurations"]["S3Destination"]["S3Bucket"]
                    .as_str(),
                Some(row.bucket),
                "case {case:?}: the stored destination must be the accepted one, got {fetched}"
            );
        } else {
            assert_refused(
                case,
                status,
                &body,
                S3_BUCKET_FIELD,
                BUCKET_PERMISSION_VALIDATION_FAILED,
            );
        }
        assert_eq!(
            export_count(&emulator).await,
            stored,
            "case {case:?}: only a served create may leave an export behind"
        );
    }
}

/// RULE, the owner half: a destination may state the account that owns its bucket, and the
/// create is served exactly when Amazon S3 reports that account IS the owner — "if the
/// account ID that you provide does not match the actual owner of the bucket, the request
/// fails" (the modelled doc for the expected bucket owner the question carries).
#[tokio::test]
async fn a_create_resolves_the_stated_bucket_owner_against_the_object_store() {
    let emulator = Deliveries::new();
    let bucket = "referenced-owner";
    emulator
        .bucket_with(bucket, Some(&published_grant(bucket)))
        .await;
    let owner = ::engine::emulator_account_id().to_string();
    // Another account id of the modelled form (`@pattern ^[0-9]{12}$`), built by moving one
    // digit of the owner's own id, so the only thing that differs is WHICH account it names.
    let other: String = owner
        .chars()
        .map(|digit| if digit == '1' { '2' } else { '1' })
        .collect();
    assert_ne!(
        other, owner,
        "the row must name an account other than the bucket's owner"
    );

    for (index, (case, stated, served)) in [
        ("no owner stated at all", None, true),
        ("the account that owns the bucket", Some(owner.as_str()), true),
        ("an account that owns no such bucket", Some(other.as_str()), false),
    ]
    .into_iter()
    .enumerate()
    {
        let request = create_request(
            &format!("referenced_owner_{index}"),
            "COST_AND_USAGE_REPORT",
            bucket,
            "exports/daily",
            stated,
            None,
        );
        let (status, body) = emulator.call("CreateExport", &request).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to be served, got {status} with {body}"
            );
        } else {
            assert_refused(
                case,
                status,
                &body,
                S3_BUCKET_OWNER_FIELD,
                BUCKET_PERMISSION_VALIDATION_FAILED,
            );
        }
    }
}

/// RULE, the billing-view half: a `BILLING_VIEW_ARN` table property may name a billing view
/// exactly when AWS Billing reports the account holds it, whichever table publishes the
/// property — the value is free-form, so nothing but the owning service can say whether it
/// names a view at all.
#[tokio::test]
async fn a_create_resolves_its_billing_view_against_aws_billing() {
    let emulator = Deliveries::new();
    let bucket = "referenced-billing-view";
    emulator
        .bucket_with(bucket, Some(&published_grant(bucket)))
        .await;
    let primary = primary_billing_view(&emulator).await;
    // An ARN of this account in the form AWS Billing mints one in, naming a view nobody ever
    // created: built from the view AWS Billing reported, with only the resource id replaced.
    let absent = primary.replace("billingview/", "billingview/custom-never-created-");
    assert_ne!(
        absent, primary,
        "the row must name a view other than the one AWS Billing reported"
    );

    let mut index = 0;
    for table in ["COST_AND_USAGE_REPORT", "FOCUS_1_0_AWS"] {
        for (case, stated, served) in [
            ("the account's own primary view", primary.as_str(), true),
            ("a view of this account that was never created", absent.as_str(), false),
            ("a value that is no ARN at all", "not-a-billing-view", false),
        ] {
            index += 1;
            let case = format!("{table} scoped to {case}");
            let request = create_request(
                &format!("referenced_view_{index}"),
                table,
                bucket,
                "exports/daily",
                None,
                Some(json!({ table: { BILLING_VIEW_ARN: stated } })),
            );
            let (status, body) = emulator.call("CreateExport", &request).await;
            if served {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a view AWS Billing holds must be served, got {status} with \
                     {body}"
                );
                let arn = body["ExportArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a served create returns an ARN"))
                    .to_string();
                let fetched = emulator.ok("GetExport", &json!({ "ExportArn": arn })).await;
                assert_eq!(
                    fetched["Export"]["DataQuery"]["TableConfigurations"][table][BILLING_VIEW_ARN]
                        .as_str(),
                    Some(stated),
                    "case {case:?}: the stored configuration must be the accepted one, got \
                     {fetched}"
                );
            } else {
                assert_refused(
                    &case,
                    status,
                    &body,
                    TABLE_CONFIGURATIONS_FIELD,
                    &format!("Billing view {stated} not found"),
                );
            }
        }
    }
}

/// RULE for `UpdateExport`, which "[updates] an existing data export by overwriting all
/// export parameters": the replacement definition's referenced resources are resolved exactly
/// as a create's are, and a replacement naming one that is not there leaves the stored
/// definition — and the instant it was last updated — exactly as they were.
#[tokio::test]
async fn an_update_resolves_the_replacement_s_referenced_resources() {
    let emulator = Deliveries::new();
    let table = "COST_AND_USAGE_REPORT";
    let held = "referenced-update-held";
    let also_held = "referenced-update-second";
    for bucket in [held, also_held] {
        emulator
            .bucket_with(bucket, Some(&published_grant(bucket)))
            .await;
    }
    let primary = primary_billing_view(&emulator).await;
    let absent_view = primary.replace("billingview/", "billingview/custom-never-created-");

    let created = emulator
        .ok(
            "CreateExport",
            &create_request("referenced_update", table, held, "exports/daily", None, None),
        )
        .await;
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();
    let before = emulator.ok("GetExport", &json!({ "ExportArn": &arn })).await;
    let definition = before["Export"].clone();

    // "All export parameters must be specified in the UpdateExport request": each replacement
    // below is the definition read back, with only the one member a row varies substituted.
    let replacement = |bucket: &str, view: Option<&str>| {
        let mut export = definition.clone();
        export["DestinationConfigurations"]["S3Destination"]["S3Bucket"] = json!(bucket);
        if let Some(view) = view {
            export["DataQuery"]["TableConfigurations"][table][BILLING_VIEW_ARN] = json!(view);
        }
        json!({ "ExportArn": &arn, "Export": export })
    };

    for (case, bucket, view, field, message) in [
        (
            "a replacement destination that was never created",
            "referenced-update-absent",
            None,
            S3_BUCKET_FIELD,
            BUCKET_PERMISSION_VALIDATION_FAILED.to_string(),
        ),
        (
            "a replacement billing view that was never created",
            held,
            Some(absent_view.as_str()),
            TABLE_CONFIGURATIONS_FIELD,
            format!("Billing view {absent_view} not found"),
        ),
    ] {
        let (status, body) = emulator
            .call("UpdateExport", &replacement(bucket, view))
            .await;
        assert_refused(case, status, &body, field, &message);
        let after = emulator.ok("GetExport", &json!({ "ExportArn": &arn })).await;
        assert_eq!(
            after["Export"], definition,
            "case {case:?}: a refused update must leave the stored definition untouched, got {}",
            after["Export"]
        );
        assert_eq!(
            after["ExportStatus"]["LastUpdatedAt"], before["ExportStatus"]["LastUpdatedAt"],
            "case {case:?}: a refused update must not move LastUpdatedAt, got {}",
            after["ExportStatus"]
        );
    }

    // The other half: a replacement whose referenced resources DO resolve is served and
    // observable.
    for (case, bucket, view) in [
        ("another bucket Amazon S3 holds", also_held, None),
        (
            "the account's own primary billing view",
            also_held,
            Some(primary.as_str()),
        ),
    ] {
        emulator.ok("UpdateExport", &replacement(bucket, view)).await;
        let after = emulator.ok("GetExport", &json!({ "ExportArn": &arn })).await;
        assert_eq!(
            after["Export"]["DestinationConfigurations"]["S3Destination"]["S3Bucket"].as_str(),
            Some(bucket),
            "case {case:?}: an accepted update must be observable, got {}",
            after["Export"]
        );
    }
}
