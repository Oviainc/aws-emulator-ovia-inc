// SPDX-License-Identifier: CC0-1.0

//! The `DestinationConfigurations` an export definition carries, over the awsJson1_1 wire.
//!
//! The rule under test: an operation that accepts an export definition refuses a
//! destination this service could not deliver to with the modelled `ValidationException`
//! (HTTP 400) and stores nothing — on `CreateExport` and on `UpdateExport` alike, because
//! an update "[overwrites] all export parameters. All export parameters must be provided
//! in the UpdateExport request" (smithy_query operation `UpdateExport`), so the
//! replacement destination is judged exactly as a create's is and a refused update leaves
//! the stored export untouched.
//!
//! Two facts about a destination make it undeliverable, and neither is expressible as a
//! constraint on one member (so the boundary validator cannot refuse either):
//!
//! 1. the output ENCODING: `S3OutputConfigurations` carries "The file format for the data
//!    export" beside "The compression type for the data export" (the modelled docs for
//!    `Format` and `Compression`), and a delivery writes ONE file — so only the published
//!    format/compression combinations name a file this service writes;
//! 2. the `S3Region`: "The S3 bucket Region" (the modelled doc for `S3Region`), whose own
//!    `@pattern ^[\S\s]*$` admits any text at all, so a value naming no Amazon Web
//!    Services Region names no bucket to deliver to.
//!
//! Each is asserted over a TABLE: the encoding rule over the WHOLE cross product of the
//! two modelled enums (`FORMAT_OPTION_VALUES` x `COMPRESSION_OPTION_VALUES`, every pair
//! judged, not only the one the issue cited), with the published output type and overwrite
//! preference varied across the rows so the rule is shown to be about the pair alone; the
//! Region rule over Region codes of several partitions and over values that miss a Region
//! in different ways (empty, wrong case, trailing space, right form but no such Region,
//! and one at the member's own length bound).

use serde_json::{json, Map, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::ops::types::{
    COMPRESSION_OPTION_VALUES, FORMAT_OPTION_VALUES, OVERWRITE_OPTION_VALUES,
    S3_OUTPUT_TYPE_VALUES,
};
use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::SUPPORTED_OUTPUT_ENCODINGS;

use crate::export_lifecycle::{cases, error_code, restated_table_configurations, Client};

/// The catalog table the definitions below query, and therefore the table whose
/// `TableConfigurations` entry an UPDATE has to restate.
const QUERIED_TABLE: &str = "COST_AND_USAGE_REPORT";

/// A Region every row delivers to unless it is varying the Region itself.
const HOME_REGION: &str = "us-east-1";

/// `S3Region`'s modelled `@length 0..1024` (smithy_query shape `S3Destination`, member
/// `S3Region`): the longest value the boundary validator lets through to the handler.
const S3_REGION_MAX_LEN: usize = 1024;

/// `ValidationException$Message`'s modelled `@length 0..1024`, which a refusal built from
/// a caller-supplied value has to stay inside.
const MESSAGE_MAX_LEN: usize = 1024;

/// One row of the ENCODING table: a `Format`/`Compression` pair and whether this service
/// delivers a file in it.
///
/// SOURCE: the AWS Billing and Cost Management user guide's data-export delivery options,
/// which publish the compression-and-file-format choices as `Parquet - parquet`,
/// `gzip - text/csv` and `zip - text/csv`; the spellings are the modelled enum values.
/// Written out here rather than read from the service so the expectation is independent of
/// the implementation, and `every_modelled_pair_has_a_row` pins that the rows are the whole
/// cross product of the two enums.
const ENCODING_ROWS: &[(&str, &str, bool)] = &[
    ("TEXT_OR_CSV", "GZIP", true),
    ("TEXT_OR_CSV", "ZIP", true),
    ("TEXT_OR_CSV", "PARQUET", false),
    ("PARQUET", "PARQUET", true),
    // The pair the issue reported accepted.
    ("PARQUET", "GZIP", false),
    ("PARQUET", "ZIP", false),
];

/// The `Export` a row submits: the leanest valid definition, with the name and the whole
/// `DestinationConfigurations` substituted.
fn export_body(name: &str, region: &str, output: (&str, &str, &str, &str)) -> Value {
    let mut export = cases().remove(0).create_request()["Export"].clone();
    let object = export
        .as_object_mut()
        .expect("the Export structure is a JSON object");
    object.insert("Name".to_string(), json!(name));
    let (output_type, format, compression, overwrite) = output;
    let mut s3 = Map::new();
    s3.insert("S3Bucket".to_string(), json!(format!("{name}-bucket")));
    s3.insert("S3Prefix".to_string(), json!("exports/daily"));
    s3.insert("S3Region".to_string(), json!(region));
    s3.insert(
        "S3OutputConfigurations".to_string(),
        json!({
            "OutputType": output_type,
            "Format": format,
            "Compression": compression,
            "Overwrite": overwrite,
        }),
    );
    object.insert(
        "DestinationConfigurations".to_string(),
        json!({ "S3Destination": Value::Object(s3) }),
    );
    export
}

/// The same definition as an UPDATE submits it: an update overwrites every export
/// parameter, so it restates the queried table's `TableConfigurations` entry.
fn update_body(name: &str, region: &str, output: (&str, &str, &str, &str)) -> Value {
    let mut export = export_body(name, region, output);
    if let Some(restated) = restated_table_configurations(&[QUERIED_TABLE], None) {
        export
            .as_object_mut()
            .expect("the Export structure is a JSON object")
            .get_mut("DataQuery")
            .and_then(Value::as_object_mut)
            .expect("the DataQuery structure is a JSON object")
            .insert("TableConfigurations".to_string(), restated);
    }
    export
}

/// The `S3Destination` a reader reports for `arn`.
async fn stored_destination(client: &Client, arn: &str) -> Value {
    let fetched = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
    fetched["Export"]["DestinationConfigurations"]["S3Destination"].clone()
}

/// How many exports the store holds.
async fn export_count(client: &Client) -> usize {
    let listed = client.ok("ListExports", &json!({})).await;
    listed["Exports"].as_array().map(Vec::len).unwrap_or_default()
}

/// An export delivered in an encoding this service writes, to update AGAINST: a refused
/// update has to leave this one exactly as it is.
async fn anchor_export(client: &Client, name: &str) -> String {
    let created = client
        .ok(
            "CreateExport",
            &json!({
                "Export": export_body(
                    name,
                    HOME_REGION,
                    (S3_OUTPUT_TYPE_VALUES[0], "TEXT_OR_CSV", "GZIP", OVERWRITE_OPTION_VALUES[0]),
                ),
            }),
        )
        .await;
    created["ExportArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateExport must return an ExportArn"))
        .to_string()
}

/// [`ENCODING_ROWS`] states every pair of the two modelled enums, so a value either enum
/// gains cannot slip past the table above.
#[test]
fn every_modelled_pair_has_a_row() {
    for format in FORMAT_OPTION_VALUES {
        for compression in COMPRESSION_OPTION_VALUES {
            let case = (*format, *compression);
            assert!(
                ENCODING_ROWS
                    .iter()
                    .any(|(row_format, row_compression, _)| (*row_format, *row_compression)
                        == case),
                "case {case:?}: every modelled Format/Compression pair must have a row, \
                 got rows {ENCODING_ROWS:?}"
            );
        }
    }
    assert_eq!(
        ENCODING_ROWS.len(),
        FORMAT_OPTION_VALUES.len() * COMPRESSION_OPTION_VALUES.len(),
        "the rows must be exactly the cross product of the two enums, got {} rows",
        ENCODING_ROWS.len()
    );
    // The encodings the service publishes are the ones this suite expects accepted, which
    // is what `export_lifecycle`'s rows are built from.
    for (format, compression, delivered) in ENCODING_ROWS {
        let published = SUPPORTED_OUTPUT_ENCODINGS
            .iter()
            .any(|(published_format, compressions)| {
                published_format == format && compressions.contains(compression)
            });
        assert_eq!(
            published, *delivered,
            "case {:?}: the published encodings must agree with the documented ones, got {:?}",
            (format, compression),
            SUPPORTED_OUTPUT_ENCODINGS
        );
    }
}

#[tokio::test]
async fn a_create_accepts_only_an_output_encoding_this_service_delivers() {
    let client = Client::new();
    let mut stored = 0;

    for (index, (format, compression, delivered)) in ENCODING_ROWS.iter().enumerate() {
        // The output type and the overwrite preference walk their own published value
        // lists, so no row's verdict can be coming from either of them.
        let output = (
            S3_OUTPUT_TYPE_VALUES[index % S3_OUTPUT_TYPE_VALUES.len()],
            *format,
            *compression,
            OVERWRITE_OPTION_VALUES[index % OVERWRITE_OPTION_VALUES.len()],
        );
        let case = format!("{format}/{compression}");
        let name = format!("encoding_{index}");
        let submitted = export_body(&name, HOME_REGION, output);
        let (status, body) = client
            .call("CreateExport", &json!({ "Export": submitted.clone() }))
            .await;

        if *delivered {
            assert_eq!(
                status, 200,
                "case {case:?}: a published encoding must be accepted, got {status} with {body}"
            );
            stored += 1;
            let arn = body["ExportArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an ExportArn, got {body}"))
                .to_string();
            assert_eq!(
                stored_destination(&client, &arn).await,
                submitted["DestinationConfigurations"]["S3Destination"],
                "case {case:?}: GetExport must report the destination that was stored"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(format!("Compression {compression} is invalid for Format {format}").as_str()),
                "case {case:?}: the refusal must name the pair it refused, got {body}"
            );
            assert_eq!(
                body["Fields"][0]["Name"].as_str(),
                Some(
                    "Export.DestinationConfigurations.S3Destination.S3OutputConfigurations.Compression"
                ),
                "case {case:?}: the refusal must name the offending member, got {body}"
            );
        }
        assert_eq!(
            export_count(&client).await,
            stored,
            "case {case:?}: a refused create must store nothing"
        );
    }
}

#[tokio::test]
async fn an_update_accepts_only_an_output_encoding_this_service_delivers() {
    let client = Client::new();

    for (index, (format, compression, delivered)) in ENCODING_ROWS.iter().enumerate() {
        let case = format!("{format}/{compression}");
        let arn = anchor_export(&client, &format!("updated_encoding_{index}")).await;
        let before = client.ok("GetExport", &json!({ "ExportArn": arn })).await;

        let output = (
            S3_OUTPUT_TYPE_VALUES[index % S3_OUTPUT_TYPE_VALUES.len()],
            *format,
            *compression,
            OVERWRITE_OPTION_VALUES[index % OVERWRITE_OPTION_VALUES.len()],
        );
        let submitted = update_body(&format!("updated_encoding_{index}"), HOME_REGION, output);
        let (status, body) = client
            .call(
                "UpdateExport",
                &json!({ "ExportArn": arn, "Export": submitted.clone() }),
            )
            .await;

        if *delivered {
            assert_eq!(
                status, 200,
                "case {case:?}: a published encoding must be accepted by an update, \
                 got {status} with {body}"
            );
            assert_eq!(
                stored_destination(&client, &arn).await,
                submitted["DestinationConfigurations"]["S3Destination"],
                "case {case:?}: GetExport must observe the replacement destination"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(format!("Compression {compression} is invalid for Format {format}").as_str()),
                "case {case:?}: the refusal must name the pair it refused, got {body}"
            );
            let after = client.ok("GetExport", &json!({ "ExportArn": arn })).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused update must leave the stored export untouched"
            );
        }
    }
}

/// Region codes a destination may name, one per partition shape: the commercial
/// partition, a non-US commercial Region, a Region added long after the service shipped,
/// GovCloud and China. SOURCE: `emulator_telemetry::policy::KNOWN_REGIONS`, "every AWS
/// region the wire may carry" — the catalog this emulator reads a caller-supplied Region
/// against. None of them is the Region the issue cited except the first.
const DELIVERABLE_REGIONS: &[&str] = &[
    "us-east-1",
    "eu-central-1",
    "ap-southeast-4",
    "us-gov-west-1",
    "cn-north-1",
];

/// Values that name no Region, differing in HOW they miss one.
fn undeliverable_regions() -> Vec<(&'static str, String)> {
    vec![
        ("empty", String::new()),
        ("not a Region code at all", "not-a-region".to_string()),
        ("a Region code in upper case", "US-EAST-1".to_string()),
        ("a Region code with a trailing space", "us-east-1 ".to_string()),
        // Region-code SHAPED, but no Region is named this: the rule is membership of the
        // Region catalog, not the spelling of the value.
        ("the right form, no such Region", "us-east-9".to_string()),
        ("a plausible but absent Region", "eu-west-9".to_string()),
        // At `S3Region`'s own modelled length bound, so the refusal is the handler's and
        // the message it builds stays inside `ValidationException$Message`'s bound.
        ("at the member's length bound", "r".repeat(S3_REGION_MAX_LEN)),
    ]
}

#[tokio::test]
async fn a_destination_region_must_name_an_aws_region() {
    let client = Client::new();
    let supported = (S3_OUTPUT_TYPE_VALUES[0], "TEXT_OR_CSV", "ZIP", OVERWRITE_OPTION_VALUES[1]);
    let mut stored = 0;

    for (index, region) in DELIVERABLE_REGIONS.iter().enumerate() {
        let name = format!("region_{index}");
        let submitted = export_body(&name, region, supported);
        let created = client
            .ok("CreateExport", &json!({ "Export": submitted.clone() }))
            .await;
        stored += 1;
        let arn = created["ExportArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {region:?}: expected an ExportArn, got {created}"))
            .to_string();
        assert_eq!(
            stored_destination(&client, &arn).await["S3Region"].as_str(),
            Some(*region),
            "case {region:?}: GetExport must report the Region that was stored"
        );

        // And the same Region is accepted as a REPLACEMENT destination.
        let replacement = update_body(&name, region, supported);
        client
            .ok(
                "UpdateExport",
                &json!({ "ExportArn": arn, "Export": replacement }),
            )
            .await;
    }

    let anchor = anchor_export(&client, "region_anchor").await;
    stored += 1;
    let before = client.ok("GetExport", &json!({ "ExportArn": anchor })).await;

    for (label, region) in undeliverable_regions() {
        let (status, body) = client
            .call(
                "CreateExport",
                &json!({ "Export": export_body("region_refused", &region, supported) }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        let message = body["Message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: expected a Message, got {body}"));
        assert!(
            message.starts_with("S3Region ") && message.ends_with(" is invalid"),
            "case {label:?}: the refusal must name the offending member's value, got {message:?}"
        );
        assert!(
            message.len() <= MESSAGE_MAX_LEN,
            "case {label:?}: expected a Message inside {MESSAGE_MAX_LEN} characters, got {}",
            message.len()
        );
        assert_eq!(
            body["Fields"][0]["Name"].as_str(),
            Some("Export.DestinationConfigurations.S3Destination.S3Region"),
            "case {label:?}: the refusal must name the offending member, got {body}"
        );
        assert_eq!(
            export_count(&client).await,
            stored,
            "case {label:?}: a refused create must store nothing"
        );

        // The same value is refused as a REPLACEMENT destination, and the stored export is
        // left as it was.
        let (status, body) = client
            .call(
                "UpdateExport",
                &json!({
                    "ExportArn": anchor,
                    "Export": update_body("region_anchor", &region, supported),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {label:?}: an update must be refused too, got {status} with {body}"
        );
        let after = client.ok("GetExport", &json!({ "ExportArn": anchor })).await;
        assert_eq!(
            after, before,
            "case {label:?}: a refused update must leave the stored export untouched"
        );
    }
}
